//! Headless renders of the client window for review. Built with
//! `cargo test -p pjdesk --features ui-shots --lib client_shots -- --nocapture`.

use eframe::egui;

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
}
