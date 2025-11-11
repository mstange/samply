mod macros;
mod provider;

pub mod marker;

/// This module contains everything needed to add markers within a convenient glob import.
///
/// ```rust
/// use samply_markers::prelude::*;
/// ```
pub mod prelude {
    pub use crate::marker::SamplyMarker;
    pub use crate::marker::SamplyTimer;
    pub use crate::marker::SamplyTimestamp;

    #[doc(inline)]
    pub use crate::samply_marker;
    #[doc(inline)]
    pub use crate::samply_measure;
    #[doc(inline)]
    pub use crate::samply_timer;
}
