use std::fmt::Debug;

use serde_derive::{Deserialize, Serialize};

pub const PKG_VERSION: &str = env!("CARGO_PKG_VERSION");
pub const PKG_NAME: &str = env!("CARGO_PKG_NAME");

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "t", content = "c")]
pub enum ParentToChildMsgWrapper<T> {
    Init(InitMessage),
    Msg(T),
    Shutdown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InitMessage {
    pub helper_type: String,
    pub pkg_name: String,
    pub pkg_version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "t", content = "c")]
#[allow(clippy::enum_variant_names)]
pub enum ChildToParentMsgWrapper<T> {
    AckInit,
    AckMsg(T),
    AckShutdown,
    Err(String),
}
