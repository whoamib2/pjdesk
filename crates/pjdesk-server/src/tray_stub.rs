//! Tray stand-in for builds that are not Windows. The real icon is Windows-only.

#![cfg_attr(not(windows), allow(dead_code))]

use std::path::Path;

pub enum Action {
    Show,
    CheckUpdates,
    Promote,
    Demote,
    SyncNow,
    Quit,
}

pub fn acquire_window() -> bool {
    true
}

pub fn holds_window() -> bool {
    false
}

pub fn gui_is_running() -> bool {
    false
}

pub fn tray_is_running() -> bool {
    false
}

pub fn signal_gui() {}

pub fn signal_tray() {}

pub fn signal_gui_quit() {}

pub fn spawn_tray(_port: u16, _data_dir: &Path) -> bool {
    false
}

pub fn run_tray(_data_dir: std::path::PathBuf, _port: u16) -> anyhow::Result<()> {
    Ok(())
}

pub fn gui_actions() -> Vec<Action> {
    Vec::new()
}

pub fn ensure_login_default() {}

pub fn login_enabled() -> bool {
    true
}

pub fn set_login(_enable: bool) {}

pub struct Tray;

impl Tray {
    pub fn set_tooltip(&self, _text: &str) {}

    pub fn poll(&self) -> Vec<Action> {
        Vec::new()
    }
}

pub fn open_log_folder(_dir: &Path) {}
