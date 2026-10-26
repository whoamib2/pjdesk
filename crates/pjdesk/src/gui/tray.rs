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
pub use imp::{icon_running, process_should_exit, request_quit, set_window_open, Tray, wait_for_reopen};
#[cfg(windows)]
mod imp {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Mutex;

    use eframe::egui;
    use tray_icon::menu::{Menu, MenuEvent, MenuItem};
    use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

    use super::Action;

    static MENU_TX: Mutex<Option<std::sync::mpsc::Sender<Action>>> = Mutex::new(None);
    static MENU_RX: Mutex<Option<std::sync::mpsc::Receiver<Action>>> = Mutex::new(None);
    static DEFERRED: Mutex<Vec<Action>> = Mutex::new(Vec::new());
    static ICON_CLICKED: AtomicBool = AtomicBool::new(false);
    static REOPEN: AtomicBool = AtomicBool::new(false);
    static QUIT: AtomicBool = AtomicBool::new(false);
    static WINDOW_OPEN: AtomicBool = AtomicBool::new(false);
    static ICON_READY: AtomicBool = AtomicBool::new(false);
    static ICON_THREAD: AtomicBool = AtomicBool::new(false);
    static TOOLTIP: Mutex<Option<String>> = Mutex::new(None);
    static WAKE: Mutex<Option<egui::Context>> = Mutex::new(None);

    fn window_is_open() -> bool {
        WINDOW_OPEN.load(Ordering::SeqCst)
    }

    /// The window thread sets this while eframe is running.
    pub fn set_window_open(open: bool) {
        WINDOW_OPEN.store(open, Ordering::SeqCst);
        if !open {
            if let Ok(mut slot) = WAKE.lock() {
                *slot = None;
            }
        }
    }

    pub fn process_should_exit() -> bool {
        QUIT.load(Ordering::SeqCst)
    }

    pub fn request_quit() {
        QUIT.store(true, Ordering::SeqCst);
    }

    pub fn icon_running() -> bool {
        ICON_READY.load(Ordering::SeqCst)
    }

    fn set_wake(ctx: &egui::Context) {
        if let Ok(mut slot) = WAKE.lock() {
            *slot = Some(ctx.clone());
        }
    }

    fn wake() {
        let ctx = WAKE.lock().ok().and_then(|slot| slot.clone());
        if let Some(ctx) = ctx {
            ctx.request_repaint();
        }
    }

    fn ensure_channel() {
        let mut rx = MENU_RX.lock().unwrap();
        if rx.is_none() {
            let (tx, recv) = std::sync::mpsc::channel();
            if let Ok(mut slot) = MENU_TX.lock() {
                *slot = Some(tx);
            }
            *rx = Some(recv);
        }
    }

    /// Blocks on this thread until a tray click should open a window, or Quit.
    /// Logs on a timer so a closed window can be told apart from a stuck one.
    pub fn wait_for_reopen(name: &str) -> bool {
        let mut logged_at = std::time::Instant::now() - std::time::Duration::from_secs(3);
        loop {
            if process_should_exit() {
                return false;
            }
            if REOPEN.swap(false, Ordering::SeqCst) {
                return true;
            }
            let mut open = false;
            if let Ok(mut slot) = MENU_RX.lock() {
                if let Some(rx) = slot.as_mut() {
                    while let Ok(action) = rx.try_recv() {
                        if matches!(action, Action::Quit) {
                            request_quit();
                            return false;
                        }
                        if let Ok(mut deferred) = DEFERRED.lock() {
                            deferred.push(action);
                        }
                        open = true;
                    }
                }
            }
            if open {
                return true;
            }
            let elapsed = logged_at.elapsed().as_millis() as u64;
            if pjdesk_proto::trayshow::parked_alive_due(elapsed) {
                tracing::info!("The tray loop is alive. The {name} window is closed and is not being drawn.");
                logged_at = std::time::Instant::now();
            }
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
    }

    /// Restores a live window, or asks the main thread to create one.
    /// There is no surface to present into while the window is closed.
    fn ask_open(reason: &str) {
        unsafe {
            let _ = windows::Win32::UI::WindowsAndMessaging::AllowSetForegroundWindow(0xFFFF_FFFF);
        }
        if window_is_open() {
            ICON_CLICKED.store(true, Ordering::SeqCst);
            crate::winshow::show_from_tray("PJDesk");
            crate::traywatch::arm();
            wake();
            tracing::info!("Tray: {reason}. Asked the PJDesk window to come forward. The tray thread restored it and did not wait.");
        } else {
            REOPEN.store(true, Ordering::SeqCst);
            tracing::info!("Tray: {reason}. The window is closed. Opening a new one. Nothing is drawn until it is open.");
        }
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
    pub struct Tray;

    impl Tray {
        /// Builds the tray icon on its own thread, or returns `None` if that fails.
        /// A second window reuses the icon that stayed up while this one was closed.
        pub fn new(ctx: &egui::Context) -> Option<Self> {
            set_wake(ctx);
            ensure_channel();
            if ICON_THREAD.swap(true, Ordering::SeqCst) {
                return if ICON_READY.load(Ordering::SeqCst) { Some(Self) } else { None };
            }
            let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel(1);
            let spawned = std::thread::Builder::new().name("pjdesk-tray".into()).spawn(move || {
                let Some(icon) = build() else {
                    let _ = ready_tx.send(false);
                    ICON_THREAD.store(false, Ordering::SeqCst);
                    return;
                };
                let _ = ready_tx.send(true);
                tracing::info!("The PJDesk tray icon is on its own thread. A click does not wait for the window.");
                pump(icon);
            });
            if spawned.is_err() {
                ICON_THREAD.store(false, Ordering::SeqCst);
                return None;
            }
            match ready_rx.recv_timeout(std::time::Duration::from_secs(2)) {
                Ok(true) => {
                    ICON_READY.store(true, Ordering::SeqCst);
                    Some(Self)
                }
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
            if let Ok(mut deferred) = DEFERRED.lock() {
                out.append(&mut deferred);
            }
            let clicked = ICON_CLICKED.swap(false, Ordering::SeqCst);
            let reopen = REOPEN.swap(false, Ordering::SeqCst);
            let second = super::super::instance::show_requested();
            if clicked || reopen || second {
                out.push(Action::Show);
            }
            if let Ok(mut slot) = MENU_RX.lock() {
                if let Some(rx) = slot.as_mut() {
                    while let Ok(action) = rx.try_recv() {
                        out.push(action);
                    }
                }
            }
            out
        }
    }

    fn build() -> Option<TrayIcon> {
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
        MenuEvent::set_event_handler(Some(move |e: MenuEvent| {
            if e.id.0 == show_now {
                ask_open("Open");
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
            wake();
        }));
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
            ask_open(reason);
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
            // A second launch sets this event. A live window is restored here.
            // A closed window is created by the main thread.
            if super::super::instance::show_requested() {
                if window_is_open() {
                    crate::winshow::show_from_tray("PJDesk");
                    ICON_CLICKED.store(true, Ordering::SeqCst);
                } else {
                    REOPEN.store(true, Ordering::SeqCst);
                    tracing::info!("A second launch asked for PJDesk. The window was closed, so it will be opened again.");
                }
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
pub use stub::{icon_running, process_should_exit, request_quit, set_window_open, Tray, wait_for_reopen};
#[cfg(not(windows))]
mod stub {
    use eframe::egui;

    use super::Action;

    pub fn set_window_open(_open: bool) {}

    pub fn process_should_exit() -> bool {
        false
    }

    pub fn request_quit() {}

    pub fn icon_running() -> bool {
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
        pub fn poll(&self) -> Vec<Action> {
            Vec::new()
        }
        pub fn set_tooltip(&self, _text: &str) {}
    }
}
