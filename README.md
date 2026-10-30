# Agent Aura ⚡

**An ultra-fast, self-learning, human-like desktop computer-use agent built in 100% Rust.**

---

## 📚 Project Documentation

All specifications, architectural blueprints, and tracking checklists are located in the [`docs/`](./docs/) directory:

* 📄 **[CORE.md](./docs/CORE.md)**: Core engine runtime, typed JSON action grammar, coordinate math, and Win32 `SendInput` actuation.
* 📄 **[FEATURES.md](./docs/FEATURES.md)**: Master feature specification (11 Speed Features + 12 Human-Like Features).
* 📄 **[IMPLEMENTATION_PLAN.md](./docs/IMPLEMENTATION_PLAN.md)**: Phase-by-phase development roadmap.
* 📄 **[PROGRESS_TRACKER.md](./docs/PROGRESS_TRACKER.md)**: Live task checklist and completion status.
* 📄 **[REVERSE_ENGINEERING_REPORT.md](./docs/REVERSE_ENGINEERING_REPORT.md)**: Teardown and architectural findings from the Agent-S reference.

---

## 🤖 Instructions for AI Coding Agents

Any AI coding agent (Claude Code, Cursor, Antigravity, Copilot, etc.) must review **[`AGENTS.md`](./AGENTS.md)** before writing or modifying code.

---

## Build, test and drive it

Windows 10/11, Rust with the **MSVC** toolchain (Visual Studio 2022 Build Tools with the C++ workload and the Windows SDK; `rust-toolchain.toml` pins it). All commands run from `src-tauri/`:

```
cargo test --lib                          # unit tests (about a minute; some use the real screen, none send input)
cargo build --release                     # agent-aura.exe and aura-cli.exe in target/release
target\release\aura-cli.exe conformance  # model-free harness tests: they DRIVE YOUR REAL SCREEN (browser, Notepad, Calculator); Esc stops
target\release\aura-cli.exe demo         # the on-screen banner, the smooth pointer glide and the start/finish sounds (about 6 s)
target\release\aura-cli.exe look         # what the model would see: windows, numbered elements, screenshot saved to %TEMP%\aura-look.png
target\release\aura-cli.exe do click "#12" --task "open the Issues tab"   # one order through the same path the agent uses
target\release\aura-cli.exe eval --model <id> --suite basic --repeat 3 --url http://<host>:1234/v1   # model pass rates
```

The frontend must be built (`npm run build` in `ui/`) before a release build so the exe embeds it. Copy `target/release/agent-aura.exe` to `dist/` (keep `dist/` to that exe, its manifest and `WebView2Loader.dll`). Test runs open and close real windows: do not run them while you are working in the same desktop.

## Run the production app

Launch `dist/agent-aura.exe`.

1. Choose **OpenAI Responses API** or **Local / OpenAI-compatible server**.
2. Enter the exact vision-capable model ID.
3. For OpenAI, enter the API key. For a local server, enter its `/v1` base URL and leave the key empty unless the server requires one.
4. Select **Save & test connection**.
5. Enter a desktop task, choose a maximum step limit, and select **Run autonomously**.
6. Agent Aura repeatedly observes, plans, executes, and verifies without step approvals. Press **Esc** at any time to stop.

While a run is active the screen edge glows and a banner at the top centre says what the agent is doing (taking a screenshot, thinking, working). A sound plays when a task starts and when it finishes; set the environment variable `AURA_MUTE=1` to silence it.

Provider settings are saved in `%LOCALAPPDATA%\AgentAura\provider.json`; the API key in it is encrypted with Windows DPAPI, so only your Windows account on this computer can read it. It is never written to SQLite, browser storage, logs, traces or repository files. "Clear" in the app deletes the saved file. Press **Esc** while actions are executing to trigger the emergency stop.

## Testing, voice, schedules, controls and the model JSON (Phase 9)

**Run every test:** `.	est-all.ps1` (add `-Repeat 3` to repeat the desktop suite). It runs the unit tests, then the desktop suite in a separate Chrome with a throwaway profile (never your own Chrome), and prints one final line. The desktop suite drives your real screen and opens and closes only its own Notepad, Calculator and test pages; Esc stops it. Measurements are written to `%LOCALAPPDATA%\AgentAura	estlogs\<date>\*.jsonl`. Groups: `click scroll type key drag window control precision draw load windows safety driver body native` (`aura-cli conformance --browser chrome --only group:click,scroll`).

**Interrupt, pause, steer:** Esc (or Ctrl+Pause) stops at once, even with the model machine unreachable; Pause pauses and resumes; Ctrl+Alt+Space pauses and opens the steer box (or hover the mini player and type). A steer message reaches the model together with a record of what is already done. From another terminal: `aura-cli control stop|pause|resume|status|steer <text>` (port 47900 for `aura-cli run`).

**Voice:** each action is announced with the offline Windows speech engine, using the model's `voice_command` or a phrase made from the action. Settings (voice on/off, volume, rate, voice name, speak-then-act) live in `%LOCALAPPDATA%\AgentAura\harness.json` under `voice`; `AURA_MUTE=1` silences it.

**Schedules:** `aura-cli schedule add --task "<text>" --every 5m | --at 09:00,17:30 [--overlap skip|queue|cancel] [--missed run|skip] [--only-if-changed]`, `schedule list`, `schedule remove <id>`; then `aura-cli daemon --url <server>/v1 --model <id>` runs them. Between runs it only sleeps: no screenshots, no model calls. Schedules are saved in `schedules.json` and survive restarts.

**Added to the model JSON** (everything else is unchanged): top-level `voice_command` (short sentence spoken before the action; optional when parsing). New actions: `triple_click mouse_down mouse_up draw_path key_down key_up wait zoom observe clipboard_read clipboard_write minimize_window maximize_window restore_window close_window move_window resize_window snap_window layout_windows switch_desktop touch`. New optional fields on existing actions: `dwell_ms` (mouse_move), `waypoints button speed` (drag_and_drop), `hold_keys delta` (scroll), `method chars_per_second` (type_text), `hold_ms` (key_press), and `window` (a window id such as `w3`). Coordinate policy: x/y are pixels of the screenshot sent; floats are rounded, numeric strings accepted, anything outside the image is refused with feedback (not clamped).

**Not tested / limits:** Photoshop and Illustrator are not installed (a mock canvas page and Paint stand in); only one monitor was available, so multi-monitor is untested; the real UAC prompt cannot be driven by design (the harness hands over); touch and pen use Windows synthetic pointer injection (Windows 10 1809+) and apps may ignore them; elevated windows and exclusive-fullscreen games ignore injected input.
