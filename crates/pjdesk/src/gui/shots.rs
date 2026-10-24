//! Headless renders of the client window for review. Built with
//! `cargo test -p pjdesk --features ui-shots --bin pjdesk client_shots -- --nocapture`.

use eframe::egui;
use egui_kittest::kittest::{NodeT, Queryable};

use super::App;

#[test]
fn client_shots() {
    let dir = std::path::PathBuf::from(
        std::env::var("PJDESK_SHOT_DIR").unwrap_or_else(|_| "/opt/cursor/artifacts/pjdesk-ui".into()),
    );
    std::fs::create_dir_all(&dir).expect("create shot dir");

    for (name, theme, settings) in [
        ("main-dark", egui::Theme::Dark, false),
        ("main-light", egui::Theme::Light, false),
        ("settings-dark", egui::Theme::Dark, true),
    ] {
        let mut cfg = crate::config::Config::default();
        cfg.settings.theme = match theme {
            egui::Theme::Dark => "dark",
            egui::Theme::Light => "light",
        }
        .into();
        let mut harness = egui_kittest::Harness::builder()
            .with_size(egui::vec2(1180.0, 760.0))
            .with_pixels_per_point(1.0)
            .with_theme(theme)
            .wgpu()
            .build_eframe(move |cc| {
                let mut app = App::create(cc, cfg, true);
                app.load_preview(settings);
                app
            });
        harness.step();
        let image = harness.render().unwrap_or_else(|e| panic!("render {name}: {e}"));
        let path = dir.join(format!("{name}.png"));
        image.save(&path).unwrap_or_else(|e| panic!("save {}: {e}", path.display()));
        eprintln!("wrote {}", path.display());
    }

    for (name, theme, prep) in [
        ("saved-dark", egui::Theme::Dark, Prep::Saved),
        ("saved-light", egui::Theme::Light, Prep::Saved),
        ("password-dark", egui::Theme::Dark, Prep::Password),
        ("password-light", egui::Theme::Light, Prep::Password),
        ("signin-dark", egui::Theme::Dark, Prep::SignIn),
        ("signin-light", egui::Theme::Light, Prep::SignIn),
        ("netkey-dark", egui::Theme::Dark, Prep::NetworkKey),
        ("netkey-light", egui::Theme::Light, Prep::NetworkKey),
        ("servercfg-dark", egui::Theme::Dark, Prep::ServerConfig),
        ("servercfg-light", egui::Theme::Light, Prep::ServerConfig),
        ("import-offer-dark", egui::Theme::Dark, Prep::ImportOffer),
        ("import-offer-light", egui::Theme::Light, Prep::ImportOffer),
        ("empty-net-dark", egui::Theme::Dark, Prep::EmptyNetwork),
        ("empty-net-light", egui::Theme::Light, Prep::EmptyNetwork),
    ] {
        let height = if matches!(prep, Prep::EmptyNetwork) { 980.0 } else { 760.0 };
        let mut harness = egui_kittest::Harness::builder()
            .with_size(egui::vec2(1180.0, height))
            .with_pixels_per_point(1.0)
            .with_theme(theme)
            .wgpu()
            .build_eframe(move |cc| {
                let mut cfg = crate::config::Config::default();
                cfg.settings.theme = match theme {
                    egui::Theme::Dark => "dark",
                    egui::Theme::Light => "light",
                }
                .into();
                let mut app = App::create(cc, cfg, true);
                match prep {
                    Prep::Saved => app.load_saved_preview(),
                    Prep::Password => app.load_password_preview(),
                    Prep::SignIn => app.load_sign_in_preview(),
                    Prep::NetworkKey => app.load_network_key_preview(),
                    Prep::ServerConfig => app.load_server_config_preview(),
                    Prep::ImportOffer => app.load_import_offer_preview(),
                    Prep::EmptyNetwork => app.load_empty_network_preview(),
                }
                app
            });
        harness.step();
        let image = harness.render().unwrap_or_else(|e| panic!("render {name}: {e}"));
        let path = dir.join(format!("{name}.png"));
        image.save(&path).unwrap_or_else(|e| panic!("save {}: {e}", path.display()));
        eprintln!("wrote {}", path.display());
    }

    crate::viewer::render_monitor_shots(&dir);
}

/// Saved list at the widths Ben asked for. The size is the physical window;
/// 125% DPI lays the same pixels out in fewer points.
#[test]
fn saved_list_widths() {
    let dir = std::path::PathBuf::from(
        std::env::var("PJDESK_SHOT_DIR").unwrap_or_else(|_| "/opt/cursor/artifacts/pjdesk-saved".into()),
    );
    std::fs::create_dir_all(&dir).expect("create shot dir");
    for (name, px, ppp) in [
        ("900-100", 900.0_f32, 1.0_f32),
        ("900-125", 900.0, 1.25),
        ("1024-100", 1024.0, 1.0),
        ("1024-125", 1024.0, 1.25),
        ("1280-100", 1280.0, 1.0),
        ("1280-125", 1280.0, 1.25),
    ] {
        let points_w = px / ppp;
        let points_h = if points_w < 992.0 { 1280.0 } else { 820.0 };
        let mut harness = egui_kittest::Harness::builder()
            .with_size(egui::vec2(points_w, points_h))
            .with_pixels_per_point(ppp)
            .with_theme(egui::Theme::Light)
            .wgpu()
            .build_eframe(|cc| {
                let mut cfg = crate::config::Config::default();
                cfg.settings.theme = "light".into();
                let mut app = App::create(cc, cfg, true);
                app.load_saved_preview();
                app
            });
        harness.step();
        harness.step();
        let menus: Vec<_> = harness.get_all_by_label("Menu").collect();
        assert!(!menus.is_empty(), "{name}: no Menu buttons");
        let mut saw_connect = false;
        let mut saw_phone = false;
        for menu in &menus {
            let rect = menu.rect();
            assert!(
                rect.right() <= points_w - 12.0 && rect.right() >= points_w - 80.0,
                "{name}: Menu is not pinned inside the window. rect {rect:?} width {points_w}"
            );
            assert!(rect.width() >= 48.0, "{name}: Menu is clipped to {:.1}", rect.width());
            let row_y = rect.center().y;
            let actions = harness
                .query_all_by_label("Connect")
                .into_iter()
                .chain(harness.query_all_by_label("View phone"))
                .filter(|node| (node.rect().center().y - row_y).abs() < 10.0 && node.rect().right() <= rect.left() + 12.0)
                .collect::<Vec<_>>();
            assert_eq!(actions.len(), 1, "{name}: expected one action beside Menu at y {row_y}, got {}", actions.len());
            let action = actions[0].rect();
            assert!(action.left() >= 0.0 && action.right() <= rect.left() + 12.0, "{name}: action clipped {action:?}");
            assert!(action.width() >= 28.0, "{name}: action too narrow {:.1}", action.width());
            let label = actions[0].accesskit_node().label().unwrap_or_default();
            if label == "View phone" {
                saw_phone = true;
                if action.width() > 40.0 {
                    assert!(action.width() >= 90.0, "{name}: View phone text is clipped to {:.1}", action.width());
                }
            } else if label == "Connect" && action.width() > 40.0 {
                saw_connect = true;
            } else if label == "Connect" {
                saw_connect = true;
            }
        }
        assert!(saw_connect, "{name}: no Connect button in the list");
        assert!(saw_phone, "{name}: no View phone button in the list");
        let image = harness.render().unwrap_or_else(|e| panic!("render {name}: {e}"));
        let path = dir.join(format!("saved-{name}.png"));
        image.save(&path).unwrap_or_else(|e| panic!("save {}: {e}", path.display()));
        assert_eq!(image.width(), (points_w * ppp).round() as u32, "{name}: image width");
        eprintln!("wrote {} ({}x{})", path.display(), image.width(), image.height());
    }
}

#[derive(Clone, Copy)]
enum Prep {
    Saved,
    Password,
    SignIn,
    NetworkKey,
    ServerConfig,
    ImportOffer,
    EmptyNetwork,
}
