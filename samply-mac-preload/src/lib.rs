use perfrecord_mach_ipc_rendezvous::{channel, mach_task_self, OsIpcChannel, OsIpcSender};

// Run our code as early as possible, by pretending to be a global constructor.
// This code was taken from https://github.com/neon-bindings/neon/blob/2277e943a619579c144c1da543874f4a7ec39879/src/lib.rs#L40-L44
#[used]
#[allow(non_upper_case_globals)]
#[cfg_attr(target_os = "macos", link_section = "__DATA,__mod_init_func")]
static __SETUP_SAMPLY_CONNECTION: unsafe extern "C" fn() = {
    unsafe extern "C" fn __load_samply_lib() {
        let _ = set_up_samply_connection();
    }
    __load_samply_lib
};

fn set_up_samply_connection() -> Option<()> {
    let name = std::env::var("SAMPLY_BOOTSTRAP_SERVER_NAME").ok()?;
    let (tx0, rx0) = channel().ok()?;
    let tx1 = OsIpcSender::connect(name).ok()?;
    // We have a connection to the parent.

    // Send our task to the parent. Then the parent can control us completely.
    let p = mach_task_self();
    let c = OsIpcChannel::RawPort(p);
    let pid = std::process::id();
    let mut message_bytes = Vec::new();
    message_bytes.extend_from_slice(b"My task");
    message_bytes.extend_from_slice(&pid.to_le_bytes());
    tx1.send(&message_bytes, vec![OsIpcChannel::Sender(tx0), c], vec![])
        .ok()?;
    // Wait for the parent to tell us to proceed, in case it wants to do any more setup with our task.
    let (result, _, _) = rx0.recv().ok()?;
    assert_eq!(b"Proceed", &result[..]);
    Some(())
}
