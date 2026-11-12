//! `dictto-gnome-settings`: the GTK settings window, started by `dictto-gnome --settings`.
//! The rules live in `form` and `chord`, which build and test on every platform.
#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

mod chord;
mod form;

#[cfg(target_os = "linux")]
mod bus;
// Shared with the daemon so a recorded shortcut names the physical keys it listens to.
#[cfg(target_os = "linux")]
#[allow(dead_code)]
#[path = "../../gnome/src/keyboard/keymap.rs"]
mod keymap;
#[cfg(target_os = "linux")]
#[allow(dead_code)]
#[path = "../../gnome/src/paths.rs"]
mod paths;
#[cfg(target_os = "linux")]
mod ui;

fn main() {
    #[cfg(target_os = "linux")]
    {
        env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
        std::process::exit(ui::run().into());
    }
    #[cfg(not(target_os = "linux"))]
    {
        eprintln!("dictto-gnome-settings runs on Linux only");
        std::process::exit(1);
    }
}
