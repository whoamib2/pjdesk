//! Last advertised server catalog, shared by the window and the service.
//!
//! Routes used to live only in whichever config last received `ServerMsg::Routes`.
//! The service's copy was often empty, so a computer with no live registration
//! dialed the saved hostname and nothing else. This file is the catalog from
//! the last registration, read before DNS.

use std::path::{Path, PathBuf};

use pjdesk_proto::routes::ServerRoutes;

use crate::config::Config;

pub fn path() -> PathBuf {
    crate::syspaths::program_data_dir().join("server-routes.json")
}

pub fn load() -> ServerRoutes {
    load_file(&path())
}

pub fn store(routes: &ServerRoutes) {
    store_file(&path(), routes);
}

/// Fills empty halves from the on-disk catalog. A catalog already in memory
/// keeps the halves it has. A file that only remembers which address answered
/// is still applied.
pub fn absorb(config: &mut Config) {
    let cached = load();
    if cached.is_empty() && cached.preferred.trim().is_empty() && cached.misses.is_empty() && cached.server_version.is_empty()
    {
        return;
    }
    let merged = pjdesk_proto::routes::keep_other_server(&cached, &config.network_routes);
    if merged != config.network_routes {
        config.network_routes = merged;
    }
}

pub fn note_answer(config: &mut Config, addr: &str) {
    pjdesk_proto::routes::note_answer(&mut config.network_routes, addr);
    store(&config.network_routes);
}

pub fn note_timeout(config: &mut Config, addr: &str) {
    pjdesk_proto::routes::note_timeout(&mut config.network_routes, addr);
    store(&config.network_routes);
}

pub fn note_version(config: &mut Config, version: &str) {
    let before = config.network_routes.server_version.clone();
    pjdesk_proto::routes::note_server_version(&mut config.network_routes, version);
    if config.network_routes.server_version != before {
        store(&config.network_routes);
    }
}

fn load_file(path: &Path) -> ServerRoutes {
    let Ok(bytes) = std::fs::read(path) else {
        return ServerRoutes::default();
    };
    serde_json::from_slice(&bytes).unwrap_or_default()
}

fn store_file(path: &Path, routes: &ServerRoutes) {
    if routes.is_empty() && routes.preferred.trim().is_empty() && routes.misses.is_empty() && routes.server_version.is_empty() {
        return;
    }
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let Ok(bytes) = serde_json::to_vec_pretty(routes) else {
        return;
    };
    let tmp = path.with_extension("json.tmp");
    if std::fs::write(&tmp, bytes).is_ok() {
        let _ = std::fs::rename(&tmp, path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_catalog_round_trips_without_the_live_config() {
        let dir = std::env::temp_dir().join(format!("pjdesk-routes-{}-{}", std::process::id(), line!()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("server-routes.json");
        let routes = ServerRoutes {
            primary_lan: vec!["10.1.1.8:21200".into()],
            primary_public: vec!["198.51.100.8:21200".into()],
            ..ServerRoutes::default()
        };
        store_file(&path, &routes);
        let loaded = load_file(&path);
        assert_eq!(loaded, routes);
        store_file(&path, &ServerRoutes::default());
        assert_eq!(load_file(&path), routes, "an empty write does not erase the saved catalog");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
