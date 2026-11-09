//! macOS platform layer. `lib.rs` mounts these under the module names the
//! server uses on Linux (`atspi_tree_impl`, `x11_display`, ...), so one
//! server, one tool contract and one behavior serve both platforms.

pub(crate) mod ax_tree;
pub(crate) mod clipboard;
pub(crate) mod diagnostics;
pub(crate) mod display;
pub(crate) mod dnd;
pub(crate) mod input;
