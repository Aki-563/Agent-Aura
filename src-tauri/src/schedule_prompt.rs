//! Schedules written into the task itself: "every day at 9am check my inbox", "open the build
//! page every 30 minutes", "in 10 minutes remind me to stretch", "tomorrow at 8:30, open Teams".
//! The time phrase is taken off the task, which then runs on that schedule.
//!
//! Only phrases at the very start or the very end of the prompt count, and a bare one-off "at 7am"
//! only at the start: "set an alarm at 7am" is a task about an alarm, not a schedule.

use regex::Regex;
use std::sync::OnceLock;

use crate::schedule::{local_to_utc, Tz, When};

/// A prompt split into the task and when it should run.
#[derive(Debug, Clone, PartialEq)]
pub struct Scheduled {
    pub task: String,
    pub when: When,
}

const TIME: &str = r"(?:\d{1,2}(?::\d{2})?\s*(?:a\.?m\.?|p\.?m\.?)?|noon|midnight)";
const NUMBER: &str = r"(?:\d+|an?|one|two|three|four|five|six|ten|twelve|fifteen|twenty|thirty|forty[- ]five|sixty|ninety)";
const UNIT: &str = r"(?:seconds?|secs?|minutes?|mins?|hours?|hrs?|days?)";

fn number_value(word: &str) -> Option<u64> {
    let word = word.trim().to_ascii_lowercase();
    if let Ok(n) = word.parse() {
        return Some(n);
    }
    Some(match word.as_str() {
        "a" | "an" | "one" => 1,
        "two" => 2,
        "three" => 3,
        "four" => 4,
        "five" => 5,
        "six" => 6,
        "ten" => 10,
        "twelve" => 12,
        "fifteen" => 15,
        "twenty" => 20,
        "thirty" => 30,
        "forty-five" | "forty five" => 45,
        "sixty" => 60,
        "ninety" => 90,
        _ => return None,
    })
}

fn unit_seconds(unit: &str) -> u64 {
    match unit.trim().to_ascii_lowercase().chars().next() {
        Some('s') => 1,
        Some('m') => 60,
        Some('h') => 3600,
        _ => 86_400,
    }
}

/// "9", "9am", "9:30 pm", "21:15", "noon", "midnight" -> minutes after midnight.
pub fn clock_minutes(text: &str) -> Option<u32> {
    let text = text.trim().to_ascii_lowercase().replace('.', "");
    match text.as_str() {
        "noon" => return Some(12 * 60),
        "midnight" => return Some(0),
        _ => {}
    }
    let (digits, suffix) = match text.find(|c: char| c == 'a' || c == 'p') {
        Some(i) => (text[..i].trim(), Some(text[i..].trim())),
        None => (text.trim(), None),
    };
    let (h, m) = match digits.split_once(':') {
        Some((h, m)) => (h.trim().parse::<u32>().ok()?, m.trim().parse::<u32>().ok()?),
        None => (digits.parse::<u32>().ok()?, 0),
    };
    if m >= 60 {
        return None;
    }
    let h = match suffix {
        Some(s) if s.starts_with('p') => {
            if !(1..=12).contains(&h) {
                return None;
            }
            h % 12 + 12
        }
        Some(_) => {
            if !(1..=12).contains(&h) {
                return None;
            }
            h % 12
        }
        None => h,
    };
    (h < 24).then_some(h * 60 + m)
}

fn hhmm(minutes: u32) -> String {
    format!("{:02}:{:02}", minutes / 60, minutes % 60)
}

/// All the clock times in a list such as "9am and 5:30pm" or "8, 12 and 18:00".
fn times_in(list: &str) -> Vec<String> {
    let re = time_regex();
    let mut times: Vec<String> = re.find_iter(list).filter_map(|m| clock_minutes(m.as_str())).map(hhmm).collect();
    times.dedup();
    times
}

fn time_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(&format!("(?i){TIME}")).expect("time regex"))
}

/// The recurring and one-off phrase patterns. Each has a name for `when_of`.
fn phrases() -> &'static [(&'static str, Regex, bool)] {
    static LIST: OnceLock<Vec<(&'static str, Regex, bool)>> = OnceLock::new();
    LIST.get_or_init(|| {
        let times = format!(r"{TIME}(?:\s*(?:,|and|&)\s*{TIME})*");
        let daily = r"(?:every\s*day|everyday|daily|each\s+day|every\s+(?:morning|afternoon|evening|night)|each\s+(?:morning|evening))";
        // (name, phrase, allowed at the end of the prompt too)
        let raw: Vec<(&'static str, String, bool)> = vec![
            ("daily_at", format!(r"{daily}\s+at\s+(?P<times>{times})"), true),
            ("at_daily", format!(r"at\s+(?P<times>{times})\s+{daily}"), true),
            ("every_n", format!(r"every\s+(?P<n>{NUMBER})\s*(?P<unit>{UNIT})"), true),
            ("every_half", r"every\s+half(?:\s+an)?\s+hour".to_owned(), true),
            ("every_unit", r"every\s+(?P<unit>second|minute|hour)|(?P<hourly>hourly)".to_owned(), true),
            ("every_part", r"every\s+(?P<part>morning|afternoon|evening|night)".to_owned(), true),
            ("every_day", r"every\s*day|everyday|daily".to_owned(), true),
            ("in_n", format!(r"in\s+(?P<n>{NUMBER})\s*(?P<unit>{UNIT})"), true),
            ("in_half", r"in\s+half\s+an\s+hour".to_owned(), true),
            ("tomorrow_at", format!(r"tomorrow(?:\s+(?:morning|afternoon|evening))?\s+at\s+(?P<time>{TIME})"), true),
            ("today_at", format!(r"(?:today|tonight|this\s+(?:morning|afternoon|evening))\s+at\s+(?P<time>{TIME})"), true),
            ("at_tomorrow", format!(r"at\s+(?P<time>{TIME})\s+tomorrow"), true),
            ("at_today", format!(r"at\s+(?P<time>{TIME})\s+(?:today|tonight)"), true),
            ("at", format!(r"at\s+(?P<time>{TIME})"), false),
        ];
        raw.into_iter()
            .map(|(name, phrase, at_end)| (name, Regex::new(&format!(r"(?i)^(?:{phrase})$")).expect("schedule phrase"), at_end))
            .collect()
    })
}

const DAY_MS: u64 = 86_400_000;

/// The next time (Unix ms) the local clock shows `minutes`, today or tomorrow (with the offset in
/// force on that day, so a daylight-saving change overnight does not shift it by an hour).
fn next_local(minutes: u32, now_ms: u64, tz: Tz, tomorrow: bool) -> u64 {
    let local_now = now_ms as i64 + tz.offset_at(now_ms as i64) as i64 * 60_000;
    let day_start = local_now - local_now.rem_euclid(DAY_MS as i64);
    let mut local = day_start + minutes as i64 * 60_000;
    if tomorrow || local_to_utc(local, tz) <= now_ms as i64 {
        local += DAY_MS as i64;
    }
    local_to_utc(local, tz).max(0) as u64
}

/// Words that make a time at the end of a prompt part of the task itself: "book a meeting in
/// 2 hours" books it now, for later; "remind me in an hour" sets a reminder now.
fn time_belongs_to_the_task(task: &str) -> bool {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)\b(?:book|booking|schedule|reschedule|remind|reminder|meeting|appointment|alarm|timer|event|calendar|reserve|reservation|deadline|due|invite|invitation|countdown)\b").expect("task words")
    })
    .is_match(task)
}

fn when_of(name: &str, caps: &regex::Captures, now_ms: u64, tz: Tz) -> Option<When> {
    let get = |k: &str| caps.name(k).map(|m| m.as_str().to_owned());
    match name {
        "daily_at" | "at_daily" => {
            let times = times_in(&get("times")?);
            (!times.is_empty()).then_some(When::DailyAt { times })
        }
        "every_n" => {
            let seconds = number_value(&get("n")?)?.checked_mul(unit_seconds(&get("unit")?))?;
            (seconds >= 1).then_some(When::Every { seconds })
        }
        "every_half" => Some(When::Every { seconds: 1800 }),
        "every_unit" => Some(When::Every { seconds: get("unit").map(|u| unit_seconds(&u)).unwrap_or(3600) }),
        "every_part" => {
            let time = match get("part")?.to_ascii_lowercase().as_str() {
                "morning" => "09:00",
                "afternoon" => "14:00",
                "evening" => "18:00",
                _ => "21:00",
            };
            Some(When::DailyAt { times: vec![time.to_owned()] })
        }
        "every_day" => Some(When::Every { seconds: 86_400 }),
        "in_n" => {
            let seconds = number_value(&get("n")?)?.checked_mul(unit_seconds(&get("unit")?))?;
            Some(When::Once { at_ms: now_ms + seconds * 1000 })
        }
        "in_half" => Some(When::Once { at_ms: now_ms + 1_800_000 }),
        "tomorrow_at" | "at_tomorrow" => Some(When::Once { at_ms: next_local(clock_minutes(&get("time")?)?, now_ms, tz, true) }),
        "today_at" | "at_today" | "at" => Some(When::Once { at_ms: next_local(clock_minutes(&get("time")?)?, now_ms, tz, false) }),
        _ => None,
    }
}

/// What is left is a real task: not empty and not itself only a time phrase ("every day at 9am"
/// read as "every day" + the task "at 9am").
fn usable(task: &str) -> bool {
    !task.is_empty() && !phrases().iter().any(|(_, re, _)| re.is_match(task))
}

fn tidy(task: &str) -> String {
    let trimmed = task.trim_matches(|c: char| c.is_whitespace() || ",;:-–—".contains(c));
    let trimmed = trimmed.strip_prefix("then ").or_else(|| trimmed.strip_prefix("Then ")).unwrap_or(trimmed);
    trimmed.trim().to_owned()
}

/// Splits `prompt` into a task and its schedule, or None when it names no schedule.
pub fn parse(prompt: &str, now_ms: u64, tz: impl Into<Tz>) -> Option<Scheduled> {
    let tz = tz.into();
    let prompt = prompt.trim();
    // Word boundaries where the phrase may end (start of prompt) or begin (end of prompt).
    let cut_points: Vec<usize> = prompt
        .char_indices()
        .filter(|(i, c)| *i > 0 && (c.is_whitespace() || ",;:".contains(*c)))
        .map(|(i, _)| i)
        .collect();
    // Longest phrase first, so "every day at 9am" wins over "every day".
    for (name, re, at_end) in phrases() {
        for &cut in cut_points.iter().rev() {
            let (head, tail) = prompt.split_at(cut);
            if let Some(caps) = re.captures(head.trim()) {
                let task = tidy(tail);
                if usable(&task) {
                    if let Some(when) = when_of(name, &caps, now_ms, tz) {
                        return Some(Scheduled { task, when });
                    }
                }
            }
        }
        if !*at_end {
            continue;
        }
        for &cut in &cut_points {
            let (head, tail) = prompt.split_at(cut);
            let phrase = tail.trim_matches(|c: char| c.is_whitespace() || ",;:.!".contains(c));
            if let Some(caps) = re.captures(phrase) {
                let task = tidy(head);
                // A one-off time at the end of a booking or reminder task is the task's own time.
                let one_off = !name.starts_with("every") && !name.contains("daily");
                if usable(&task) && !(one_off && time_belongs_to_the_task(&task)) {
                    if let Some(when) = when_of(name, &caps, now_ms, tz) {
                        return Some(Scheduled { task, when });
                    }
                }
            }
        }
    }
    None
}

/// "every 30 min", "daily at 09:00, 17:30", "once at 14:05" (local time).
pub fn describe(when: &When, tz_offset_min: i32) -> String {
    match when {
        When::Every { seconds } => {
            if *seconds == 86_400 {
                "every day".to_owned()
            } else if seconds % 3600 == 0 {
                let h = seconds / 3600;
                if h == 1 { "every hour".to_owned() } else { format!("every {h} hours") }
            } else if seconds % 60 == 0 {
                let m = seconds / 60;
                if m == 1 { "every minute".to_owned() } else { format!("every {m} min") }
            } else {
                format!("every {seconds} s")
            }
        }
        When::DailyAt { times } => format!("daily at {}", times.join(", ")),
        When::Once { at_ms } => {
            let local = *at_ms as i64 + tz_offset_min as i64 * 60_000;
            let minutes = (local.rem_euclid(DAY_MS as i64) / 60_000) as u32;
            let today = (crate::schedule::now_ms() as i64 + tz_offset_min as i64 * 60_000).div_euclid(DAY_MS as i64);
            let day = local.div_euclid(DAY_MS as i64);
            let which = match day - today {
                0 => "today",
                1 => "tomorrow",
                _ => "on a later day",
            };
            format!("once, {which} at {}", hhmm(minutes))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // 2026-10-30 10:00:00 UTC, in a UTC+5:30 zone (local 15:30).
    const NOW: u64 = 1_793_354_400_000;
    const TZ: i32 = 330;

    fn p(text: &str) -> Option<Scheduled> {
        parse(text, NOW, TZ)
    }

    #[test]
    fn clock_times_are_read_the_way_people_write_them() {
        assert_eq!(clock_minutes("9am"), Some(540));
        assert_eq!(clock_minutes("9:30 PM"), Some(21 * 60 + 30));
        assert_eq!(clock_minutes("12am"), Some(0));
        assert_eq!(clock_minutes("12pm"), Some(720));
        assert_eq!(clock_minutes("21:15"), Some(21 * 60 + 15));
        assert_eq!(clock_minutes("noon"), Some(720));
        assert_eq!(clock_minutes("13pm"), None);
        assert_eq!(clock_minutes("25:00"), None);
    }

    #[test]
    fn recurring_phrases_at_the_start_or_the_end() {
        let s = p("every day at 9am check my inbox and summarise it").unwrap();
        assert_eq!(s.task, "check my inbox and summarise it");
        assert_eq!(s.when, When::DailyAt { times: vec!["09:00".into()] });

        let s = p("Open the build page every 30 minutes").unwrap();
        assert_eq!(s.task, "Open the build page");
        assert_eq!(s.when, When::Every { seconds: 1800 });

        let s = p("check the weather daily at 7:30 and 18:00.").unwrap();
        assert_eq!(s.task, "check the weather");
        assert_eq!(s.when, When::DailyAt { times: vec!["07:30".into(), "18:00".into()] });

        let s = p("Every hour, save the document").unwrap();
        assert_eq!((s.task.as_str(), s.when), ("save the document", When::Every { seconds: 3600 }));

        let s = p("at 6pm every day, close Teams").unwrap();
        assert_eq!(s.when, When::DailyAt { times: vec!["18:00".into()] });
        assert_eq!(s.task, "close Teams");

        assert_eq!(p("every morning open Outlook").unwrap().when, When::DailyAt { times: vec!["09:00".into()] });
        assert_eq!(p("refresh the dashboard every half hour").unwrap().when, When::Every { seconds: 1800 });
        assert_eq!(p("every two hours, empty the downloads view").unwrap().when, When::Every { seconds: 7200 });
    }

    #[test]
    fn one_off_phrases() {
        let s = p("in 10 minutes open notepad and type hello").unwrap();
        assert_eq!(s.task, "open notepad and type hello");
        assert_eq!(s.when, When::Once { at_ms: NOW + 600_000 });

        let s = p("in an hour, close the browser").unwrap();
        assert_eq!(s.when, When::Once { at_ms: NOW + 3_600_000 });
        assert_eq!(s.task, "close the browser");

        // Local 15:30 now: "at 5pm" is today at 17:00 local = 11:30 UTC.
        let s = p("At 5pm, open the timesheet").unwrap();
        assert_eq!(s.when, When::Once { at_ms: NOW + 90 * 60_000 });
        // "at 9am" has passed today: tomorrow.
        let s = p("at 9am open the timesheet").unwrap();
        assert_eq!(s.when, When::Once { at_ms: NOW + (24 * 60 - 390) * 60_000 });
        let s = p("open Teams tomorrow at 8:30").unwrap();
        assert_eq!(s.task, "open Teams");
        assert_eq!(s.when, When::Once { at_ms: NOW + (24 * 60 - 420) * 60_000 });
    }

    #[test]
    fn ordinary_tasks_are_not_schedules() {
        for text in [
            "open notepad",
            "set an alarm at 7am in the Clock app",
            "type the word daily into notepad",
            "search for the best restaurants in town",
            "open calculator and compute 12 + 30",
            "every",
            "every day at 9am",
            "book a meeting with Sam in 2 hours",
            "remind me to stretch in an hour",
            "set a timer for the oven in 20 minutes",
            "schedule the review call tomorrow at 10am",
        ] {
            assert_eq!(p(text), None, "{text}");
        }
    }

    #[test]
    fn descriptions_are_short_and_local() {
        assert_eq!(describe(&When::Every { seconds: 1800 }, TZ), "every 30 min");
        assert_eq!(describe(&When::Every { seconds: 3600 }, TZ), "every hour");
        assert_eq!(describe(&When::DailyAt { times: vec!["09:00".into()] }, TZ), "daily at 09:00");
    }
}
