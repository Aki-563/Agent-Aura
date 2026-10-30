//! Test measurements (pixel errors, latencies, dropped events, success rates) go to log files,
//! not to reports (HARNESS_COMPLETION_PLAN.md §0, the brief's §9.3):
//! `%LOCALAPPDATA%\AgentAura\testlogs\<YYYY-MM-DD>\<group>.jsonl`, one JSON object per line.

use serde_json::{json, Value};
use std::io::Write;

pub fn dir() -> std::path::PathBuf {
    let base = std::env::var_os("LOCALAPPDATA").map(std::path::PathBuf::from).unwrap_or_else(std::env::temp_dir);
    let seconds = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    // Civil date from Unix days (UTC), no date crate needed.
    let days = (seconds / 86_400) as i64;
    let (year, month, day) = civil_from_days(days);
    base.join("AgentAura").join("testlogs").join(format!("{year:04}-{month:02}-{day:02}"))
}

fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + i64::from(month <= 2), month, day)
}

/// Appends one measurement. Never fails the caller (a test log is not worth a crash).
pub fn record(group: &str, name: &str, data: Value) {
    let dir = dir();
    let _ = std::fs::create_dir_all(&dir);
    let at = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
    let line = json!({"at": at, "case": name, "data": data});
    if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(dir.join(format!("{group}.jsonl"))) {
        let _ = writeln!(file, "{line}");
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn civil_dates_are_right() {
        assert_eq!(super::civil_from_days(0), (1970, 1, 1));
        assert_eq!(super::civil_from_days(20_756), (2026, 10, 30));
        assert_eq!(super::civil_from_days(11_016), (2000, 2, 29));
    }
}
