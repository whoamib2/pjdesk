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
    for (name, locked) in [("server-dark", false), ("server-unelevated", true), ("server-devices", false)] {
        let mut harness = egui_kittest::Harness::builder()
            .with_size(egui::vec2(960.0, 1040.0))
            .with_pixels_per_point(1.0)
            .with_theme(egui::Theme::Dark)
            .wgpu()
            .build_eframe(move |cc| preview(cc, locked, name == "server-devices"));
        harness.step();
        let image = harness.render().unwrap_or_else(|e| panic!("render {name}: {e}"));
        let path = dir.join(format!("{name}.png"));
        image.save(&path).unwrap_or_else(|e| panic!("save {}: {e}", path.display()));
        eprintln!("wrote {}", path.display());
    }
    for (name, theme) in [("server-network-dark", egui::Theme::Dark), ("server-network-light", egui::Theme::Light)] {
        let mut harness = egui_kittest::Harness::builder()
            .with_size(egui::vec2(960.0, 720.0))
            .with_pixels_per_point(1.0)
            .with_theme(theme)
            .wgpu()
            .build_eframe(move |cc| {
                let mut app = preview(cc, false, false);
                app.preview_network = true;
                let theme_name = match theme {
                    egui::Theme::Light => "light",
                    egui::Theme::Dark => "dark",
                };
                crate::ui_theme::apply(&cc.egui_ctx, theme_name);
                std::fs::write(
                    app.data_dir.join(crate::access::FILE_NAME),
                    r#"{"require_accounts":true,"public_key":"00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff","fingerprint":"cd","allowed_accounts":["ben@spader.us","ada@example.com"],"revoked_devices":[]}"#,
                )
                .unwrap();
                std::fs::write(
                    app.data_dir.join("devices.json"),
                    r#"{"devices":{"123456789":{"secret_sha256":"hidden","hostname":"Office","app_version":"0.5.1","first_seen":1,"last_seen":2,"signed_in":true},"555666777":{"secret_sha256":"hidden","hostname":"Laptop","app_version":"0.5.2","first_seen":1,"last_seen":2,"signed_in":false},"902488771":{"secret_sha256":"hidden","hostname":"This PC","app_version":"0.6.0","first_seen":1,"last_seen":2,"signed_in":true}}}"#,
                )
                .unwrap();
                app.refresh();
                app
            });
        harness.step();
        let image = harness.render().unwrap_or_else(|e| panic!("render {name}: {e}"));
        let path = dir.join(format!("{name}.png"));
        image.save(&path).unwrap_or_else(|e| panic!("save {}: {e}", path.display()));
        eprintln!("wrote {}", path.display());
    }
    for (name, theme) in [("server-netkey-dark", egui::Theme::Dark), ("server-netkey-light", egui::Theme::Light)] {
        let mut harness = egui_kittest::Harness::builder()
            .with_size(egui::vec2(960.0, 720.0))
            .with_pixels_per_point(1.0)
            .with_theme(theme)
            .wgpu()
            .build_eframe(move |cc| {
                let mut app = preview(cc, false, false);
                app.preview_netkey = true;
                let theme_name = match theme {
                    egui::Theme::Light => "light",
                    egui::Theme::Dark => "dark",
                };
                crate::ui_theme::apply(&cc.egui_ctx, theme_name);
                let secret = [0x11u8; 32];
                let contents = pjdesk_proto::netkey::Contents {
                    network_id: "net_office",
                    active: "office.example:21200",
                    active_fingerprint: "abcd",
                    standby: "cabin.example:21200",
                    standby_fingerprint: "00112233",
                };
                app.nk_blob = pjdesk_proto::netkey::seal_embedded(&contents, &secret).unwrap_or_default();
                app.nk_embedded = true;
                app.nk_open = true;
                app.nk_loaded = true;
                app.nk_invites = vec![crate::cloud::NetworkInvite {
                    id: "ink_office".into(),
                    mode: "passphrase".into(),
                    bound_email: String::new(),
                    max_uses: Some(5),
                    uses: 1,
                    expires_at: None,
                    revoked_at: None,
                    history: vec![crate::cloud::InviteUse {
                        device_name: "Laptop".into(),
                        email: "ben@spader.us".into(),
                        used_at: 1_728_100_000,
                    }],
                }];
                app
            });
        harness.step();
        let image = harness.render().unwrap_or_else(|e| panic!("render {name}: {e}"));
        let path = dir.join(format!("{name}.png"));
        image.save(&path).unwrap_or_else(|e| panic!("save {}: {e}", path.display()));
        eprintln!("wrote {}", path.display());
    }
}

fn preview(cc: &eframe::CreationContext<'_>, locked: bool, devices_only: bool) -> App {
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
                r#"{{"enabled":true,"role":"active","account":"ben@spader.us","epoch":4,"active_name":"office","bucket":"PJDesk Cloud","updated_at":{now},"last_upload_at":{},"peer_name":"cabin","peer_version":"0.5.1","peer_role":"standby"}}"#,
                now.saturating_sub(60)
            ),
        )
        .unwrap();
        std::fs::write(
            dir.join("devices.json"),
            r#"{"devices":{"123456789":{"secret_sha256":"hidden","hostname":"Office","app_version":"0.5.1","first_seen":1,"last_seen":2},"555666777":{"secret_sha256":"hidden","hostname":"Laptop","app_version":"","first_seen":1,"last_seen":2},"902488771":{"secret_sha256":"hidden","hostname":"This PC","app_version":"0.5.2","first_seen":1,"last_seen":2}}}"#,
        )
        .unwrap();
        let online = format!(
            r#"[{{"id":"123456789","hostname":"Office","version":"0.5.1"}},{{"id":"902488771","hostname":"This PC","version":"0.5.2"}}]"#
        );
        std::fs::write(dir.join("online.json"), online).unwrap();
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
            peer_name: "cabin".into(),
            peer_version: "0.5.1".into(),
            peer_role: "standby".into(),
            devices: vec![
                crate::status_view::ListedDevice {
                    id: "123456789".into(),
                    hostname: "Office".into(),
                    version: "0.5.1".into(),
                    online: true,
                    signed_in: true,
                },
                crate::status_view::ListedDevice {
                    id: "444555666".into(),
                    hostname: "Old laptop".into(),
                    version: String::new(),
                    online: false,
                    signed_in: false,
                },
            ],
            ..Snapshot::default()
        });
    } else {
        app.refresh();
        app.latest_release = Some("0.5.2".into());
    }
    if devices_only {
        app.preview_devices = true;
        app.latest_release = Some("0.5.2".into());
    }
    app
}
