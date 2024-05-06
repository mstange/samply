use std::error::Error;
use std::fmt::Debug;
use std::path::Path;

use serde::de::DeserializeOwned;
use serde::Serialize;

pub trait UtilityProcess {
    const PROCESS_TYPE: &'static str;
    type Parent: UtilityProcessParent;
    type Child: UtilityProcessChild<Self::ParentToChildMsg, Self::ChildToParentMsg>;
    type ParentToChildMsg: Serialize + DeserializeOwned + Debug;
    type ChildToParentMsg: Serialize + DeserializeOwned + Debug;
}

pub trait UtilityProcessParent: Send + 'static {
    fn spawn_child(self, ipc_directory: &Path);
}

pub trait UtilityProcessChild<ParentToChildMsg, ChildToParentMsg> {
    fn handle_message(
        &mut self,
        msg: ParentToChildMsg,
    ) -> Result<ChildToParentMsg, Box<dyn Error + Send + Sync>>;
}
