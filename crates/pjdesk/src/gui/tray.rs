//! System-tray icon with a Show/Quit menu, integrated with the eframe loop.
//!
//! The icon lives on its own thread, with its own message loop. A click is
//! seen even when the window thread is busy. That thread only records the
//! click and wakes the window. It does not call `ShowWindow` and it does not
//! wait for the window thread.

/// What the user asked for via the tray.
#[cfg_attr(not(windows), allow(dead_code))]
pub enum Action {
    Show,
    Update,
    Revoke,
    Quit,
}

#[cfg(windows)]
pub use imp::Tray;
#[cfg(windows)]
mod imp {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Mutex;

    use eframe::egui;
    use tray_icon::menu::{Menu, MenuEvent, MenuItem};
    use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

    use super::Action;

    static MENU_TX: Mutex<Option<std::sync::mpsc::Sender<Action>>> = Mutex::new(None);
    static ICON_CLICKED: AtomicBool = AtomicBool::new(false);
    static TOOLTIP: Mutex<Option<String>> = Mutex::new(None);

    /// Restores the window from this thread and returns. `ShowWindowAsync`
    /// does not call the window procedure, so the click thread is not waiting.
    fn ask_open(reason: &str, ctx: &egui::Context) {
        unsafe {
            let _ = windows::Win32::UI::WindowsAndMessaging::AllowSetForegroundWindow(0xFFFF_FFFF);
        }
        ICON_CLICKED.store(true, Ordering::SeqCst);
        crate::winshow::show_from_tray("PJDesk");
        crate::traywatch::arm();
        ctx.request_repaint();
        tracing::info!("Tray: {reason}. Asked the PJDesk window to come forward. The tray thread restored it and did not wait.");
    }

    fn push_menu(action: Action) {
        // The window thread never holds this mutex. Two tries, then drop
        // the click, rather than waiting on the window.
        for _ in 0..2 {
            if let Ok(slot) = MENU_TX.try_lock() {
                if let Some(tx) = slot.as_ref() {
                    let _ = tx.send(action);
                }
                return;
            }
            std::thread::yield_now();
        }
        tracing::info!("Tray menu click was not delivered because the menu queue was busy.");
    }

    /// The window thread reads clicks from here. The icon stays on the tray thread.
    pub struct Tray {
        menu_rx: std::sync::mpsc::Receiver<Action>,
    }

    impl Tray {
        /// Builds the tray icon on its own thread, or returns `None` if that fails.
        pub fn new(ctx: &egui::Context) -> Option<Self> {
            let (menu_tx, menu_rx) = std::sync::mpsc::channel();
            let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel(1);
            let ctx = ctx.clone();
            let spawned = std::thread::Builder::new().name("pjdesk-tray".into()).spawn(move || {
                if let Ok(mut slot) = MENU_TX.lock() {
                    *slot = Some(menu_tx);
                }
                let Some(icon) = build(&ctx) else {
                    let _ = ready_tx.send(false);
                    return;
                };
                let _ = ready_tx.send(true);
                tracing::info!("The PJDesk tray icon is on its own thread. A click does not wait for the window.");
                pump(icon);
            });
            if spawned.is_err() {
                return None;
            }
            match ready_rx.recv_timeout(std::time::Duration::from_secs(2)) {
                Ok(true) => Some(Self { menu_rx }),
                _ => None,
            }
        }

        pub fn set_tooltip(&self, text: &str) {
            // The window thread does not call into the tray window. The tray
            // thread picks this up on its own loop.
            if let Ok(mut slot) = TOOLTIP.try_lock() {
                *slot = Some(text.to_string());
            }
        }

        /// Drains any pending tray actions. This runs on the window thread.
        pub fn poll(&self) -> Vec<Action> {
            let mut out = Vec::new();
            let clicked = ICON_CLICKED.swap(false, Ordering::SeqCst);
            let second = super::super::instance::show_requested();
            if clicked || second {
                out.push(Action::Show);
            }
            while let Ok(action) = self.menu_rx.try_recv() {
                out.push(action);
            }
            out
        }
    }

    fn build(ctx: &egui::Context) -> Option<TrayIcon> {
        let icon = Icon::from_rgba(include_bytes!("../../assets/icon_32.rgba").to_vec(), 32, 32).ok()?;
        let menu = Menu::new();
        let show = MenuItem::new("Open PJDesk", true, None);
        let update = MenuItem::new("Check for updates now", true, None);
        let revoke = MenuItem::new("Revoke access", true, None);
        let quit = MenuItem::new("Quit PJDesk", true, None);
        menu.append(&show).ok()?;
        menu.append(&update).ok()?;
        menu.append(&revoke).ok()?;
        menu.append(&quit).ok()?;
        let show_id = show.id().0.clone();
        let update_id = update.id().0.clone();
        let revoke_id = revoke.id().0.clone();
        let quit_id = quit.id().0.clone();

        let show_now = show_id.clone();
        let update_now = update_id.clone();
        let revoke_now = revoke_id.clone();
        let quit_now = quit_id.clone();
        let c1 = ctx.clone();
        MenuEvent::set_event_handler(Some(move |e: MenuEvent| {
            if e.id.0 == show_now {
                ask_open("Open", &c1);
                return;
            }
            let action = if e.id.0 == update_now {
                Action::Update
            } else if e.id.0 == revoke_now {
                Action::Revoke
            } else if e.id.0 == quit_now {
                Action::Quit
            } else {
                return;
            };
            push_menu(action);
            c1.request_repaint();
        }));
        let c2 = ctx.clone();
        TrayIconEvent::set_event_handler(Some(move |e: TrayIconEvent| {
            let click = match e {
                TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } => {
                    Some(pjdesk_proto::trayshow::Click::Left)
                }
                TrayIconEvent::DoubleClick { button: MouseButton::Left, .. } => Some(pjdesk_proto::trayshow::Click::LeftDouble),
                _ => None,
            };
            let Some(click) = click else { return };
            if !pjdesk_proto::trayshow::opens_window(click) {
                return;
            }
            let reason = if click == pjdesk_proto::trayshow::Click::LeftDouble { "double-click" } else { "left click" };
            ask_open(reason, &c2);
        }));

        TrayIconBuilder::new()
            .with_tooltip("PJDesk")
            .with_icon(icon)
            .with_menu(Box::new(menu))
            .with_menu_on_left_click(false)
            .build()
            .ok()
    }

    /// Pumps the tray window. This thread is not the window thread, so a
    /// stuck window cannot hide a click.
    fn pump(icon: TrayIcon) {
        use windows::Win32::Foundation::WAIT_OBJECT_0;
        use windows::Win32::UI::WindowsAndMessaging::{
            DispatchMessageW, MsgWaitForMultipleObjects, PeekMessageW, TranslateMessage, MSG, PM_REMOVE, QS_ALLINPUT,
            WM_QUIT,
        };
        loop {
            let tip = TOOLTIP.try_lock().ok().and_then(|mut slot| slot.take());
            if let Some(tip) = tip {
                let _ = icon.set_tooltip(Some(tip));
            }
            // A second launch sets this event. Restore here so a parked window
            // does not have to be awake before the window appears.
            if super::super::instance::show_requested() {
                crate::winshow::show_from_tray("PJDesk");
                ICON_CLICKED.store(true, Ordering::SeqCst);
            }
            let wait = unsafe { MsgWaitForMultipleObjects(None, false, 200, QS_ALLINPUT) };
            if wait != WAIT_OBJECT_0 {
                continue;
            }
            let mut saw = false;
            loop {
                let mut msg: MSG = unsafe { std::mem::zeroed() };
                if !unsafe { PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE) }.as_bool() {
                    break;
                }
                saw = true;
                if msg.message == WM_QUIT {
                    return;
                }
                unsafe {
                    let _ = TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            }
            if !saw {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        }
    }
}

#[cfg(not(windows))]
pub use stub::Tray;
#[cfg(not(windows))]
mod stub {
    use eframe::egui;

    use super::Action;

    pub struct Tray;
    impl Tray {
        pub fn new(_ctx: &egui::Context) -> Option<Self> {
            None
        }
        pub fn poll(&self) -> Vec<Action> {
            Vec::new()
        }
        pub fn set_tooltip(&self, _text: &str) {}
    }
}
