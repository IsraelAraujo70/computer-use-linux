#[cfg(target_os = "linux")]
pub mod i3;
#[cfg(target_os = "macos")]
pub mod mac;
#[cfg(target_os = "linux")]
pub mod x11;
