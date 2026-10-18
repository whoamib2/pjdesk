//! Text for the server tray tooltip. Pure so it can be tested without Windows.

/// What the tooltip is built from. The window fills this in from the service
/// and the files the service writes.
pub struct Tip<'a> {
    pub installed: bool,
    pub running: bool,
    pub hosts: usize,
    pub role: &'a str,
    pub epoch: u64,
    pub active: &'a str,
    pub available: &'a str,
    pub last_result: &'a str,
}

pub fn format(tip: &Tip<'_>) -> String {
    let service = if !tip.installed {
        "Service: not installed".to_string()
    } else if tip.running {
        "Service: running".to_string()
    } else {
        "Service: stopped".to_string()
    };
    let hosts = match tip.hosts {
        0 => "No computers connected".to_string(),
        1 => "1 computer connected".to_string(),
        n => format!("{n} computers connected"),
    };
    let role = if tip.role.is_empty() { "off" } else { tip.role };
    let active = if tip.active.is_empty() { "none" } else { tip.active };
    let mut lines = vec![
        "PJDesk Server".to_string(),
        service,
        hosts,
        format!("Cloud sync: {role} · generation {} · active: {active}", tip.epoch),
    ];
    if !tip.available.is_empty() {
        lines.push(format!("Update available: {}", tip.available));
    } else if !tip.last_result.is_empty() {
        lines.push(format!("Updates: {}", tip.last_result));
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tooltip_names_the_service_hosts_role_and_update() {
        let text = format(&Tip {
            installed: true,
            running: true,
            hosts: 2,
            role: "active",
            epoch: 4,
            active: "cabin",
            available: "0.4.1",
            last_result: "update available",
        });
        assert!(text.contains("Service: running"));
        assert!(text.contains("2 computers connected"));
        assert!(text.contains("Cloud sync: active · generation 4 · active: cabin"));
        assert!(text.contains("Update available: 0.4.1"));

        let one = format(&Tip {
            installed: false,
            running: false,
            hosts: 1,
            role: "",
            epoch: 0,
            active: "",
            available: "",
            last_result: "up to date",
        });
        assert!(text.starts_with("PJDesk Server"));
        assert!(one.contains("1 computer connected"));
        assert!(one.contains("Service: not installed"));
        assert!(one.contains("Cloud sync: off"));
        assert!(one.contains("Updates: up to date"));
    }
}
