//! Tray stand-in for builds that are not Windows. The real icon is Windows-only.

#![cfg_attr(not(windows), allow(dead_code))]

use std::path::Path;

use eframe::egui;

pub enum Action {
    Show,
    ServiceStart,
    ServiceStop,
    ServiceRestart,
    CheckUpdates,
    OpenLog,
    Promote,
    Demote,
    SyncNow,
    Quit,
}

pub fn acquire_or_signal() -> bool {
    true
}

pub fn holds_window() -> bool {
    false
}

pub fn ensure_login_default() {}

pub fn login_enabled() -> bool {
    true
}

pub fn set_login(_enable: bool) {}

pub fn set_window_open(_open: bool) {}

pub fn process_should_exit() -> bool {
    false
}

pub fn request_quit() {}

pub fn icon_running() -> bool {
    false
}

pub fn start_without_window() -> bool {
    false
}

pub fn wait_for_reopen(_name: &str) -> bool {
    false
}

pub struct Tray;

impl Tray {
    pub fn new(_ctx: &egui::Context) -> Option<Self> {
        None
    }

    pub fn set_tooltip(&self, _text: &str) {}

    pub fn poll(&self) -> Vec<Action> {
        Vec::new()
    }
}

pub fn open_log_folder(_dir: &Path) {}
