//! This module defines the macros that ship with [samply-markers](crate).
//!
//! While it's possible to use the marker types directly, the macros provide a nicer experience.
//!
//! [samply-markers](crate) provides three primary macros for instrumenting code:
//!
//! * [`samply_marker!`](crate::samply_marker) - Emits an instant or interval marker at the current location.

mod samply_marker;
