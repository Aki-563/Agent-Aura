//! Soak test: repeats a small, safe slice of the desktop conformance suite for hours and watches
//! the process for leaks (memory, handles, GDI and USER objects). One summary line at the end.
//!
//! `aura-cli soak --minutes 1440 --browser chrome` is the 24 hour run. It drives the real screen
//! (a separate Chrome, nothing of yours), so leave the PC alone; Esc stops it.

use std::time::{Duration, Instant};

use serde_json::json;

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Sample {
    pub working_set_mb: f64,
    pub handles: u32,
    pub gdi_objects: u32,
    pub user_objects: u32,
}

pub fn sample() -> Sample {
    use windows_sys::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetGuiResources, GetProcessHandleCount, GR_GDIOBJECTS, GR_USEROBJECTS};
    unsafe {
        let process = GetCurrentProcess();
        let mut counters: PROCESS_MEMORY_COUNTERS = std::mem::zeroed();
        counters.cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
        GetProcessMemoryInfo(process, &mut counters, counters.cb);
        let mut handles = 0u32;
        GetProcessHandleCount(process, &mut handles);
        Sample {
            working_set_mb: counters.WorkingSetSize as f64 / (1024.0 * 1024.0),
            handles,
            gdi_objects: GetGuiResources(process, GR_GDIOBJECTS),
            user_objects: GetGuiResources(process, GR_USEROBJECTS),
        }
    }
}

/// Whether the last samples show a leak compared with the first ones. Memory may grow by 50% plus
/// 150 MB (caches warm up); handles by 300; GDI and USER objects by 150 each.
pub fn leak_report(samples: &[Sample]) -> Option<String> {
    if samples.len() < 6 {
        return None;
    }
    let mean = |slice: &[Sample], f: fn(&Sample) -> f64| slice.iter().map(f).sum::<f64>() / slice.len() as f64;
    let (head, tail) = (&samples[1..4], &samples[samples.len() - 3..]);
    let mut problems = Vec::new();
    let (m0, m1) = (mean(head, |s| s.working_set_mb), mean(tail, |s| s.working_set_mb));
    if m1 > m0 * 1.5 + 150.0 {
        problems.push(format!("memory {m0:.0} -> {m1:.0} MB"));
    }
    let (h0, h1) = (mean(head, |s| f64::from(s.handles)), mean(tail, |s| f64::from(s.handles)));
    if h1 > h0 + 300.0 {
        problems.push(format!("handles {h0:.0} -> {h1:.0}"));
    }
    let (g0, g1) = (mean(head, |s| f64::from(s.gdi_objects)), mean(tail, |s| f64::from(s.gdi_objects)));
    if g1 > g0 + 150.0 {
        problems.push(format!("GDI objects {g0:.0} -> {g1:.0}"));
    }
    let (u0, u1) = (mean(head, |s| f64::from(s.user_objects)), mean(tail, |s| f64::from(s.user_objects)));
    if u1 > u0 + 150.0 {
        problems.push(format!("USER objects {u0:.0} -> {u1:.0}"));
    }
    (!problems.is_empty()).then(|| problems.join(", "))
}

/// Runs for `minutes`; returns the process exit code (0 = no failures and no leak).
pub fn run(minutes: u64) -> i32 {
    let deadline = Instant::now() + Duration::from_secs(minutes * 60);
    let mut samples = vec![sample()];
    let (mut cycles, mut failed_cases, mut total_cases) = (0u32, 0u32, 0u32);
    let mut first_failures: Vec<String> = Vec::new();
    let mut locked_seconds = 0u64;
    while Instant::now() < deadline && !crate::input::killswitch::is_killed() {
        // A locked PC (or a UAC prompt) cannot be driven: wait for it instead of counting the
        // refusals as failures, and say how long it was.
        if crate::system_state::secure_desktop_active() {
            std::thread::sleep(Duration::from_secs(5));
            locked_seconds += 5;
            continue;
        }
        cycles += 1;
        let failed_before = (failed_cases, first_failures.len());
        match crate::conformance::run(Some("group:click,type,key,scroll"), |_| {}) {
            Ok(results) => {
                total_cases += results.len() as u32;
                for r in results.iter().filter(|r| !r.passed) {
                    failed_cases += 1;
                    if first_failures.len() < 5 {
                        first_failures.push(format!("cycle {cycles}: {} / {}: {}", r.group, r.name, r.detail));
                    }
                }
            }
            Err(error) => {
                failed_cases += 1;
                if first_failures.len() < 5 {
                    first_failures.push(format!("cycle {cycles}: could not start: {error}"));
                }
                std::thread::sleep(Duration::from_secs(10));
            }
        }
        if crate::system_state::secure_desktop_active() {
            // The PC locked during this cycle: its failures say nothing about the harness.
            failed_cases = failed_before.0;
            first_failures.truncate(failed_before.1);
            cycles -= 1;
            continue;
        }
        let now = sample();
        crate::testlog::record("soak", "cycle", json!({"cycle": cycles, "working_set_mb": now.working_set_mb, "handles": now.handles, "gdi": now.gdi_objects, "user": now.user_objects, "failed_total": failed_cases}));
        samples.push(now);
    }
    let leak = leak_report(&samples);
    for line in &first_failures {
        println!("{line}");
    }
    match (&leak, failed_cases) {
        (None, 0) => {
            println!("SOAK PASSED: {cycles} cycles, {total_cases} cases, no failures, no leak ({:.0} MB, {} handles at the end; PC locked for {locked_seconds} s)", samples.last().map_or(0.0, |s| s.working_set_mb), samples.last().map_or(0, |s| s.handles));
            0
        }
        _ => {
            println!("SOAK FAILED: {cycles} cycles, {failed_cases} of {total_cases} cases failed{}", leak.map(|l| format!(", leak: {l}")).unwrap_or_default());
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(mb: f64, handles: u32, gdi: u32, user: u32) -> Sample {
        Sample { working_set_mb: mb, handles, gdi_objects: gdi, user_objects: user }
    }

    #[test]
    fn steady_use_is_not_a_leak_and_growth_is() {
        let steady: Vec<_> = (0..20).map(|i| s(200.0 + (i % 3) as f64 * 5.0, 500 + (i % 4) as u32 * 10, 100, 80)).collect();
        assert_eq!(leak_report(&steady), None);
        let leaking: Vec<_> = (0..20).map(|i| s(200.0, 500 + i * 40, 100 + i * 20, 80)).collect();
        let report = leak_report(&leaking).unwrap();
        assert!(report.contains("handles") && report.contains("GDI"), "{report}");
        let memory: Vec<_> = (0..20).map(|i| s(200.0 + i as f64 * 40.0, 500, 100, 80)).collect();
        assert!(leak_report(&memory).unwrap().contains("memory"));
        assert_eq!(leak_report(&steady[..3]), None, "too few samples to say");
    }

    #[test]
    fn this_process_can_be_sampled() {
        let now = sample();
        assert!(now.working_set_mb > 1.0 && now.handles > 0);
    }
}
