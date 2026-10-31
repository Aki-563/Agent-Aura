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

**Installer:** from the repo root, `ui\node_modules\.bin\tauri build` builds the page, the exe (icon, version and the per-monitor-DPI manifest embedded) and `src-tauri\target\release\bundle\nsis\Agent Aura_<version>_x64-setup.exe`, a per-user installer that needs no admin rights and fetches WebView2 if it is missing (Windows 11 and up-to-date Windows 10 already have it). A plain `cargo build --release` also embeds the page (build `ui/` first) and the manifest; `dist\agent-aura.exe` is a copy of that exe and needs no other file. Test runs open and close real windows: do not run them while you are working in the same desktop.

**Signing:** set `AURA_SIGN_THUMBPRINT` to the SHA-1 thumbprint of a code-signing certificate in your certificate store (and have `signtool` from the Windows SDK on `PATH`) before `tauri build`; `tools\sign.cmd` then signs the exe, the CLI and the installer with a timestamp. Without it the build is unsigned, and SmartScreen or antivirus programs may warn about or remove the installer.

## Run the production app

Install it with the setup exe (or launch `dist\agent-aura.exe`). The home screen is one box: type the task and press Enter (or the arrow). While a task runs the arrow is a stop button; the footprints icon next to it sets the maximum number of steps.

* **Models:** the connection pill at the top right shows the active model (green when it answers); the gear opens Settings. **Add model** takes a local server's `/v1` address or the OpenAI API, the model ID and an optional key; **Connect** checks the model with a real image request and saves it as a preset in the app's database (`agent-aura.db`, the key encrypted with Windows DPAPI). Tap a preset to switch to it; the bin deletes it (and disconnects it if it was in use). A newly saved model's coordinate convention is measured automatically in the background on a generated picture with four labelled buttons (no screen, no clicks): Qwen3-VL-4B measures as the 0-1000 grid.
* **Schedules from the prompt:** a task that names its own time is saved as a schedule instead of running now: "every day at 9am check my inbox", "open the build page every 30 minutes", "in 10 minutes open Teams", "tomorrow at 8:30, open Outlook". Only a phrase at the very start or end counts ("set an alarm at 7am" is a normal task). The toast offers **Run now instead**; Settings lists scheduled tasks with a delete button.
* **Activity** (the pulse icon): recent tasks, and for the chosen one its result, step count, time and how many steps worked, then every step in order, left to right, with the screenshot the model looked at, the action and its outcome. "Remember this way" saves the run as a skill; tapping a step lets you write what should have happened instead.
* **While a task runs** the window becomes a small player (no shadow) that you can drag anywhere; the place is remembered for the next run, and the player still steps aside when the agent needs to click under it. It shows what the agent is doing, the step number, and a speaker switch for the step reader. Hover it for Pause, the voice-input switch, End, the steer box and approval cards. The pointer becomes a violet-to-cyan arrow while Aura drives it and glides there along a slight curve; the screen edges glow in the state colour and a pill at the top centre names the state. Soft chimes mark the start and the end; `AURA_MUTE=1` silences them.

`%LOCALAPPDATA%\AgentAura\provider.json` names the active model's preset; its key is kept once, DPAPI-encrypted, in the preset (never in logs, traces or repository files). Press **Esc** while a task runs to stop it; outside a task Esc belongs to whatever app is in front.

* **One copy:** starting Aura again brings the open window forward instead of opening a second one (two copies would run every schedule twice).
* **Tray:** with any schedule switched on, closing the window keeps Aura running in the tray so the schedules still happen; click the tray icon to open it, right-click for **Quit**. With no schedules, closing quits.
* **History:** the database keeps the last 200 runs (at most 30 days); their step screenshots are encrypted for your Windows account. Password-like words are masked in history and traces.
* **After a crash:** the cursor, the Sticky Keys shortcut, any window raised above others and a display scale changed by `scale-test` are put back on the next start.

## Testing, voice, schedules, controls and the model JSON (Phase 9)

**Run every test:** `.\test-all.ps1` (add `-Repeat 3` to repeat the desktop suite). It runs the unit tests, then the desktop suite in a separate Chrome with a throwaway profile (never your own Chrome), and prints one final line. The desktop suite drives your real screen and opens and closes only its own Notepad, Calculator and test pages; Esc stops it. Measurements are written to `%LOCALAPPDATA%\AgentAura\testlogs\<date>\*.jsonl`. Groups: `click scroll type key drag window control precision draw load windows safety obstruct driver body native (and, only when named, `system`)` (`aura-cli conformance --browser chrome --only group:click,scroll`).

**Interrupt, pause, steer:** Esc (or Ctrl+Pause) stops at once, even with the model machine unreachable; Pause pauses and resumes; Ctrl+Alt+Space pauses and opens the steer box (or hover the mini player and type). A steer message reaches the model together with a record of what is already done. From another terminal: `aura-cli control stop|pause|resume|status|steer <text>` (port 47900 for `aura-cli run`).

**Voice:** each action is announced with the offline Windows speech engine, using the model's `voice_command` or a phrase made from the action. Settings (voice on/off, volume, rate, voice name, speak-then-act) live in `%LOCALAPPDATA%\AgentAura\harness.json` under `voice`; `AURA_MUTE=1` silences it.

**Schedules:** `aura-cli schedule add --task "<text>" --every 5m | --at 09:00,17:30 [--overlap skip|queue|cancel] [--missed run|skip] [--only-if-changed]`, `schedule list`, `schedule remove <id>`; then `aura-cli daemon --url <server>/v1 --model <id>` runs them. Between runs it only sleeps: no screenshots, no model calls. Schedules are saved in `schedules.json` and survive restarts.

**Added to the model JSON** (everything else is unchanged): top-level `voice_command` (short sentence spoken before the action; optional when parsing). New actions: `triple_click mouse_down mouse_up draw_path key_down key_up wait zoom observe clipboard_read clipboard_write minimize_window maximize_window restore_window close_window move_window resize_window snap_window layout_windows switch_desktop touch`. New optional fields on existing actions: `dwell_ms` (mouse_move), `waypoints button speed` (drag_and_drop), `hold_keys delta` (scroll), `method chars_per_second` (type_text), `hold_ms` (key_press), and `window` (a window id such as `w3`). Coordinate policy: x/y are pixels of the screenshot sent; floats are rounded, numeric strings accepted, anything outside the image is refused with feedback (not clamped).

**Not tested / limits:** Photoshop and Illustrator are not installed (a mock canvas page and Paint stand in); only one monitor was available, so multi-monitor is untested; the real UAC prompt cannot be driven by design (the harness hands over); touch and pen use Windows synthetic pointer injection (Windows 10 1809+) and apps may ignore them; elevated windows and exclusive-fullscreen games ignore injected input.

### Added after the first Phase 9 write-up

**Grounding modes** (`harness.json`, `grounding_mode`): `a` (default) uses the model's own coordinates; `b` asks a second, tiny question on the same screenshot ("where is this target?"), optionally to a separate model (`grounding_model`, `grounding_url`); `c` makes no extra call and instead matches the point against the numbered elements, moving a near miss onto the element the model described and asking the verifier only when nothing matches. Each step writes `grounding.json` to its trace folder and a record to the test log. `coordinate_scale` is `pixels` (default) or `norm1000` for models that answer on a 0 to 1000 grid.

**Parallel windows:** `aura-cli parallel --model <id> --window "Notepad=type hello" --window "Calculator=compute 6*7" [--model-slots 2]`. Each window gets its own worker; model calls overlap, while mouse and keyboard take turns through one arbiter (nothing can overlap), and every turn re-checks the screen before acting.

**Things that get in the way:** a notification, tooltip, floating widget or new window over the target is identified before any click. Toasts and hover cards are waited out (the pointer moves away first); an always-on-top widget is stepped over by raising the target window for that one action; another program's window is never clicked through, and the model is told what is covering the target. UAC prompts and the lock screen are handed to you. The Sticky Keys pop-up shortcut is switched off during a run and put back afterwards; High Contrast, Sticky Keys and Filter Keys are mentioned to the model.

**Per-app tips:** built-in profiles (browser, Excel, Word, editors, terminal, Explorer, creative apps) add a short tip to the prompt and scale the settle patience. Add your own in `%LOCALAPPDATA%\AgentAura\profiles.json` (a list of `{"processes": ["app"], "tips": "...", "settle_factor": 1.5}`).

**Schedules in the app:** written into the task itself (see above). Scheduled runs never ask you anything: risky actions are declined. A locked PC skips the run. A schedule that comes due while your own task runs waits for it (up to 30 minutes); a one-off more than 15 minutes late (the PC was off or asleep) is skipped and marked missed; repeats need at least a minute. "Book a meeting in 2 hours" or "remind me in an hour" stays a normal task (the time belongs to the meeting or reminder).

**Checks and diagnostics:** `aura-cli doctor` asks the saved model one tiny question (Connect does the same). `AURA_SELFTEST_TASK="open notepad" agent-aura.exe` runs a task through the real app and logs every event to `%LOCALAPPDATA%\AgentAura\selftest.log`. `node tools/cdp.mjs eval|shot|errors` drives and screenshots the running app's page (start the app with `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=9222`). `aura-cli soak --minutes 1440` repeats a safe slice of the desktop suite and reports memory, handle and GDI-object growth (the 24 hour run). `cargo test --lib real_speech -- --ignored` is the audible speech check; `cargo test --lib long_pause -- --ignored` is the 10 minute pause test (`AURA_PAUSE_SECS` shortens it).

**Traces never keep secrets:** values after password, api key, token or Bearer, and long mixed-case tokens, are written as `[hidden]`.

### Making any model's orders executable

The harness's job is to carry out what the model orders, exactly, whatever model it is.

* **Coordinate convention per model:** `aura-cli calibrate --url <server>/v1 --model <id>` opens a test page, asks the model to point at known buttons and decides whether it answers in screenshot pixels, on a 0 to 1000 grid (Qwen-VL family) or 0 to 1. The result is saved in `%LOCALAPPDATA%\AgentAura\model_scale.json` and used automatically, prompts included (`coordinate_scale` in `harness.json` overrides it). Measured: `qwen/qwen3-vl-4b` answers on the 0 to 1000 grid, within a few pixels.
* **Other models' JSON dialects are understood:** `coordinate`/`point`/`position`/`start_box`/`bbox` for x and y, `start_coordinate`/`end_coordinate`/`from`/`to` for drags, `"action": "click"` instead of `type`, `"keys": "ctrl+s"` as a string, `content`/`value` for typed text, `"element": "#7"`, a list of `actions` (the first is taken) or the action written at the top level.
* **Miscounted element numbers and stray points are caught:** when the model names a target ("Search", or a key cap like "1" or "+") but gives the number of, or a point on, a different element, the harness aims at the one element carrying that name (accessible names such as "One" and "Plus" count), or lets the verifier look first when there is none.
* **"Done" is checked against the screen:** when the task names exact text to type ("type the sentence hello from aura"), a done claim is rejected until that text is visible, and the model is told what is missing.
* **The taskbar is always offered** (Start, Task View, pinned and running apps) as numbered elements, and "open X and ..." starts X without a model call.
* **Orders suite:** `aura-cli conformance --browser chrome --only group:orders` runs a scripted model's orders (element numbers, pixels, dialects, an eight-step form, drag, scroll, zoom then a click in the zoomed picture, observe, grounding modes B and C) through the whole agent loop and checks the test page received each one.
* **Every display scale:** `aura-cli scale-test` switches the main monitor to each scale it offers up to 200 % (the same call Windows Settings uses), runs the scale-sensitive groups (`system`, `precision`, `click`, `orders`) in a fresh process at each, and puts your scale back, also if it is stopped with Esc.
