use std::error::Error;
use std::path::Path;

use super::file_channel::BidiChannelCreator;
use super::shared::{
    ChildToParentMsgWrapper, InitMessage, ParentToChildMsgWrapper, PKG_NAME, PKG_VERSION,
};
use super::traits::{UtilityProcess, UtilityProcessChild};

pub fn run_child<T: UtilityProcess>(ipc_directory: &Path, child: T::Child) {
    match run_child_internal::<T>(ipc_directory, child) {
        Ok(()) => {}
        Err(e) => log::error!("Error running elevated helper: {e:?}"),
    }
}

pub fn run_child_internal<T: UtilityProcess>(
    ipc_directory: &Path,
    mut child: T::Child,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    let (mut receiver, mut sender) = BidiChannelCreator::open_in_child(ipc_directory)?;

    let init_msg: ParentToChildMsgWrapper<T::ParentToChildMsg> = receiver.recv_blocking()?;
    if let Err(e) = check_init_msg::<T>(init_msg) {
        let _ = sender.send(ChildToParentMsgWrapper::Err(e.to_string()));
        return Err(e);
    }
    sender.send(ChildToParentMsgWrapper::AckInit)?;

    let mut shutting_down = false;
    while !shutting_down {
        let msg: ParentToChildMsgWrapper<T::ParentToChildMsg> = receiver.recv_blocking()?;
        let reply: ChildToParentMsgWrapper<T::ChildToParentMsg> = match msg {
            init_msg @ ParentToChildMsgWrapper::Init { .. } => ChildToParentMsgWrapper::Err(
                format!("Unexpected init message after initialization: {init_msg:?}"),
            ),
            ParentToChildMsgWrapper::Msg(msg) => match child.handle_message(msg) {
                Ok(reply) => ChildToParentMsgWrapper::AckMsg(reply),
                Err(e) => ChildToParentMsgWrapper::Err(e.to_string()),
            },
            ParentToChildMsgWrapper::Shutdown => {
                shutting_down = true;
                ChildToParentMsgWrapper::AckShutdown
            }
        };
        sender.send(reply)?;
    }

    Ok(())
}

fn check_init_msg<T: UtilityProcess>(
    init_msg: ParentToChildMsgWrapper<T::ParentToChildMsg>,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    let ParentToChildMsgWrapper::Init(init_msg) = init_msg else {
        return Err(format!("Unexpected init message type: {init_msg:?}").into());
    };

    let InitMessage {
        helper_type,
        pkg_name,
        pkg_version,
    } = init_msg;

    if helper_type != T::PROCESS_TYPE {
        return Err(format!(
            "Unexpected helper_type {helper_type}, expected {}",
            T::PROCESS_TYPE
        )
        .into());
    }
    if pkg_name != PKG_NAME {
        return Err(format!("Unexpected pkg_name {pkg_name}, expected {PKG_NAME}").into());
    }
    if pkg_version != PKG_VERSION {
        return Err(format!("Unexpected pkg_version {pkg_version}, expected {PKG_VERSION}").into());
    }
    Ok(())
}
