use std::sync::{Arc, Mutex, OnceLock};

use tokio::sync::oneshot;

static INSTANCE: OnceLock<Arc<Mutex<CtrlCState>>> = OnceLock::new();

pub type Receiver = oneshot::Receiver<()>;

/// Provides Ctrl+C notifications, and allows suppressing the automatic termination
/// of the process.
///
/// Ctrl+C can only be suppressed "once at a time". Imagine the following scenario:
///
///  - You call `CtrlC::observe_oneshot()` and store the receiver in a variable
///    that remains alive for the upcoming long-running activity.
///  - The long-running is running.
///  - The user presses Ctrl+C. This sends a message to the receiver.
///  - You take your time reacting to the message, or you don't check it often enough.
///  - In the meantime, the user presses Ctrl+C again.
///
/// This second press terminates the process, which is what the user expects.
pub struct CtrlC;

impl CtrlC {
    /// Returns a new [`Receiver`] which will receive a message once
    /// Ctrl+C is pressed.
    ///
    /// Suspends the automatic termination for *one* Ctrl+C.
    ///
    /// But the Ctrl+C after that will terminate again - unless `observe_oneshot`
    /// has been called again since then.
    ///
    /// Furthermore, once the receiver is dropped, Ctrl+C will also terminate the process.
    ///
    /// ## Usage
    ///
    /// ### Example 1: Suspend automatic termination for a given scope
    ///
    /// ```
    /// let mut ctrl_c_receiver = CtrlC::observe_oneshot();
    ///
    /// // do something
    /// // [...]
    ///
    /// ctrl_c_receiver.close(); // Restores automatic termination behavior
    /// ```
    ///
    /// ### Example 2: Suspend automatic termination and check if Ctrl+C was pressed
    ///
    /// ```
    /// let mut ctrl_c_receiver = CtrlC::observe_oneshot();
    ///
    /// // do something
    /// // [...]
    ///
    /// match ctrl_c_receiver.try_recv() {
    ///     Ok(()) => {
    ///         // Ctrl+C was pressed once. If it had been pressed another time then we wouldn't
    ///         // be here because the process would already have terminated.
    ///     }
    ///     Err(TryRecvError::Empty) => {
    ///         // Ctrl+C was not pressed.
    ///     }
    ///     Err(TryRecvError::Closed) => {
    ///         // Someone else has called `CtrlC::observe_oneshot()` in the meantime and swapped
    ///         // out our handler.
    ///         // When our handler was active, Ctrl+C was not pressed.
    ///     }
    /// }
    /// ctrl_c_receiver.close(); // Restores automatic termination behavior
    /// // Alternatively, just drop ctrl_c_receiver, or let it go out of scope.
    /// ```
    ///
    /// ### Example 3: Keep checking for Ctrl+C in a loop
    ///
    /// ```
    /// let mut ctrl_c_receiver = CtrlC::observe_oneshot();
    ///
    /// loop {
    ///     if ctrl_c_receiver.try_recv().is_ok() {
    ///         // Ctrl+C was pressed once. Exit the loop.
    ///         // (If Ctrl+C had been pressed more than once then we wouldn't
    ///         // be here because the process would already have terminated.)
    ///         break;
    ///     }
    ///
    ///     // do something
    ///     // [...]
    /// }
    ///
    /// ctrl_c_receiver.close(); // Restores automatic termination behavior
    /// // Alternatively, just drop ctrl_c_receiver, or let it go out of scope.
    /// ```
    ///
    /// ### Example 4: Loop on a future and stop early if Ctrl+C is pressed
    ///
    /// ```
    /// let mut ctrl_c_receiver = CtrlC::observe_oneshot();
    ///
    /// loop {
    ///     tokio::select! {
    ///         ctrl_c_result = &mut ctrl_c_receiver => {
    ///             match ctrl_c_result {
    ///                 Ok(()) => {
    ///                     // Ctrl+C was pressed once. If it had been pressed another time then we wouldn't
    ///                     // be here because the process would already have terminated.
    ///                 }
    ///                 Err(e) => {
    ///                     // Someone else has called `CtrlC::observe_oneshot()` in the meantime and swapped
    ///                     // out our handler.
    ///                     // When our handler was active, Ctrl+C was not pressed.
    ///                 }
    ///             }
    ///         }
    ///         something_else = some_other_future => {
    ///             // [...]
    ///         }
    ///     }
    ///
    ///     // do something
    ///     // [...]
    /// }
    ///
    /// ctrl_c_receiver.close(); // Restores automatic termination behavior
    /// // Alternatively, just drop ctrl_c_receiver, or let it go out of scope.
    /// ```
    pub fn observe_oneshot() -> oneshot::Receiver<()> {
        let (tx, rx) = oneshot::channel();
        CtrlCState::get().lock().unwrap().current_sender = Some(tx);
        rx
    }
}

struct CtrlCState {
    current_sender: Option<oneshot::Sender<()>>,
}

impl CtrlCState {
    pub fn get() -> &'static Arc<Mutex<CtrlCState>> {
        INSTANCE.get_or_init(|| {
            ctrlc::set_handler(|| {
                let sender = CtrlCState::get().lock().unwrap().current_sender.take();
                if let Some(sender) = sender {
                    if let Ok(()) = sender.send(()) {
                        // The receiver still existed. Trust that it will handle this Ctrl+C.
                        // Do not terminate this process.
                        return;
                    }
                }

                // We get here if there is no current handler installed, or if the
                // receiver has been destroyed.
                // Terminate the process.
                terminate_for_ctrl_c();
            })
            .expect("Couldn't install Ctrl+C handler");
            Arc::new(Mutex::new(CtrlCState {
                current_sender: None,
            }))
        })
    }
}

fn terminate_for_ctrl_c() -> ! {
    std::process::exit(1)
}
