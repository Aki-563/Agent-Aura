//! Scheduled runs (HARNESS_COMPLETION_PLAN.md §H7): "do this every 5 minutes", "check this at
//! 09:00 and 17:30". The logic is pure functions over a clock value so it can be tested with a
//! fake clock; `run_forever` is the one async piece, and it **sleeps until the next due time**
//! (no polling, no screenshots, no model calls while idle).
//!
//! * Times never drift: the n-th interval run is always `anchor + n * period`, not "last run + period".
//! * Overlap policy when a run is still going at the next due time: skip, queue one, or cancel it.
//! * Missed runs (computer asleep, Aura closed): run once on wake, or skip, by policy.
//! * Schedules persist in `schedules.json` under the data root, so they survive crashes and reboots.
//! * Each schedule carries state between runs (what the last run saw) and can be "only act if the
//!   screen changed since the last run", which skips the run before any model call.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum When {
    /// Every `seconds` seconds, counted from the schedule's anchor time.
    Every { seconds: u64 },
    /// Every day at these local times, written "HH:MM".
    DailyAt { times: Vec<String> },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum Overlap {
    /// The next run is skipped when the previous one is still going.
    #[default]
    Skip,
    /// One run waits and starts as soon as the previous one ends.
    QueueOne,
    /// The running one is stopped and the new one starts.
    CancelPrevious,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum Missed {
    /// Runs that were missed while the computer slept or Aura was closed run once, now.
    #[default]
    RunOnce,
    Skip,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Schedule {
    pub id: String,
    pub task: String,
    pub when: When,
    #[serde(default)]
    pub overlap: Overlap,
    #[serde(default)]
    pub missed: Missed,
    /// Skip a run when the screen is the same as at the end of the last run.
    #[serde(default)]
    pub only_if_changed: bool,
    #[serde(default = "yes")]
    pub enabled: bool,
    /// Unix milliseconds the schedule was created; interval runs are counted from here.
    pub anchor_ms: u64,
    pub next_run_ms: u64,
    #[serde(default)]
    pub last_run_ms: Option<u64>,
    /// What the last run reported; shown to the next run as "LAST RUN".
    #[serde(default)]
    pub last_summary: String,
    #[serde(default)]
    pub last_hash: Option<u64>,
}

fn yes() -> bool {
    true
}

const DAY_MS: i64 = 86_400_000;
/// A run due less than this long ago is simply late, not missed.
pub const MISSED_GRACE_MS: u64 = 60_000;

/// "HH:MM" -> minutes after midnight.
pub fn parse_time(text: &str) -> Option<u32> {
    let (h, m) = text.trim().split_once(':')?;
    let (h, m): (u32, u32) = (h.trim().parse().ok()?, m.trim().parse().ok()?);
    (h < 24 && m < 60).then_some(h * 60 + m)
}

/// "5m", "30s", "2h", "90" (minutes) -> seconds.
pub fn parse_interval(text: &str) -> Option<u64> {
    let text = text.trim().to_lowercase();
    let (number, unit) = match text.chars().last()? {
        's' => (&text[..text.len() - 1], 1),
        'm' => (&text[..text.len() - 1], 60),
        'h' => (&text[..text.len() - 1], 3600),
        c if c.is_ascii_digit() => (text.as_str(), 60),
        _ => return None,
    };
    let seconds = number.trim().parse::<u64>().ok()?.checked_mul(unit)?;
    (seconds >= 1).then_some(seconds)
}

/// The first run time strictly after `after_ms`. `tz_offset_min` is local time minus UTC.
pub fn next_run_after(when: &When, anchor_ms: u64, after_ms: u64, tz_offset_min: i32) -> Option<u64> {
    match when {
        When::Every { seconds } => {
            let period = seconds.checked_mul(1000).filter(|p| *p > 0)?;
            let k = if after_ms < anchor_ms { 1 } else { (after_ms - anchor_ms) / period + 1 };
            Some(anchor_ms + k * period)
        }
        When::DailyAt { times } => {
            let mut minutes: Vec<u32> = times.iter().filter_map(|t| parse_time(t)).collect();
            if minutes.is_empty() {
                return None;
            }
            minutes.sort_unstable();
            let offset_ms = tz_offset_min as i64 * 60_000;
            let local_now = after_ms as i64 + offset_ms;
            let day_start = local_now - local_now.rem_euclid(DAY_MS);
            for day in 0..3 {
                for minute in &minutes {
                    let utc = day_start + day * DAY_MS + *minute as i64 * 60_000 - offset_ms;
                    if utc > after_ms as i64 {
                        return Some(utc as u64);
                    }
                }
            }
            None
        }
    }
}

/// What to do when a schedule is due.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Start {
    Now,
    Skip,
    Queue,
    CancelThenStart,
}

pub fn decide_start(overlap: Overlap, a_run_is_active: bool, one_already_queued: bool) -> Start {
    if !a_run_is_active {
        return Start::Now;
    }
    match overlap {
        Overlap::Skip => Start::Skip,
        Overlap::QueueOne if one_already_queued => Start::Skip,
        Overlap::QueueOne => Start::Queue,
        Overlap::CancelPrevious => Start::CancelThenStart,
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Store {
    pub schedules: Vec<Schedule>,
}

pub fn store_path(data_root: &std::path::Path) -> std::path::PathBuf {
    data_root.join("schedules.json")
}

impl Store {
    pub fn load(data_root: &std::path::Path) -> Self {
        std::fs::read_to_string(store_path(data_root)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
    }

    pub fn save(&self, data_root: &std::path::Path) -> Result<(), String> {
        let text = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        std::fs::create_dir_all(data_root).map_err(|e| e.to_string())?;
        let target = store_path(data_root);
        let temp = target.with_extension("json.tmp");
        std::fs::write(&temp, text).map_err(|e| e.to_string())?;
        std::fs::rename(&temp, &target).map_err(|e| e.to_string())
    }

    pub fn add(&mut self, task: &str, when: When, overlap: Overlap, missed: Missed, only_if_changed: bool, now_ms: u64, tz: i32) -> Result<String, String> {
        let next = next_run_after(&when, now_ms, now_ms, tz).ok_or("that schedule never runs (check the interval or the HH:MM times)")?;
        let id = format!("s{}", self.schedules.iter().filter_map(|s| s.id.trim_start_matches('s').parse::<u32>().ok()).max().unwrap_or(0) + 1);
        self.schedules.push(Schedule {
            id: id.clone(), task: task.to_owned(), when, overlap, missed, only_if_changed, enabled: true, anchor_ms: now_ms, next_run_ms: next,
            last_run_ms: None, last_summary: String::new(), last_hash: None,
        });
        Ok(id)
    }

    pub fn remove(&mut self, id: &str) -> bool {
        let before = self.schedules.len();
        self.schedules.retain(|s| s.id != id);
        self.schedules.len() != before
    }

    /// Earliest next run among enabled schedules.
    pub fn earliest(&self) -> Option<u64> {
        self.schedules.iter().filter(|s| s.enabled).map(|s| s.next_run_ms).min()
    }

    /// Schedules due at `now_ms`, applying the missed-run policy: a schedule that is more than the
    /// grace period late was missed (the computer slept, Aura was closed). Its time is advanced
    /// for good; it runs once now only when its policy says so. Every returned schedule has its
    /// next time moved forward already, so a run that takes long cannot be started twice.
    pub fn take_due(&mut self, now_ms: u64, tz: i32) -> Vec<Schedule> {
        let mut due = Vec::new();
        for s in self.schedules.iter_mut().filter(|s| s.enabled && s.next_run_ms <= now_ms) {
            let missed = now_ms - s.next_run_ms > MISSED_GRACE_MS;
            let snapshot = s.clone();
            s.next_run_ms = next_run_after(&s.when, s.anchor_ms, now_ms, tz).unwrap_or(u64::MAX);
            if !(missed && s.missed == Missed::Skip) {
                due.push(snapshot);
            }
        }
        due
    }

    pub fn record_result(&mut self, id: &str, now_ms: u64, summary: &str, hash: Option<u64>) {
        if let Some(s) = self.schedules.iter_mut().find(|s| s.id == id) {
            s.last_run_ms = Some(now_ms);
            s.last_summary = summary.chars().take(400).collect();
            if hash.is_some() {
                s.last_hash = hash;
            }
        }
    }
}

pub fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// Local time minus UTC, in minutes (Windows time-zone settings, including daylight saving now).
pub fn local_offset_minutes() -> i32 {
    use windows_sys::Win32::System::Time::{GetTimeZoneInformation, TIME_ZONE_INFORMATION};
    unsafe {
        let mut info: TIME_ZONE_INFORMATION = std::mem::zeroed();
        let state = GetTimeZoneInformation(&mut info);
        let extra = if state == 2 { info.DaylightBias } else { info.StandardBias };
        -(info.Bias + extra)
    }
}

/// The task text for a run: the task, plus what the previous run reported.
pub fn task_with_state(schedule: &Schedule) -> String {
    if schedule.last_summary.is_empty() {
        schedule.task.clone()
    } else {
        format!("{}\n\nLAST RUN (for comparison, may be out of date): {}", schedule.task, schedule.last_summary)
    }
}

/// How a run ended, for the schedule's memory.
pub struct RunReport {
    pub summary: String,
    /// Hash of the screen at the end of the run (for "only if changed").
    pub screen_hash: Option<u64>,
}

pub type Runner = Arc<dyn Fn(Schedule) -> std::pin::Pin<Box<dyn std::future::Future<Output = RunReport> + Send>> + Send + Sync>;

/// Whether the screen changed since the schedule's last run. `current_hash` is computed by the
/// caller (cheap, local); None means "cannot tell", which counts as changed.
pub fn should_run(schedule: &Schedule, current_hash: Option<u64>) -> bool {
    !schedule.only_if_changed || schedule.last_hash.is_none() || current_hash.is_none() || schedule.last_hash != current_hash
}

/// Serves the schedules until the process ends. Between runs it only sleeps: it wakes at the next
/// due time (or at most once an hour to re-read the clock), starts runs through `runner`, and
/// applies the overlap policy. `changed` lets the caller wake it when schedules are edited.
pub async fn run_forever(data_root: std::path::PathBuf, store: Arc<Mutex<Store>>, changed: Arc<tokio::sync::Notify>, runner: Runner, current_hash: Arc<dyn Fn() -> Option<u64> + Send + Sync>) {
    let active: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    let queued: Arc<Mutex<Option<Schedule>>> = Arc::new(Mutex::new(None));
    let mut last_tick = now_ms();
    loop {
        let now = now_ms();
        let wait = store.lock().ok().and_then(|s| s.earliest()).map(|next| next.saturating_sub(now)).unwrap_or(3_600_000).min(3_600_000);
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_millis(wait)) => {}
            _ = changed.notified() => { continue; }
        }
        let now = now_ms();
        // A long gap between wake-ups means the computer slept; due times then count as missed.
        let _slept = now.saturating_sub(last_tick) > wait + 30_000;
        last_tick = now;
        let due = {
            let mut guard = match store.lock() {
                Ok(g) => g,
                Err(_) => continue,
            };
            let due = guard.take_due(now, local_offset_minutes());
            let _ = guard.save(&data_root);
            due
        };
        for schedule in due {
            let is_active = active.lock().map(|a| a.is_some()).unwrap_or(false);
            let is_queued = queued.lock().map(|q| q.is_some()).unwrap_or(false);
            match decide_start(schedule.overlap, is_active, is_queued) {
                Start::Skip => continue,
                Start::Queue => {
                    if let Ok(mut q) = queued.lock() {
                        *q = Some(schedule);
                    }
                    continue;
                }
                Start::CancelThenStart => {
                    crate::control::stop();
                    for _ in 0..100 {
                        if active.lock().map(|a| a.is_none()).unwrap_or(true) {
                            break;
                        }
                        tokio::time::sleep(Duration::from_millis(50)).await;
                    }
                }
                Start::Now => {}
            }
            spawn_run(schedule, Arc::clone(&store), data_root.clone(), Arc::clone(&runner), Arc::clone(&active), Arc::clone(&queued), Arc::clone(&current_hash));
        }
    }
}

fn spawn_run(schedule: Schedule, store: Arc<Mutex<Store>>, data_root: std::path::PathBuf, runner: Runner, active: Arc<Mutex<Option<String>>>, queued: Arc<Mutex<Option<Schedule>>>, current_hash: Arc<dyn Fn() -> Option<u64> + Send + Sync>) {
    if let Ok(mut a) = active.lock() {
        *a = Some(schedule.id.clone());
    }
    tokio::spawn(async move {
        let mut next = Some(schedule);
        while let Some(schedule) = next.take() {
            if should_run(&schedule, current_hash()) {
                let id = schedule.id.clone();
                let report = runner(schedule).await;
                if let Ok(mut s) = store.lock() {
                    s.record_result(&id, now_ms(), &report.summary, report.screen_hash);
                    let _ = s.save(&data_root);
                }
            }
            // A run that was queued while this one ran starts right away.
            next = queued.lock().ok().and_then(|mut q| q.take());
        }
        if let Ok(mut a) = active.lock() {
            *a = None;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIN: u64 = 60_000;
    // 2026-10-30 12:00:00 UTC
    const T0: u64 = 1_793_361_600_000;

    #[test]
    fn intervals_are_counted_from_the_anchor_and_never_drift_over_a_day() {
        let when = When::Every { seconds: 300 };
        let mut next = next_run_after(&when, T0, T0, 0).unwrap();
        assert_eq!(next, T0 + 5 * MIN);
        // Simulate a day where every run starts a few seconds late: the schedule stays on the grid.
        for n in 1..=288u64 {
            assert_eq!(next, T0 + n * 5 * MIN, "run {n}");
            let late_start = next + 3_700; // the run began 3.7 s after it was due
            next = next_run_after(&when, T0, late_start, 0).unwrap();
        }
        // After a long gap the next time is the next grid point, not "now + period".
        assert_eq!(next_run_after(&when, T0, T0 + 61 * MIN + 17, 0), Some(T0 + 65 * MIN));
    }

    #[test]
    fn daily_times_follow_the_local_clock_and_its_offset() {
        let when = When::DailyAt { times: vec!["17:30".into(), "09:00".into()] };
        // 12:00 UTC at UTC+5:30 is 17:30 local: next is tomorrow 09:00 local = 03:30 UTC.
        let next = next_run_after(&when, T0, T0, 330).unwrap();
        assert_eq!(next, T0 + 15 * 60 * MIN + 30 * MIN, "09:00 local the next day");
        // At UTC it is 12:00: the next is 17:30 the same day.
        assert_eq!(next_run_after(&when, T0, T0, 0), Some(T0 + 5 * 60 * MIN + 30 * MIN));
        // Negative offsets work too (UTC-8: 12:00 UTC is 04:00 local; next 09:00 local = 17:00 UTC).
        assert_eq!(next_run_after(&when, T0, T0, -480), Some(T0 + 5 * 60 * MIN));
        assert_eq!(next_run_after(&When::DailyAt { times: vec!["25:00".into()] }, T0, T0, 0), None);
    }

    #[test]
    fn intervals_and_times_parse() {
        assert_eq!((parse_interval("5m"), parse_interval("90"), parse_interval("30s"), parse_interval("2h"), parse_interval("x")), (Some(300), Some(5400), Some(30), Some(7200), None));
        assert_eq!((parse_time("09:05"), parse_time("24:00"), parse_time("9")), (Some(545), None, None));
    }

    #[test]
    fn overlap_policies_decide_skip_queue_or_cancel() {
        assert_eq!(decide_start(Overlap::Skip, false, false), Start::Now);
        assert_eq!(decide_start(Overlap::Skip, true, false), Start::Skip);
        assert_eq!(decide_start(Overlap::QueueOne, true, false), Start::Queue);
        assert_eq!(decide_start(Overlap::QueueOne, true, true), Start::Skip, "only one waits");
        assert_eq!(decide_start(Overlap::CancelPrevious, true, false), Start::CancelThenStart);
    }

    #[test]
    fn missed_runs_follow_the_policy_and_late_runs_are_just_late() {
        let mut store = Store::default();
        let a = store.add("run once", When::Every { seconds: 300 }, Overlap::Skip, Missed::RunOnce, false, T0, 0).unwrap();
        let b = store.add("skip", When::Every { seconds: 300 }, Overlap::Skip, Missed::Skip, false, T0, 0).unwrap();
        // Ten seconds late: both run.
        let due = store.take_due(T0 + 5 * MIN + 10_000, 0);
        assert_eq!(due.len(), 2);
        // The computer slept for an hour: the missed run happens once (not twelve times) or not at all.
        let due = store.take_due(T0 + 65 * MIN + 1, 0);
        let ids: Vec<&str> = due.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, vec![a.as_str()], "run-once policy runs once; skip policy ({b}) does not");
        assert!(store.schedules.iter().all(|s| s.next_run_ms > T0 + 65 * MIN), "both moved on to the future");
    }

    #[test]
    fn schedules_survive_a_restart_and_only_if_changed_compares_hashes() {
        let dir = std::env::temp_dir().join(format!("aura-sched-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut store = Store::default();
        let id = store.add("check the page", When::Every { seconds: 600 }, Overlap::QueueOne, Missed::RunOnce, true, T0, 0).unwrap();
        store.record_result(&id, T0 + 1, "price was 5 dollars", Some(42));
        store.save(&dir).unwrap();
        let loaded = Store::load(&dir);
        assert_eq!(loaded.schedules, store.schedules);
        let s = &loaded.schedules[0];
        assert!(task_with_state(s).contains("LAST RUN") && task_with_state(s).contains("5 dollars"));
        assert!(!should_run(s, Some(42)), "same screen: skip, no model call");
        assert!(should_run(s, Some(43)) && should_run(s, None));
        assert!(should_run(&Schedule { last_hash: None, ..s.clone() }, Some(42)), "first run always happens");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn editing_and_removing_schedules() {
        let mut store = Store::default();
        let a = store.add("a", When::Every { seconds: 60 }, Overlap::Skip, Missed::Skip, false, T0, 0).unwrap();
        let b = store.add("b", When::Every { seconds: 120 }, Overlap::Skip, Missed::Skip, false, T0, 0).unwrap();
        assert_ne!(a, b);
        assert_eq!(store.earliest(), Some(T0 + MIN));
        assert!(store.remove(&a) && !store.remove(&a));
        assert_eq!(store.earliest(), Some(T0 + 2 * MIN));
        assert!(store.add("bad", When::Every { seconds: 0 }, Overlap::Skip, Missed::Skip, false, T0, 0).is_err());
    }

    fn counting_runner(counter: Arc<std::sync::atomic::AtomicU32>, hold_ms: u64) -> Runner {
        Arc::new(move |_s| {
            let counter = Arc::clone(&counter);
            Box::pin(async move {
                counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                tokio::time::sleep(Duration::from_millis(hold_ms)).await;
                RunReport { summary: "ok".into(), screen_hash: None }
            })
        })
    }

    #[tokio::test]
    async fn the_timer_runs_on_time_and_does_nothing_while_idle() {
        let dir = std::env::temp_dir().join(format!("aura-sched-live-{}", std::process::id()));
        let counter = Arc::new(std::sync::atomic::AtomicU32::new(0));
        // One schedule an hour away (must never fire) and one every second.
        let mut store = Store::default();
        store.add("far", When::Every { seconds: 3600 }, Overlap::Skip, Missed::Skip, false, now_ms(), 0).unwrap();
        let store = Arc::new(Mutex::new(store));
        let changed = Arc::new(tokio::sync::Notify::new());
        let task = tokio::spawn(run_forever(dir.clone(), Arc::clone(&store), Arc::clone(&changed), counting_runner(Arc::clone(&counter), 10), Arc::new(|| None)));
        tokio::time::sleep(Duration::from_millis(1500)).await;
        assert_eq!(counter.load(std::sync::atomic::Ordering::SeqCst), 0, "idle: nothing runs, nothing is called");
        store.lock().unwrap().add("soon", When::Every { seconds: 1 }, Overlap::Skip, Missed::Skip, false, now_ms(), 0).unwrap();
        changed.notify_one();
        tokio::time::sleep(Duration::from_millis(3600)).await;
        let runs = counter.load(std::sync::atomic::Ordering::SeqCst);
        assert!((3..=4).contains(&runs), "about one run per second for 3.6 s, got {runs}");
        task.abort();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn a_run_longer_than_the_interval_is_skipped_or_queued_by_policy() {
        for (overlap, expected) in [(Overlap::Skip, 2u32), (Overlap::QueueOne, 3u32)] {
            let dir = std::env::temp_dir().join(format!("aura-sched-ov-{}-{:?}", std::process::id(), overlap));
            let counter = Arc::new(std::sync::atomic::AtomicU32::new(0));
            let mut store = Store::default();
            store.add("slow", When::Every { seconds: 1 }, overlap, Missed::Skip, false, now_ms(), 0).unwrap();
            let store = Arc::new(Mutex::new(store));
            let changed = Arc::new(tokio::sync::Notify::new());
            // Each run takes 2.2 s against a 1 s interval; watch for 6 s: Skip runs at 1 s and 4 s (2), QueueOne also at 3.2 s and 5.4 s (3).
            let task = tokio::spawn(run_forever(dir.clone(), store, changed, counting_runner(Arc::clone(&counter), 2200), Arc::new(|| None)));
            tokio::time::sleep(Duration::from_millis(6000)).await;
            let runs = counter.load(std::sync::atomic::Ordering::SeqCst);
            assert_eq!(runs, expected, "{overlap:?}");
            task.abort();
            let _ = std::fs::remove_dir_all(&dir);
        }
    }
}
