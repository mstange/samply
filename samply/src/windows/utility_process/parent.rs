use std::error::Error;
use std::fmt::Debug;

use tempfile::TempDir;

use super::file_channel::{BidiChannelCreator, Receiver, Sender};
use super::shared::{
    ChildToParentMsgWrapper, InitMessage, ParentToChildMsgWrapper, PKG_NAME, PKG_VERSION,
};
use super::traits::{UtilityProcess, UtilityProcessParent};

#[derive(Debug)]
pub struct UtilityProcessSession<T: UtilityProcess> {
    _ipc_dir: TempDir,
    spawn_thread: std::thread::JoinHandle<()>,
    sender: Sender<ParentToChildMsgWrapper<T::ParentToChildMsg>>,
    receiver: Receiver<ChildToParentMsgWrapper<T::ChildToParentMsg>>,
}

impl<T: UtilityProcess> UtilityProcessSession<T> {
    pub fn spawn_process(parent: T::Parent) -> Result<Self, Box<dyn Error + Send + Sync>> {
        let ipc_dir = TempDir::with_prefix("samply-elevated-helper-")?;
        let channel = BidiChannelCreator::create_in_parent(ipc_dir.path())?;

        let ipc_dir_path = ipc_dir.path().to_owned();
        let spawn_thread = std::thread::Builder::new()
            .name("UtilityProcessSession".into())
            .spawn(move || parent.spawn_child(&ipc_dir_path))?;
        let (receiver, sender) = channel.wait_for_child_to_connect()?;

        let mut session = Self {
            _ipc_dir: ipc_dir,
            spawn_thread,
            sender,
            receiver,
        };

        let init_reply = session.send_msg_and_wait_for_response_impl(
            ParentToChildMsgWrapper::Init(InitMessage {
                helper_type: T::PROCESS_TYPE.into(),
                pkg_name: PKG_NAME.into(),
                pkg_version: PKG_VERSION.into(),
            }),
        )?;
        match init_reply {
            ChildToParentMsgWrapper::AckInit => Ok(session),
            ChildToParentMsgWrapper::Err(err) => Err(err.into()),
            other_reply => Err(format!("Unexpected reply to init msg: {other_reply:?}").into()),
        }
    }

    pub fn send_msg_and_wait_for_response(
        &mut self,
        msg: T::ParentToChildMsg,
    ) -> Result<T::ChildToParentMsg, Box<dyn Error + Send + Sync>> {
        let reply = self.send_msg_and_wait_for_response_impl(ParentToChildMsgWrapper::Msg(msg))?;
        match reply {
            ChildToParentMsgWrapper::AckMsg(reply) => Ok(reply),
            ChildToParentMsgWrapper::Err(err) => Err(err.into()),
            _ => Err(format!("Unexpected reply from helper: {reply:?}").into()),
        }
    }

    fn send_msg_and_wait_for_response_impl(
        &mut self,
        msg: ParentToChildMsgWrapper<T::ParentToChildMsg>,
    ) -> Result<ChildToParentMsgWrapper<T::ChildToParentMsg>, Box<dyn Error + Send + Sync>> {
        log::info!("Sending message to elevated helper: {msg:?}");
        self.sender.send(msg)?;
        let reply: ChildToParentMsgWrapper<T::ChildToParentMsg> = self.receiver.recv_blocking()?;
        log::info!("Received reply from elevated helper: {reply:?}");
        Ok(reply)
    }

    pub fn shutdown(mut self) {
        let reply_res = self.send_msg_and_wait_for_response_impl(ParentToChildMsgWrapper::Shutdown);
        match reply_res {
            Ok(ChildToParentMsgWrapper::AckShutdown) => {}
            other_msg => log::warn!("Unexpected reply to Shutdown msg: {other_msg:?}"),
        }

        self.spawn_thread.join().unwrap();
    }
}
