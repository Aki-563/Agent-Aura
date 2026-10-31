//! Undo for the temporary system changes a run makes, also after a crash or a force-close.
//!
//! A run swaps the Windows cursors for the working pointer, switches the Sticky/Filter/Toggle Keys
//! shortcuts off, may raise a window to always-on-top, and `aura-cli scale-test` changes the
//! display scale. Each change is written to a small journal file *before* it is made, one file per
//! process (`recovery/<pid>.json` under the data folder), and removed when it is undone. On start
//! the app and `aura-cli` undo whatever a process that no longer runs left behind; a panic undoes
//! this process's own changes at once.

use std::path::PathBuf;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Journal {
    /// The shortcut settings before the run: (sticky, filter, toggle) hotkey active.
    #[serde(default)]
    pub shortcuts: Option<(bool, bool, bool)>,
    /// Windows made topmost: (window handle, owning process id).
    #[serde(default)]
    pub raised: Vec<(isize, u32)>,
    #[serde(default)]
    pub cursor: bool,
    /// The display scale (percent) to put back.
    #[serde(default)]
    pub scale: Option<u32>,
}

impl Journal {
    fn is_empty(&self) -> bool {
        self.shortcuts.is_none() && self.raised.is_empty() && !self.cursor && self.scale.is_none()
    }
}

static CURRENT: Mutex<Journal> = Mutex::new(Journal { shortcuts: None, raised: Vec::new(), cursor: false, scale: None });

fn folder() -> PathBuf {
    std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir).join("AgentAura").join("recovery")
}

fn own_file() -> PathBuf {
    folder().join(format!("{}.json", std::process::id()))
}

/// Changes this process's journal and writes it out (or deletes the file when nothing is left).
fn update(change: impl FnOnce(&mut Journal)) {
    if cfg!(test) {
        return;
    }
    let mut journal = CURRENT.lock().unwrap_or_else(|e| e.into_inner());
    change(&mut journal);
    let path = own_file();
    if journal.is_empty() {
        let _ = std::fs::remove_file(&path);
    } else if let Ok(text) = serde_json::to_string(&*journal) {
        let _ = std::fs::create_dir_all(folder());
        let _ = std::fs::write(&path, text);
    }
}

pub fn note_shortcuts(before: Option<(bool, bool, bool)>) {
    update(|j| j.shortcuts = before);
}

pub fn note_raised(hwnd: isize, pid: u32) {
    update(|j| {
        if !j.raised.iter().any(|(h, _)| *h == hwnd) {
            j.raised.push((hwnd, pid));
        }
    });
}

pub fn clear_raised() {
    update(|j| j.raised.clear());
}

pub fn note_cursor(replaced: bool) {
    update(|j| j.cursor = replaced);
}

pub fn note_scale(original: Option<u32>) {
    update(|j| j.scale = original);
}

/// Puts back everything in `journal`.
fn undo(journal: &Journal) {
    if let Some((sticky, filter, toggle)) = journal.shortcuts {
        crate::system_state::restore_shortcuts(sticky, filter, toggle);
    }
    for (hwnd, pid) in &journal.raised {
        crate::desktop::obstruction::unraise_if_owned(*hwnd, *pid);
    }
    if journal.cursor {
        crate::overlay::cursor::restore();
    }
    if let Some(scale) = journal.scale {
        if let Err(error) = crate::display_scale::set(scale) {
            eprintln!("[aura] could not put the display scale back to {scale}%: {error}");
        }
    }
}

/// Undoes this process's own changes now (the panic hook calls this).
pub fn undo_own() {
    let journal = CURRENT.lock().map(|j| j.clone()).unwrap_or_default();
    undo(&journal);
    update(|j| *j = Journal::default());
}

/// Ctrl+C, Ctrl+Break or closing the console window ends the process without running destructors;
/// this undoes the journal first (for `aura-cli`).
pub fn undo_on_console_close() {
    use windows_sys::Win32::System::Console::SetConsoleCtrlHandler;
    unsafe extern "system" fn handler(_kind: u32) -> i32 {
        undo_own();
        0 // let the default handler end the process
    }
    unsafe { SetConsoleCtrlHandler(Some(handler), 1) };
}

fn process_alive(pid: u32) -> bool {
    use windows_sys::Win32::Foundation::{CloseHandle, STILL_ACTIVE};
    use windows_sys::Win32::System::Threading::{GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            return false;
        }
        let mut code = 0u32;
        let ok = GetExitCodeProcess(handle, &mut code);
        CloseHandle(handle);
        ok != 0 && code == STILL_ACTIVE as u32
    }
}

/// Undoes what Aura processes that are gone (crashed, killed) left behind. Returns what was found.
pub fn recover_leftovers() -> Vec<Journal> {
    if cfg!(test) {
        return Vec::new();
    }
    let mut found = Vec::new();
    let Ok(entries) = std::fs::read_dir(folder()) else { return found };
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(pid) = path.file_stem().and_then(|s| s.to_str()).and_then(|s| s.parse::<u32>().ok()) else { continue };
        if pid == std::process::id() || process_alive(pid) {
            continue;
        }
        if let Some(journal) = std::fs::read_to_string(&path).ok().and_then(|t| serde_json::from_str::<Journal>(&t).ok()) {
            undo(&journal);
            found.push(journal);
        }
        let _ = std::fs::remove_file(&path);
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_journal_round_trips_and_knows_when_it_is_empty() {
        let journal = Journal { shortcuts: Some((true, false, true)), raised: vec![(42, 7)], cursor: true, scale: Some(125) };
        let text = serde_json::to_string(&journal).unwrap();
        assert_eq!(serde_json::from_str::<Journal>(&text).unwrap(), journal);
        assert!(!journal.is_empty());
        assert!(Journal::default().is_empty());
        // Older or partial files still load.
        assert_eq!(serde_json::from_str::<Journal>("{\"cursor\":true}").unwrap(), Journal { cursor: true, ..Journal::default() });
    }

    #[test]
    fn this_process_is_alive_and_a_made_up_one_is_not() {
        assert!(process_alive(std::process::id()));
        assert!(!process_alive(0xFFFF_FFF0));
    }
}
