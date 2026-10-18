//! Headless renders of the server window.
//! `cargo test -p pjdesk-server --features ui-shots --bin pjdesk-server server_shots -- --nocapture`

use eframe::egui;

use super::App;
use crate::status_view::Snapshot;

#[test]
fn server_shots() {
    let dir = std::path::PathBuf::from(
        std::env::var("PJDESK_SHOT_DIR").unwrap_or_else(|_| "/opt/cursor/artifacts/pjdesk-ui".into()),
    );
    std::fs::create_dir_all(&dir).expect("create shot dir");
    for (name, locked) in [("server-dark", false), ("server-unelevated", true)] {
        let mut harness = egui_kittest::Harness::builder()
            .with_size(egui::vec2(960.0, 1040.0))
            .with_pixels_per_point(1.0)
            .with_theme(egui::Theme::Dark)
            .wgpu()
            .build_eframe(move |cc| preview(cc, locked));
        harness.step();
        let image = harness.render().unwrap_or_else(|e| panic!("render {name}: {e}"));
        let path = dir.join(format!("{name}.png"));
        image.save(&path).unwrap_or_else(|e| panic!("save {}: {e}", path.display()));
        eprintln!("wrote {}", path.display());
    }
}

fn preview(cc: &eframe::CreationContext<'_>, locked: bool) -> App {
    let dir = std::env::temp_dir().join(if locked { "pjdesk-server-shot-locked" } else { "pjdesk-server-shot-open" });
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    if !locked {
        let _ = pjdesk_proto::tls::TlsIdentity::load_or_create(
            &dir.join("server-cert.pem"),
            &dir.join("server-key.pem"),
            "pjdesk-server",
        );
        std::fs::write(
            dir.join("server-update.json"),
            r#"{"auto":true,"window_start":"","window_end":"","jitter_secs":9}"#,
        )
        .unwrap();
        std::fs::write(
            dir.join("server-update-status.json"),
            format!(r#"{{"last_check_at":{},"last_result":"Up to date","available":"","error":""}}"#, now.saturating_sub(120)),
        )
        .unwrap();
        std::fs::write(
            dir.join(crate::cloud::CONFIG_FILE),
            r#"{"mode":"account","email":"ben@spader.us","key":"abcd","token":"hidden-token"}"#,
        )
        .unwrap();
        std::fs::write(
            dir.join(crate::cloud::STATUS_FILE),
            format!(
                r#"{{"enabled":true,"role":"active","account":"ben@spader.us","epoch":4,"active_name":"office","bucket":"PJDesk Cloud","updated_at":{now},"last_upload_at":{}}}"#,
                now.saturating_sub(60)
            ),
        )
        .unwrap();
    }
    let mut app = App::build(cc, dir, 21200, false);
    app.svc_override = Some((true, true));
    crate::ui_theme::apply(&cc.egui_ctx, "dark");
    if locked {
        app.preview_locked = true;
        app.access = crate::status_view::Access::Locked;
        app.can_write = false;
        app.fingerprint.clear();
        app.cloud_on = false;
        app.cloud = None;
        app.live = Some(Snapshot {
            version: env!("CARGO_PKG_VERSION").into(),
            port: 21200,
            service_running: true,
            fingerprint: "7f:3a:9c:11:b2:44:e0:8d:6c:15:aa:90:33:de:71:04:58:c6:2b:f9:17:ab:63:d0:4e:88:21:5c:b7:0a:66:d4".into(),
            fingerprint_known: true,
            update_auto: true,
            update_known: true,
            last_check_at: now.saturating_sub(120),
            last_result: "Up to date".into(),
            cloud_on: true,
            cloud_known: true,
            account: "ben@spader.us".into(),
            role: "active".into(),
            epoch: 4,
            active_name: "office".into(),
            bucket: "PJDesk Cloud".into(),
            cloud_updated_at: now,
            ..Snapshot::default()
        });
    } else {
        app.refresh();
    }
    app
}
