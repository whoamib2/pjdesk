//! Inbound Windows Firewall rules for the server's TCP and UDP port.
//!
//! The elevated installer and the service (LocalSystem) call [`ensure`]. It
//! checks each rule and adds or re-enables any that is missing, disabled or
//! different. The unelevated window never changes rules; it only reads the
//! last result from `firewall-status.txt` in the data folder.

use std::path::Path;

/// Program-scoped rules, then plain port rules (for a moved or renamed exe).
pub const RULES: [(&str, &str, bool); 4] = [
    ("PJDesk Server TCP", "TCP", true),
    ("PJDesk Server UDP", "UDP", true),
    ("PJDesk Server Port TCP", "TCP", false),
    ("PJDesk Server Port UDP", "UDP", false),
];

/// Names removed on uninstall. "PJDesk Server" is the older program-only rule.
pub const ALL_NAMES: [&str; 5] =
    ["PJDesk Server", "PJDesk Server TCP", "PJDesk Server UDP", "PJDesk Server Port TCP", "PJDesk Server Port UDP"];

pub const STATUS_FILE: &str = "firewall-status.txt";

/// The netsh arguments that add one rule.
pub fn add_args(name: &str, proto: &str, port: u16, program: Option<&Path>) -> String {
    let mut args = format!(
        "advfirewall firewall add rule name=\"{name}\" dir=in action=allow protocol={proto} localport={port} profile=any enable=yes"
    );
    if let Some(exe) = program {
        args.push_str(&format!(" program=\"{}\"", exe.display()));
    }
    args
}

/// What `netsh advfirewall firewall show rule name=... verbose` says about one rule.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Shown {
    pub enabled: bool,
    pub inbound: bool,
    pub allow: bool,
    pub all_profiles: bool,
    pub protocol: String,
    pub local_port: String,
    pub program: String,
}

/// Parses the first rule block of netsh's English output. `None` when no rule matched.
pub fn parse_show(text: &str) -> Option<Shown> {
    let mut out = Shown::default();
    let mut seen = false;
    for line in text.lines() {
        let Some((key, value)) = line.split_once(':') else { continue };
        let key = key.trim().to_ascii_lowercase();
        let value = value.trim();
        match key.as_str() {
            "rule name" => {
                if seen {
                    break;
                }
                seen = true;
            }
            "enabled" => out.enabled = value.eq_ignore_ascii_case("yes"),
            "direction" => out.inbound = value.eq_ignore_ascii_case("in"),
            "action" => out.allow = value.eq_ignore_ascii_case("allow"),
            "profiles" => {
                let v = value.to_ascii_lowercase();
                out.all_profiles = v == "any" || (v.contains("domain") && v.contains("private") && v.contains("public"));
            }
            "protocol" => out.protocol = value.to_ascii_uppercase(),
            "localport" => out.local_port = value.to_string(),
            // "Program" values contain "C:", so keep everything after the first colon.
            "program" => out.program = value.to_string(),
            _ => {}
        }
    }
    seen.then_some(out)
}

/// Whether a shown rule is the one we want.
pub fn rule_ok(shown: &Shown, proto: &str, port: u16, program: Option<&Path>) -> bool {
    let program_ok = match program {
        Some(exe) => shown.program.eq_ignore_ascii_case(&exe.display().to_string()),
        None => shown.program.is_empty() || shown.program.eq_ignore_ascii_case("any"),
    };
    shown.enabled
        && shown.inbound
        && shown.allow
        && shown.all_profiles
        && shown.protocol == proto
        && shown.local_port == port.to_string()
        && program_ok
}

/// The result of one check, for the log and the server window.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Report {
    pub ok: Vec<String>,
    pub repaired: Vec<String>,
    pub failed: Vec<String>,
}

impl Report {
    pub fn summary(&self, port: u16) -> String {
        if !self.failed.is_empty() {
            format!(
                "Firewall: could not set {} (TCP/UDP {port}). Run the installer again as administrator.",
                self.failed.join(", ")
            )
        } else if !self.repaired.is_empty() {
            format!("Firewall: repaired {} (TCP/UDP {port} allowed, all profiles)", self.repaired.join(", "))
        } else {
            format!("Firewall: TCP and UDP {port} allowed (all profiles)")
        }
    }
}

/// Reads the last check written by the service or installer.
pub fn last_status(data_dir: &Path) -> Option<String> {
    std::fs::read_to_string(data_dir.join(STATUS_FILE)).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

#[cfg(windows)]
fn netsh(args: &str) -> Option<(bool, String)> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let out = std::process::Command::new("netsh").raw_arg(args).creation_flags(CREATE_NO_WINDOW).output().ok()?;
    Some((out.status.success(), String::from_utf8_lossy(&out.stdout).into_owned()))
}

/// Checks every rule and fixes any that is missing or wrong. Must run elevated.
#[cfg(windows)]
pub fn ensure(exe: &Path, port: u16, data_dir: Option<&Path>) -> Report {
    let mut report = Report::default();
    for (name, proto, scoped) in RULES {
        let program = scoped.then_some(exe);
        let shown = netsh(&format!("advfirewall firewall show rule name=\"{name}\" verbose"))
            .filter(|(ok, _)| *ok)
            .and_then(|(_, text)| parse_show(&text));
        if shown.as_ref().is_some_and(|s| rule_ok(s, proto, port, program)) {
            report.ok.push(name.to_string());
            continue;
        }
        let _ = netsh(&format!("advfirewall firewall delete rule name=\"{name}\""));
        match netsh(&add_args(name, proto, port, program)) {
            Some((true, _)) => report.repaired.push(name.to_string()),
            _ => report.failed.push(name.to_string()),
        }
    }
    let summary = report.summary(port);
    if report.failed.is_empty() {
        tracing::info!("{summary}");
    } else {
        tracing::warn!("{summary}");
    }
    if let Some(dir) = data_dir {
        let _ = std::fs::write(dir.join(STATUS_FILE), format!("{summary}\n"));
    }
    report
}

#[cfg(windows)]
pub fn remove() {
    for name in ALL_NAMES {
        let _ = netsh(&format!("advfirewall firewall delete rule name=\"{name}\""));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\r\nRule Name:                            PJDesk Server UDP\r\n----------------------------------------------------------------------\r\nEnabled:                              Yes\r\nDirection:                            In\r\nProfiles:                             Domain,Private,Public\r\nGrouping:                             \r\nLocalIP:                              Any\r\nRemoteIP:                             Any\r\nProtocol:                             UDP\r\nLocalPort:                            21200\r\nRemotePort:                           Any\r\nEdge traversal:                       No\r\nProgram:                              C:\\Program Files\\PJDesk Server\\pjdesk-server.exe\r\nInterfaceTypes:                       Any\r\nSecurity:                             NotRequired\r\nRule source:                          Local Setting\r\nAction:                               Allow\r\nOk.\r\n";

    #[test]
    fn a_matching_rule_is_kept_and_a_disabled_one_is_repaired() {
        let exe = Path::new(r"C:\Program Files\PJDesk Server\pjdesk-server.exe");
        let shown = parse_show(SAMPLE).unwrap();
        assert_eq!(shown.program, exe.display().to_string());
        assert!(rule_ok(&shown, "UDP", 21200, Some(exe)));
        assert!(!rule_ok(&shown, "TCP", 21200, Some(exe)), "wrong protocol");
        assert!(!rule_ok(&shown, "UDP", 21201, Some(exe)), "wrong port");
        assert!(!rule_ok(&shown, "UDP", 21200, Some(Path::new(r"C:\old\pjdesk-server.exe"))), "moved program");
        let disabled = parse_show(&SAMPLE.replace("Yes", "No")).unwrap();
        assert!(!rule_ok(&disabled, "UDP", 21200, Some(exe)));
        let private_only = parse_show(&SAMPLE.replace("Domain,Private,Public", "Private")).unwrap();
        assert!(!rule_ok(&private_only, "UDP", 21200, Some(exe)), "every profile, so a Public network still works");
        assert!(parse_show("No rules match the specified criteria.\r\n").is_none());
    }

    #[test]
    fn port_rules_have_no_program_and_scoped_rules_name_the_exe() {
        let exe = Path::new(r"C:\Program Files\PJDesk Server\pjdesk-server.exe");
        let scoped = add_args("PJDesk Server UDP", "UDP", 21200, Some(exe));
        assert!(scoped.contains("protocol=UDP localport=21200 profile=any enable=yes"));
        assert!(scoped.ends_with("program=\"C:\\Program Files\\PJDesk Server\\pjdesk-server.exe\""));
        let port = add_args("PJDesk Server Port TCP", "TCP", 21200, None);
        assert!(!port.contains("program="));
        let shown = parse_show(&SAMPLE.replace("Program:                              C:\\Program Files\\PJDesk Server\\pjdesk-server.exe\r\n", "")).unwrap();
        assert!(rule_ok(&shown, "UDP", 21200, None));
        assert_eq!(RULES.iter().filter(|r| r.2).count(), 2);
        assert!(ALL_NAMES.contains(&"PJDesk Server"), "the old program rule is removed on uninstall");
    }

    #[test]
    fn the_summary_says_what_happened() {
        let mut r = Report::default();
        assert_eq!(r.summary(21200), "Firewall: TCP and UDP 21200 allowed (all profiles)");
        r.repaired.push("PJDesk Server UDP".into());
        assert!(r.summary(21200).starts_with("Firewall: repaired PJDesk Server UDP"));
        r.failed.push("PJDesk Server TCP".into());
        assert!(r.summary(21200).contains("could not set PJDesk Server TCP"));
    }
}
