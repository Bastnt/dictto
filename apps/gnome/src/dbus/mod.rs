//! The daemon's D-Bus surface. The XML under `apps/gnome/dbus` is the contract that the
//! Shell extension and the settings process are written against.
pub mod app;
pub mod shell;

pub const APP_BUS_NAME: &str = "com.dictto.App";
pub const APP_PATH: &str = "/com/dictto/App";
pub const SHELL_BUS_NAME: &str = "com.dictto.Shell";
pub const SHELL_PATH: &str = "/com/dictto/Shell";
