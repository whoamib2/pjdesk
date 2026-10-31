//! Exact local date and time with a time-zone label, e.g.
//! "Oct 31, 2026 9:58:12 AM CDT". Shared by the client and the server.

use chrono::{Local, TimeZone};

/// "Oct 31, 2026 9:58:12 AM CDT" in this computer's time zone, or "never" for 0.
pub fn exact_local(unix: u64) -> String {
    if unix == 0 {
        return "never".into();
    }
    match Local.timestamp_opt(unix as i64, 0).single() {
        Some(t) => {
            let off = t.offset().local_minus_utc();
            format!("{} {}", t.format("%b %-d, %Y %-I:%M:%S %p"), zone_label(off))
        }
        None => "unknown".into(),
    }
}

/// "Last seen Oct 31, 2026 9:58:12 AM CDT". An online computer reads
/// "Online · last report …" with its latest heartbeat.
pub fn seen_line(online: bool, unix: u64) -> String {
    match (online, unix) {
        (true, 0) => "Online · last report time not known yet".into(),
        (true, t) => format!("Online · last report {}", exact_local(t)),
        (false, 0) => "Last seen: never reported".into(),
        (false, t) => format!("Last seen {}", exact_local(t)),
    }
}

/// "Central Daylight Time" -> "CDT". Names already short are kept.
#[allow(dead_code)]
pub fn initials(name: &str) -> String {
    let words: Vec<&str> = name.split_whitespace().collect();
    if words.len() <= 1 {
        return name.trim().to_string();
    }
    words.iter().filter_map(|w| w.chars().next()).filter(|c| c.is_alphabetic()).map(|c| c.to_ascii_uppercase()).collect()
}

/// "UTC-5", "UTC+5:30", "UTC".
pub fn utc_label(off_secs: i32) -> String {
    if off_secs == 0 {
        return "UTC".into();
    }
    let sign = if off_secs < 0 { '-' } else { '+' };
    let a = off_secs.unsigned_abs();
    let (h, m) = (a / 3600, (a % 3600) / 60);
    if m == 0 { format!("UTC{sign}{h}") } else { format!("UTC{sign}{h}:{m:02}") }
}

#[cfg(windows)]
fn zone_label(off_secs: i32) -> String {
    use windows::Win32::System::Time::{GetTimeZoneInformation, TIME_ZONE_INFORMATION};
    let mut tz = TIME_ZONE_INFORMATION::default();
    unsafe {
        let _ = GetTimeZoneInformation(&mut tz);
    }
    let name = |w: &[u16]| String::from_utf16_lossy(&w[..w.iter().position(|c| *c == 0).unwrap_or(w.len())]);
    let standard = -(tz.Bias + tz.StandardBias) * 60;
    let daylight = -(tz.Bias + tz.DaylightBias) * 60;
    let label = if off_secs == daylight && tz.DaylightBias != 0 {
        initials(&name(&tz.DaylightName))
    } else if off_secs == standard {
        initials(&name(&tz.StandardName))
    } else {
        String::new()
    };
    if label.is_empty() || label.len() > 6 { utc_label(off_secs) } else { label }
}

#[cfg(not(windows))]
fn zone_label(off_secs: i32) -> String {
    utc_label(off_secs)
}

#[cfg(test)]
mod localtime_tests {
    use super::*;

    #[test]
    fn labels_and_lines() {
        assert_eq!(initials("Central Daylight Time"), "CDT");
        assert_eq!(utc_label(-5 * 3600), "UTC-5");
        assert_eq!(utc_label(19800), "UTC+5:30");
        assert_eq!(utc_label(0), "UTC");
        let line = seen_line(false, 1_793_458_692);
        assert!(line.starts_with("Last seen "), "{line}");
        assert!(line.contains(", 2026 ") && (line.contains(" AM ") || line.contains(" PM ")), "{line}");
        assert!(seen_line(true, 1_793_458_692).starts_with("Online · last report "));
        assert_eq!(seen_line(false, 0), "Last seen: never reported");
    }
}
