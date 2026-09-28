#[doc(hidden)]
pub mod terminal;

#[allow(dead_code)]
pub(crate) mod chooser_child;
#[allow(dead_code)]
pub(crate) mod chooser_protocol;
#[cfg(any(target_os = "linux", target_os = "freebsd"))]
pub(crate) mod service;
