pub use debugid;

mod config;
mod debuginfod;
mod helper;
mod moria_mac;
#[cfg(target_os = "macos")]
mod moria_mac_spotlight;
mod symbol_manager;

pub use config::SymbolManagerConfig;
pub use samply_api::samply_symbols;
pub use samply_api::samply_symbols::{
    AddressInfo, CodeId, ElfBuildId, Error, ExternalFileAddressInFileRef, ExternalFileAddressRef,
    ExternalFileRef, ExternalFileSymbolMap, FilePath, FramesLookupResult, InlineStackFrame,
    LibraryInfo, MultiArchDisambiguator, PeCodeId, SymbolInfo,
};
pub use symbol_manager::{SymbolFileOrigin, SymbolManager, SymbolMap};
