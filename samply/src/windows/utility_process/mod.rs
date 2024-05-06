//! This module contains APIs which allow launching a utility process and
//! having bidirectional communication with it.
//!
//! Usage:
//!
//!  - Implement the [`UtilityProcess`] trait and its associated traits.
//!  - In the parent process, create a [`UtilityProcessSession`]. This will set up a
//!    communication channel on the file system and then call your implementation
//!    of [`UtilityProcessParent::spawn_child`] to launch the child.
//!  - In the child process, call [`run_child`]. This will process incoming messages
//!    and call your implementation of [`UtilityProcessChild::handle_message`] for
//!    each message.
//!  - In the parent process, call [`UtilityProcessSession::send_msg_and_wait_for_response`]
//!    whenever you want to send a message.

mod child;
mod file_channel;
mod parent;
mod shared;
mod traits;

pub use child::run_child;
pub use parent::UtilityProcessSession;
pub use traits::*;
