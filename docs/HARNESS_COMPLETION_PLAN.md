# Agent Aura — Harness Completion Plan (`HARNESS_COMPLETION_PLAN.md`)

**Document Version:** 1.0 (2026-10-30)
**Phase:** 9 — Harness completion, testing and hardening (planned; starts when the user approves this plan)
**Source:** the user's brief "Complete, Test, and Harden My Computer-Use Agent Harness" (sections referenced below as *Brief §N*).
**Relation to other docs:** `RELIABILITY_PLAN.md` §5.10 stays the description of what is built. This plan only **adds** to it or **corrects** it. Where the two disagree on something already built, the code is the truth, as before.

---

## 0. Working rules for this phase (from the brief)

1. **Add or correct, never rewrite.** Keep existing actions, JSON keys, files and behaviour. A new capability is a new key, a new action, a new module or a new check. Existing code changes only when an item below names a bug, or when a test proves it wrong.
2. **Test-fix loop.** Every milestone has tests with numeric thresholds. When one fails, fix the harness in the same session and re-run it and its neighbours. A test that passes 19 of 20 times is failing.
3. **No reports.** No pass/fail tables or score summaries. Numbers go into log files only (`%LOCALAPPDATA%\AgentAura\testlogs\<date>\*.jsonl`).
4. **Ask only for decisions the code can't answer** (§3 lists the open ones).
5. **Project test-safety rules still apply** (`AGENTS.md`, `RELIABILITY_PLAN.md` §12): only touch windows a test opened, say what a desktop test will touch before running it, never send Ctrl+A, Delete, Ctrl+W, Alt+F4 or a close outside a verified test window.
6. After each milestone: `cargo test --lib`, the milestone's conformance group, tick the boxes here and in `PROGRESS_TRACKER.md`, add a line to §10.

---

## 1. Architecture as built (short note)

| Layer | Where | What it does today |
|---|---|---|
| Main loop | `agent.rs::autonomous_loop` (single loop, single target window) | Observe → propose (model call) → conditional verify (2nd call) → guards (`prepare_input`) → execute → settle and measure the effect → reuse the settled capture as the next observation. Done claims get a `done_check` call. |
| Model client | `agent.rs::request_model` and `build_request` | OpenAI Responses or OpenAI-compatible chat, `json_schema` output with text fallback, JPEG data URL, `tokio::select!` against the kill switch. |
| Contract | `model_schema.rs` | Proposal schema (camelCase top level: `previousOutcome`, `reflection`, `thought`, `target`, `expectedOutcome`, `action`, `confidence`, `impact`). 16 action types, `anyOf` variants with `additionalProperties: false`. Lenient parse, then strict `ActionPrimitive`. |
| Session | `reliability.rs::TaskSession` | Text-only history of the last 8 steps (3 shown in the prompt), notes, impact gate. |
| Screenshot | `agent.rs::capture_screen`, `capture/dxgi.rs`, `capture/gdi.rs` | Whole monitor holding the active window, Lanczos resize to at most 1280 px, JPEG q88. A new DXGI duplication per capture, with GDI fallback. |
| Perception | `perception.rs`, `marks.rs`, `coords.rs` | UIA from the window handle plus OCR clipped to the active program, ≤50 numbered marks, raw x/y snapped onto elements (14 px), per-model offset learning. |
| Windows | `desktop/target.rs`, `desktop/panel.rs`, `desktop/affinity.rs` | Target tracking, foreground guard (AttachThreadInput and Alt tap), occlusion check, `WINDOWS OPEN` list (title, program, ACTIVE/minimized/other-monitor), Aura excluded from capture. |
| Input | `input/send_input.rs`, `clipboard.rs`, `killswitch.rs`, `takeover.rs` | `SendInput` absolute over the virtual desktop, glide with easing, move confirmed within ±2 px, VK for ASCII and Unicode packets otherwise, paste with clipboard restore, `release_all()`. Esc and Ctrl+Pause kill (injected keys ignored). Auto-pause on real user input. |
| Settle | `settle/effect.rs`, `settle/cursor.rs` | Change-then-stable state machine per action class, busy-cursor check, effect text fed to the next prompt. |
| Tests | `cargo test --lib` (122), `loop_tests.rs` + `mock_server.rs` (scripted model), `aura-cli conformance` (91 desktop cases on `eval_fixture.rs` pages at `127.0.0.1:47831` plus Notepad and Calculator), `aura-cli look/do` (acting as the model) | |
| Feedback | `overlay/`, `sound.rs`, `trace.rs` | HUD glow and banner, start and finish sounds, per-step trace folder. |

---

## 2. Brief §12 checks: findings and where they get fixed

| # | Question | Finding in the code | Fix |
|---|---|---|---|
| 1 | Coordinate system | Image pixels of the JPEG sent (≤1280 long edge), scaled back by `scale_action_checked` with the monitor origin. Out-of-image values are rejected, not clamped. Correct in principle. Risks: rounding at 125%/150%, the move check accepts ±2 px, and the `SendInput` 0–65535 rounding (`normalize_coordinates` uses `/(vw-1)`) is not proven exact on every pixel of multi-monitor layouts. | H2 |
| 2 | Image size | 1280 px long edge, Lanczos. Small text on a 4K monitor is unreadable at that size, and there is no zoom action. Whether LM Studio resizes again inside the model is unknown. | H2 (exhaustive mapping test), H4 (`zoom`, per-provider `max_image_edge` setting, measured) |
| 3 | History growth | Text only, last 8 steps kept and 3 shown. Notes are capped. No screenshots in history. Fine for small context windows. Steering messages and the completed-step ledger need a fixed budget. | H1 |
| 4 | Loop termination | `done` is checked by a `done_check` call, there is a stuck streak and a step cap (clamped to 30), plus a loop detector. Missing: wall-clock limit, model-call limit, and identical-action detection across non-adjacent steps. | H6 |
| 5 | Grounding | The main model returns coordinates or an element number. **Per step: 1 capture** (the post-action settled capture is reused), **1 model call** normally, **+1** verifier call when raw x/y fall outside every listed element or the impact is non-routine, **+1** `done_check` on done. No modes, no metrics. | H4 |
| 6 | Model server concurrency | Unknown. Calls are strictly sequential today. | H5 (probe command measures it) |
| 7 | UI Automation | Yes: elements, focus read-back, scroll data, Value read-back. | — |
| 8 | Input method | `SendInput`. Elevated windows are refused with a clear message (UIPI). Exclusive-fullscreen games and anti-cheat may ignore injected input: document it, and detect "no effect" through settle. | H2 (docs), H8 |
| 9 | Fixed sleeps | Several: 35 ms after a move, 45 ms between actions, 150/250/600 ms around focus and app start, a 2.5 s quiet period for takeover. Some are input pacing (they stay, moved into the app profiles). The ones in the observe path become settle conditions. | H4 |
| 10 | Screenshot scope | **Whole monitor holding the active window**, not a window crop (good), but other monitors are invisible to the model, and the window list doesn't say what the image covers. | H3 |
| 11 | Window awareness | `WINDOWS OPEN: N` (count and numbered list, flags ACTIVE/minimized/other monitor) is in every prompt. Missing: stable ids, rects, z, covered-by, not-responding, minimized count, screenshot scope and offset, structured change list, and validating actions against an explicit window id. The registry is rebuilt per call (`LISTED` keeps only the numbering). | H3 |

---

## 3. Decisions

### 3.1 Defaults used unless the user says otherwise

- **D9.1 `voice_command` naming.** The brief names the key `voice_command`, but the existing top-level keys are camelCase. The schema uses `voice_command` exactly as the brief says, and the parser also accepts `voiceCommand`. The key is optional in the parser (backward compatible). In the schema it is listed as **required**, because constrained decoders rarely emit optional keys; a model without schema support can still leave it out. Why: otherwise small models would almost never speak.
- **D9.2 TTS engine.** Windows SAPI 5 (`ISpVoice`, through the `windows` crate) on its own COM thread. No Python (pyttsx3 and Piper are ruled out by the stack rules). Offline, built in, supports async speak and purge. WinRT `SpeechSynthesizer` stays as a later option for the newer voices.
- **D9.3 Hotkeys.** Esc and Ctrl+Pause stay as hard stop. New: **Pause** key for pause/resume, **Ctrl+Alt+Space** to open the steer box. Both configurable, handled in the existing low-level hook, injected keys ignored.
- **D9.4 Window ids.** The `WINDOWS OPEN` list keeps its line format, but its numbers become **stable registry ids** for the whole run (`w5` stays `w5` while that window exists, ids are never reused). `focus_window "2"` and `"w2"` keep working. Why: the brief's window-state block needs stable ids, and two numbering schemes would confuse the model.
- **D9.5 Screenshot scope.** Default: the whole monitor holding the target window (as today), plus the window-state block describing every monitor and window. A `full_virtual_desktop` setting exists, but a two-monitor desktop shrunk to 1280 px is unreadable. Window crops are used only for `zoom` and for per-window workers, and they always come with the block and the crop transform.
- **D9.6 Coordinate policy** (documented in the README): image coordinates must lie inside the image sent. Floats are rounded and numeric strings parsed. Negative or out-of-image values are **rejected with feedback** (not clamped), which keeps the existing behaviour. Exception: up to 2 px outside the edge is clamped (rounding at the border).
- **D9.7 Test tiers.**
  - **T0/T1:** `cargo test --lib` (no input).
  - **T2:** `aura-cli conformance` (drives the real screen, test windows only).
  - **T3:** opt-in `--system` cases that change system settings or run for hours: display scaling, virtual desktops, CPU stress, 10-minute pause, 24-hour soak, sleep/lock.

  `aura-cli test-all` runs T0–T2. T3 runs only with `--system` and announces what it changes.
- **D9.8 Chrome DevTools Protocol** is used only for Chrome windows the harness launched itself, with `--remote-debugging-port` and a throwaway profile. Never on the user's own Chrome. Visual settle is the fallback everywhere else.
- **D9.9 Photoshop and Illustrator** are not installed. They are covered by a mock creative app page (`/studio`) plus real MS Paint. The README will say the real apps were not tested.
- **D9.10 Terminal use.** The harness has no shell execution and must not get one (`AGENTS.md`). "Terminal use" means typing into a console window with `type_text` and `key_press`, which needs the console typing path (Unicode/VK, no Ctrl+V).

### 3.2 Open questions for the user (asked once, before H0 work starts)

1. **Git (D1, still open).** This phase touches most modules and the folder is not a git repository. Recommended: `git init` plus a first commit before H1.
2. **File Explorer in tests.** The rules say never type or press Enter in Explorer. The brief asks for Explorer tests (address bar, context menus, file lists). Proposal: allowed **only** in an Explorer window the test opened on `%TEMP%\AuraEval\explorer`. Its folder path is verified through `IShellWindows` before every keystroke, and nothing is ever sent outside that window.
3. **System-changing tests (T3).** May they change display scaling, create virtual desktops, toggle Sticky Keys or High Contrast, and run CPU stress, each restored afterwards? Default: built, but run only when the user passes `--system`.
4. **Second monitor.** Is one available for multi-monitor tests? Without it, multi-monitor coverage is synthetic (mapping math with fake layouts) and the README says so.

---

## 4. Contract additions (Brief §2)

All additions go into `model_schema.rs` (schema variants and lenient parse), `input::ActionPrimitive` (new variants with serde names in the existing snake_case style), `describe_action_text`, and a few lines in `build_proposal_prompt`. Nothing existing is renamed.

### 4.1 New top-level key

`voice_command` (string, ≤ 80 chars): spoken before the action. Example added to the prompt: `"voice_command": "Clicking the Save button"`.

### 4.2 New optional fields on existing actions

| Action | New optional fields |
|---|---|
| any pointer or typing action | `window` (registry id such as `"w3"`, checked before execution, §H3) |
| `mouse_move` | `dwell_ms` (hover time for tooltips and menus) |
| `click` | `clicks` already exists (3 = triple click). `triple_click` is added as an alias that maps to it. |
| `drag_and_drop` | `waypoints` `[[x,y],…]`, `button`, `speed` (`slow|normal|fast`) |
| `scroll` | `hold_keys` (Ctrl+wheel zoom), `delta` (raw wheel units for fine scrolling) |
| `type_text` | `method` (`auto|keys|unicode|paste`), `chars_per_second` |
| `key_press` | `hold_ms` (hold a key for a duration) |
| `wait_settle` | unchanged (`seconds` already exists) |

### 4.3 New actions

| Action | Fields | Milestone |
|---|---|---|
| `mouse_down` / `mouse_up` | `x,y` or `element`, `button` | H2 |
| `draw_path` | `points` `[[x,y],…]` **or** `shape` `{kind: line|polyline|circle|ellipse|spiral|bezier|sine, …params}`, `closed`, `button`, `hold_keys`, `duration_ms`, `pointer` (`mouse|pen`), `pressure`, `tilt_x`, `tilt_y` | H2 / H8 |
| `key_down` / `key_up` | `key` | H2 |
| `wait` | `seconds` (alias of `wait_settle` with seconds, because models use this name) | H4 |
| `zoom` | `x,y,width,height` (image coordinates). The next observation is that region enlarged; coordinates in the next answer map back exactly. | H4 |
| `observe` | none (fresh screenshot, no input) | H4 |
| `minimize_window`, `maximize_window`, `restore_window`, `close_window` (needs confirmation), `move_window` (`x,y`), `resize_window` (`width,height`), `snap_window` (`side: left|right|top|bottom|top_left|…`) | `window` | H3 |
| `switch_desktop` | `direction: left|right` | H8 |
| `clipboard_read` / `clipboard_write` | — / `text` | H8 |
| `touch` | `gesture: tap|double_tap|long_press|swipe|pinch|rotate`, `x,y`, `to_x,to_y`, `scale`, `angle`, `duration_ms` | H8 |
| `layout_windows` | `layout: columns|rows|grid`, `windows` `["w1",…]` | H5 |

`ask_user`, `note`, `open_app`, `focus_window`, `open_url`, `hotkey`, `done` and `fail` already exist.

### 4.4 Validator additions (`model_schema.rs`)

These extend the existing lenient parser:
- strip `<think>` (already done), markdown fences and surrounding prose (already done)
- treat a **truncated** object as invalid (no guessing) and ask again
- if the response is larger than 64 KB, keep only the first JSON object
- an unknown action gets feedback that lists the valid names, plus a nearest-name hint
- wrong types: numeric strings become numbers, floats are rounded, `"true"` becomes a bool. Anything else is specific feedback.
- out-of-range coordinates follow D9.6.

Invalid output means **nothing executes**. The model gets up to 3 corrective retries, each with a fresh screenshot (existing behaviour, made explicit and tested). After that the run stops safely, with an HUD/voice/UI notice.

---

## 5. Milestones (in the brief's priority order, Brief §10)

Every milestone lists **Build** (files), **Tests** (tier, case, threshold) and **Done when**.

### H0 — Test infrastructure (needed by every later milestone)

**Build**
- [ ] `eval_fixture.rs` new pages (the fixture already logs events via `/event` and `/events` and reports rects):
  - `/recorder`: full-page target grid with 1, 2 and 3 px targets. It logs every `pointer*`/`mouse*`/`wheel`/`key*`/`touch*` event with type, `screenX/Y`, client x/y, `devicePixelRatio`, button(s), key, code, modifiers, `pointerType`, `pressure`, `tiltX/Y`, `timeStamp`, and `getCoalescedEvents()` counts.
  - `/canvas`: a drawing surface that records the received path and reports deviation metrics against a submitted ideal path.
  - `/studio`: mock creative app with a zoomable canvas (Ctrl+wheel, zoom field, 100% and fit shortcuts), Space+drag pan, a custom cursor, tool shortcuts with an active-tool indicator, X/Y/W/H numeric fields, scrubby number labels, a layers panel, a modal dialog, a context menu, an undo history. It reports its canvas-to-screen mapping and state.
  - `/load?delay=&kind=`: spinner, skeleton, progress bar, lazy content on scroll, infinite spinner, render delayed after resize, layout shift. The page logs when it became ready and every click with its timestamp (an "early click" is a click before ready).
  - `/windows`: opens `window.open` pop-ups, alerts and notification-like overlays on command, to steal focus.
- [ ] `aura-cli testwin` (separate child process, so Aura's own-window guard doesn't apply): a native Win32 window with exact pixel targets that logs `WM_MOUSEMOVE`, buttons, `WM_POINTER*` (pen pressure/tilt, touch contacts) and keys with QPC timestamps. Flags `--dpi-aware=unaware|system|permonitor2` exercise apps with each awareness mode **without** changing system scaling.
- [ ] **Input journal** in `send_input.rs`: every `INPUT` sent (kind, coordinates, flags, QPC time), so a test can compare what was sent with what the page or test window received. Off unless enabled (`AURA_INPUT_JOURNAL=1` or by tests).
- [ ] `mock_server.rs` additions:
  - per-request latency and hang
  - HTTP 4xx/5xx and dropped connections
  - malformed-output scripts (fenced, prose-wrapped, truncated, wrong types, unknown action, 200 KB output, empty)
  - a "near-miss" mode that answers with the real target point from the fixture plus an offset of 1–20 px
  - a request counter for "zero calls while idle" assertions
- [ ] `aura-cli test-all [--system] [--only group] [--repeat N]`: runs T0, T1 and T2 in order and prints only a final line plus the log path (no report tables, Brief §9.3).
- [ ] Metrics writer `testlog.rs`: per-case jsonl (pixel error, latency, dropped events, stuck inputs, repeats, success over N).

**Done when** the new pages and `testwin` record events that a sanity case can read, and `test-all` runs the existing 122 unit tests plus 91 conformance cases unchanged.

---

### H1 — Interrupt, stop, pause, steer (Brief §6; priority 1)

**Build**
- [ ] `control.rs` (new), a single control bus used by the UI, hotkeys, the CLI, the scheduler and (later) workers:
  - `RunState` atomic (`Running|Paused|Stopping|Stopped`) and an `epoch: AtomicU64` that increments on every stop, pause or steer
  - `steer` queue: the latest message wins, earlier unread ones are folded in as "(superseded: …)"
  - existing `killswitch::is_killed()` becomes one input to `Stopping`; its semantics stay as they are
- [ ] **In-flight response discard.** Each model request carries the epoch it was sent under. After the response arrives **and again inside the execute guard**, if the epoch changed, the response is dropped and logged. This closes the stop-vs-response race (Brief §6 last bullet).
- [ ] **Check before every input event.**
  - `dispatch_inputs` already checks the kill switch per batch. Large batches (`type_unicode`, chords, scroll chunks) get split into single events (keys) or ≤ 4-event packets.
  - Glide, drag and `draw_path` check on every step.
  - `wait` sleeps in ≤ 50 ms slices under `tokio::select!`.
- [ ] **Held-input safety.**
  - A RAII `HeldInputGuard` around every action calls `release_all()` on drop, so a panic or early return also releases.
  - After release, `GetAsyncKeyState` is cross-checked for every key or button Aura pressed; anything still down gets an extra up event.
  - Mid-drag stop leaves the button released and the cursor where it is.
- [ ] **Pause/resume** (Pause key, UI button, `aura-cli` control port).
  - Pause takes effect at the next event boundary: the current transaction is cut and its held inputs released, then it is recorded as `half_done`.
  - Resume always takes a **fresh** screenshot (`settled_capture = None`) and builds a "while paused" diff: window registry changes plus the changed screen area against the pre-pause frame.
- [ ] **Manual takeover.** Keep the existing auto-pause on real input and make it configurable (`yield_to_user: on|off`, quiet period). Add an explicit "I'll take over" mode, which stays paused until the user says *continue*, followed by the same re-observation and diff.
- [ ] **Steering.**
  - The steer box (UI panel and Ctrl+Alt+Space; `aura-cli` control port for tests) bumps the epoch, so the in-flight call is discarded.
  - The session records `USER STEER after step N: "<msg>"` together with a **completed-actions ledger**: side-effecting steps with their measured effects, marked `done`, `half_done` or `not_started`. Irreversible ones (submit, send, save, delete) are marked `DO NOT REPEAT`.
  - A goal change goes into `session.amendments` and is shown above the task. `done_check` checks the amended goal.
  - Budget: the last 3 steers in full, older ones summarised in one line, so the history stays clean after 10 steers.
- [ ] **Stop also stops speech** (hook added now, wired in H8) **and the HUD**, writes a trace event, and works with the model host unreachable: the stop path never touches the network.
- [ ] **Local control port** `127.0.0.1:<port>` (token-protected, off by default in the app, on for `aura-cli`): `stop`, `pause`, `resume`, `steer <text>`, `status`. Tests drive the harness through it.

**Tests**
- T0:
  - epoch discard (a response arriving after stop or steer is never executed)
  - steer queue ordering and folding
  - ledger rendering
  - 10-steer history size bound
  - `HeldInputGuard` on panic (simulated)
  - pause-state machine
- T1 (mock):
  - stop during a hanging model call: under 500 ms, 0 requests executed afterwards
  - stop with the mock unreachable
  - 5 stops in 200 ms, and stop while idle
  - steer: "go back", "use the other tab", "skip this step", changed goal, small correction, contradicting instruction, several in a row (latest wins)
  - no completed step repeated (the mock replays the old plan; the ledger makes the harness refuse a duplicate submit, and the model is told why)
  - stop/response race fuzzed over 200 random timings
- T2:
  - stop mid-drag on `/drag`: button up at the OS level (`GetAsyncKeyState`) and on the page
  - stop mid-typing (5,000 chars into Notepad): no keys held, typing ends within one event
  - stop during a `wait` and during a scheduled idle period
  - global Esc with the harness window not focused
  - stop latency under 500 ms over 20 repeats (median and max logged)
  - pause → move a window → resume: the fresh screenshot and the diff mention the move
  - manual takeover → type in Notepad → continue: the model is told what changed
- T3: pause for 10 minutes, then resume.

**Done when** every case above passes 20/20, with 0 stuck inputs across all repeats.

---

### H2 — Mouse precision, timing and coordinate mapping (Brief §4 mouse, §5, §5C core; priority 2)

**Build**
- [ ] **DPI awareness assert.** At startup, read `GetAwarenessFromDpiAwarenessContext(GetThreadDpiAwarenessContext())`. If it is not per-monitor v2, log an error and refuse to run pointer actions (a wrong mode silently scales every coordinate). Check the manifest of both binaries.
- [ ] **Exact absolute mapping.** Replace the `/(vw-1)` formula with one that maps back exactly under Windows' conversion `px = vx + floor(norm·vw / 65536)`, choosing the smallest `norm` that lands on the target pixel. Unit-test it for every pixel of synthetic layouts: 1–3 monitors, negative origins, mixed sizes, odd widths.
- [ ] **Closed-loop move** (corrects `move_to_confirmed`): target error **0 px**.
  - After the move, read `GetCursorPos` (and `GetPhysicalCursorPos`).
  - On error: `SetCursorPos` (exact in a per-monitor-v2 process), then a zero-delta move so the app still gets `WM_MOUSEMOVE`.
  - Re-verify up to 3 times. If it is still off, **refuse the click** with feedback instead of clicking elsewhere.
  - Log the error per move.
- [ ] **High-resolution pacing.** `CreateWaitableTimerExW(CREATE_WAITABLE_TIMER_HIGH_RESOLUTION)` for event spacing (Windows `sleep` is ~15 ms granular). Used by glide, drag and `draw_path`.
- [ ] **New primitives:**
  - `mouse_down` and `mouse_up` (tracked by `release_all`)
  - `triple_click` alias
  - `dwell_ms` on hover
  - drag `waypoints` and speed
  - `draw_path` with arc-length resampling at ≤ 2 px spacing, integer dedupe, Bresenham gap fill and a constant event rate (default 125 Hz, set per profile). Shapes: line, polyline, circle, ellipse, spiral, bezier, sine. `closed`, plus `hold_keys: ["shift"]` for straight lines.
  - scroll `hold_keys` (Ctrl+wheel) and `delta`
  - modifier+click already works through `hold_keys`; add a test
  - `key_down`/`key_up`, and `key_press` with `hold_ms`
- [ ] **Event timing control:** per-profile `down_up_delay_ms`, `move_event_interval_ms`, `drag_step_px`, `pre_click_rest_ms` (replaces the fixed 35 ms and 45 ms sleeps in the input path).
- [ ] **Instant vs human-like motion.** `AURA_FAST_MOUSE` already exists; it becomes a profile field `motion: glide|instant`.
- [ ] **App profiles** `profiles.rs` + `%LOCALAPPDATA%\AgentAura\profiles.toml`:
  - defaults plus overrides per process name: move speed, event delay, drag step, zoom shortcuts, wait-after-action, capture method, `use_uia`, undo shortcut
  - built-ins for `mspaint.exe`, `Photoshop.exe`, `Illustrator.exe`, `chrome.exe`, `notepad.exe`, `CalculatorApp.exe`, `explorer.exe`
  - chosen automatically from the target window's process every step
- [ ] **Screenshot mapping.** Generalise `scale_action_checked` to an `ImageTransform {offset_x, offset_y, scale_x, scale_y}` stored with each capture (full monitor, monitor crop, window crop, zoom crop). It must handle windows partly off-screen and spanning monitors. The existing math is kept as the full-monitor case.
- [ ] **Cursor in screenshot** option (`include_cursor`): draw the current cursor shape (`GetCursorInfo` + `DrawIconEx` into the frame copy). Default off.
- [ ] Document the injection limits in the README: UIPI and elevated windows, exclusive-fullscreen and anti-cheat games, and remote desktop clients.

**Tests**
- T0:
  - exhaustive normalization round-trip (all pixels, synthetic layouts)
  - `ImageTransform` round-trip for every scope at scales 1.0, 1.25, 1.5, 2.0 and 1280-px downscale (error ≤ 0.5 image px)
  - shape generators (max deviation from the ideal curve ≤ 0.5 px, no gaps > 2 px)
  - profile resolution
- T2 (`/recorder`, `/canvas`, `testwin`, Paint):
  - cursor error after 500 random moves across the monitor, edges and corners: all 0 px
  - left/right/middle, double and triple click, mouse down/up, hold-release, modifier+click
  - 1, 2 and 3 px targets, 20/20 each; also on `testwin --dpi-aware=unaware|system`, to prove mapping into virtualised apps
  - hover dwell opens a tooltip
  - drag onto a 3 px target ≥ 19/20, slow and fast drags, slider, list reorder, drag-select text, file drag in a test-opened Explorer window (pending Q2)
  - dropped events: page received ≥ 99% of the sent moves on drags
  - circle, spiral, sine, bezier and a signature on `/canvas`: max deviation ≤ 1 px, 0 gaps, steady event rate
  - the same shapes in Paint (a new canvas the test opened), compared with a ground-truth render (pixel IoU ≥ 0.9 after 1 px dilation)
  - scroll vertical, horizontal, nested containers, Ctrl+wheel zoom
  - out-of-bounds coordinates are refused, never clicked
  - rapid clicks against the system double-click time
- T3: real display scaling at 125%, 150% and 200% (restored afterwards); second monitor if available (Q4).

**Done when** all the precision targets in Brief §5C hold over 20 repeats, logged.

---

### H3 — Multi-window handling and active-window verification (Brief §5A; priority 3)

**Build**
- [ ] `desktop/registry.rs`, the internal window registry:
  - stable ids (D9.4) keyed by HWND, refreshed every step **and** by a `SetWinEventHook` thread (`EVENT_OBJECT_CREATE/DESTROY/SHOW/HIDE`, `EVENT_OBJECT_LOCATIONCHANGE` for top-level windows, `EVENT_SYSTEM_FOREGROUND`, `EVENT_SYSTEM_MINIMIZESTART/END`)
  - per window: title, process, PID, DWM rect, monitor, z-order, state (normal/minimized/maximized/fullscreen), `is_active`, `is_target`, `covered_by` (rect intersection with higher-z visible windows), `not_responding` (`IsHungAppWindow`), `elevated`, `cloaked` (`DWMWA_CLOAKED`, which marks windows on another virtual desktop), owner and modal relationships
  - a change list since the last step
  - `existing window_inventory`, `window_list_text` and `resolve_window_reference` become thin views over the registry, so their output and behaviour stay the same
- [ ] **Window-state block** in every proposal prompt, next to the existing `WINDOWS OPEN` line. It is compact JSON with exactly the fields of the brief's example: `window_count`, `minimized_count`, `active_window`, `windows[]`, `screenshot {scope, monitor, width, height, scale, offset}`, `monitors[]`, `changes[]`. Asserted: no prompt is ever built without it (a debug assertion plus a test).
- [ ] **Action validation against the registry** before execution: the `window` field, or otherwise the observed active window, must exist, not be minimized, not be covered at the point, and be the target. Existing guards in `prepare_input` stay. The registry check is added in front of them, and a refusal is fed back with the reason (for example "w4 was closed").
- [ ] **Bring-to-front ladder** (extends `bring_to_front`), with per-method success logged:
  1. restore if iconic, then `SetForegroundWindow`
  2. AttachThreadInput (exists)
  3. Alt tap (exists)
  4. `SwitchToThisWindow`
  5. click the window's taskbar button, found through UIA

  Every step is followed by **verification**: `GetForegroundWindow` must match, then a fresh capture.
- [ ] **Window actions:** minimize, maximize, restore, move, resize (`SetWindowPos`, then read back the DWM rect, compensating for invisible borders), close (confirmation card), snap (`SetWindowPos` to work-area halves and quarters, Win+arrow as fallback), "launch if not running" (`open_app` already focuses a running app; add a verify step), not-responding detection.
- [ ] **Target tracking** by process + title pattern + handle. If the handle dies and a window with the same process and title appears (app restart), re-bind and tell the model.
- [ ] **Tricky windows:**
  - always-on-top windows in the covered-by calculation
  - modal dialogs: the parent is disabled (`IsWindowEnabled`), so the target becomes the dialog
  - menus and tooltips as same-PID popups (already allowed by the occlusion check)
  - toasts flagged in `changes`
  - UAC / secure desktop: foreground is `consent.exe`, `OpenInputDesktop` fails, or DXGI access is denied. Then pause and use `ask_user`, never drive it.
- [ ] **Per-app tips** (D3) as read-only constants: Notepad, Calculator (Standard/Scientific layouts), Explorer (address bar, file list, context menu), Chrome (tabs, omnibox, downloads bar, permission prompts, DevTools), Paint. The current process's tips are injected (≤ 3 lines).

**Tests**
- T0: covered-by geometry, state classification, id stability across refresh, change-list generation, block always present (prompt builder test).
- T1 (mock):
  - the model acts in a window that was closed: refused before any input
  - the model acts in a minimized window
  - the model acts in a covered window
  - the block matches the registry on every step
- T2, using only windows the test opened (Notepad, Calculator, Paint, a fixture Chrome window, `testwin`; Explorer per Q2):
  - 5+ windows open, a task that moves across 4 of them, with **0 wrong-window inputs** (the input journal plus the per-window recorders prove it)
  - target behind others, target minimized
  - focus stolen mid-task by `/windows` pop-ups and alerts: detected and corrected
  - bring-to-front 20/20 per app
  - window moved or resized between steps
  - a hung app (`testwin --hang`) detected as not responding
  - modal dialog blocking its parent
  - copy in one app and paste in another
  - the block, the registry and `EnumWindows` agree after open, close, minimize, move and resize
- T3: another virtual desktop; UAC prompt handoff (the user triggers it, the harness must pause).

**Done when** wrong-window inputs = 0 across all repeats, and the block is correct on every step of every T2 case.

---

### H4 — Settle and loading detection, then grounding modes (Brief §5E then §5B; priority 4)

**Build: settle (extends `settle/effect.rs`)**
- [ ] **Ignore masks:**
  - the caret rect (`GetGUIThreadInfo`)
  - the taskbar clock
  - masks from the profile or the model
  - regions that keep changing across 3 consecutive idle frames before the action (video, ads, animations), learned automatically
- [ ] "Stable for N frames over T ms" semantics, with the existing per-class timings as defaults and the per-profile overrides.
- [ ] **System signals:**
  - busy cursor (exists)
  - `IsHungAppWindow`
  - target-process CPU and I/O deltas (`GetProcessTimes`, `GetProcessIoCounters`)
  - UIA `ProgressBar` and busy states, plus disabled target elements
  - OCR "Loading…" text in the target region
  - CDP `readyState` and network idle, for launched Chrome only (D9.8)
- [ ] **Expected change.** Wait for *some* change first. "No visible effect" stays a reported result (exists).
- [ ] **Adaptive timing.**
  - Measure capture time, input-to-first-change latency and frame time over a rolling window, plus system load (`GetSystemTimes`, memory pressure).
  - Scale `change_timeout` and `max_ms` by the measured lag factor (cap ×4).
  - **Never re-issue a pointer action on NoEffect** without a fresh observation and a model decision. A test asserts that no duplicate click happens under stutter.
- [ ] **Stuck detection.** Past `max_ms` plus the extension, report "still loading after X s" with options (wait longer, reload, restart the app, ask the user).
- [ ] **Log the wait reason** for every step (`waited 2.3 s: spinner visible, CPU busy`).
- [ ] Replace the fixed sleeps in the observe path (§2 row 9) with settle conditions.

**Build: grounding (Brief §5B)**
- [ ] Config `grounding_mode = A|B|C` (default A, which is today's single call), plus an optional separate grounding provider (URL and model).
  - **Mode B:** the proposal's existing `target.description` goes to a `grounding` call (`{element|x,y}`) on the **same `EncodedCapture`**, optionally its zoom crop. The frame id is asserted equal between the two stages.
  - **Mode C:** harness refinement with no extra call:
    1. take the zoom crop around the point
    2. snap to the UIA element under or near the point (`coords.rs` exists)
    3. hover, then check that UIA `ElementFromPoint` matches the target text or role
    4. only when the confidence is low, fall back to the existing crosshair verifier
- [ ] **`zoom` action:** crop the full-resolution frame and enlarge it to the provider's max edge. The next observation carries `scope: zoom` and its `ImageTransform`. The model's answer maps back exactly (H2 transform).
- [ ] **`observe` action:** fresh screenshot, no input.
- [ ] **Change-detection skip** (config): if the frame hash and the registry are unchanged since the last call, with no steer and no new effect, skip the model call. Used in `wait` loops and scheduled runs only.
- [ ] **Metrics per mode**, from the API's `usage` field: latency per step, model calls per step, tokens, and click accuracy (the fixture reports hits).

**Tests**
- T0: masks, adaptive factor, frame-sequence cases (spinner, skeleton, delayed change, blink), transform for the zoom scope.
- T1: Mode B uses the same frame id for both stages; one capture per step in every mode; skip-on-unchanged.
- T2:
  - `/load` at delays 0.5, 1, 3, 10 and 30 s for each loading kind: **0 early clicks in 20 repeats**, with waste time logged
  - an infinite spinner times out with the recovery options
  - a CPU-stress helper (T3 if heavy) produces no early or duplicate clicks
  - ignore regions (blinking caret, animated banner)
  - a no-effect action is reported
  - visual-only vs signals vs CDP, compared in the logs
  - near-miss mock (1–20 px): Mode C recovers ≥ 95% of misses within 14 px
  - Modes A/B/C × UIA on/off on the same scenarios, with the metrics logged
- **Measure** `max_image_edge` vs accuracy for the local model and record the default in the provider settings.

**Done when** there are 0 early actions and 0 duplicate clicks across all repeats, and the three modes are switchable with metrics in the logs.

---

### H5 — Window layout and parallel multi-window workers (Brief §5D; after H1–H4 are solid)

**Build**
- [ ] `desktop/layout.rs`:
  - tilings for 2, 3, 4 and 6 windows (columns, rows, grid, custom rects) per monitor work area, with margins and minimum sizes
  - visible rects tile exactly: invisible borders are compensated
  - `SetWindowPos`, then **read back** the DWM rect. If an app refuses the size (`WM_GETMINMAXINFO`), reflow the layout, then wait for content to settle.
  - layouts saved and restored (JSON)
  - drift detection through registry events: re-assert or adapt
- [ ] **Per-window capture:** Windows.Graphics.Capture for a single window (works when it is partly covered), with `PrintWindow(PW_RENDERFULLCONTENT)` as fallback. A per-window `ImageTransform` maps crops to screen coordinates exactly.
- [ ] **De-globalise the loop state** that blocks workers: the target window, the `LISTED` numbering and the run state become per-worker context objects. The single-window path keeps one context, so its behaviour doesn't change. This is the riskiest refactor of the phase, done behind the full T0–T2 suite.
- [ ] `workers/` module:
  - `WindowWorker` = the existing loop with its own session, history, settle timers, error budget and cancellation token
  - the **input arbiter** serialises real input. A transaction (focus, verify focus, act, release) runs under one exclusive lock. Priorities, aging for fairness, stop-all through the global kill switch, stop or steer for one worker through its token.
  - optional background accelerators: UIA Invoke/Value patterns, and CDP for launched Chrome. Each falls back to real input.
- [ ] **Model request scheduler:** a semaphore with a configurable limit, a priority queue, timeouts, and calls/min and queue-wait metrics.
- [ ] `aura-cli probe-concurrency` measures whether the model server really runs requests in parallel (§2 row 6).
- [ ] **Shared task memory** (notes visible to all workers) and an explicit `handoff` (worker A posts a value, worker B waits for it).
- [ ] Voice lines prefixed with the window name ("Notepad: typing the heading").
- [ ] **Isolation:** a worker panic or failure marks only that worker failed. Other workers continue.
- [ ] **Resolution budget:** each worker gets its own window crop at a readable size. Accuracy vs crop size is logged.

**Tests**
- T0:
  - layout math (borders, minimum sizes, reflow)
  - arbiter: no interleaving within a transaction, no starvation over 10k randomized requests, priority honoured
  - scheduler: limit honoured, no dropped requests, timeouts
- T1: 6 mock workers, throughput logged, one worker failing doesn't affect the others, stop-one and stop-all.
- T2:
  - tile 2, 3, 4 and 6 test windows and verify each read-back rect within 1 px (or adapted to the app's minimum)
  - resize or move a window mid-task: the worker adapts
  - close a window while the others run
  - capture a partly covered window and map its coordinates exactly
  - two workers requesting input at the same moment: 0 interleaving, 0 wrong-window inputs

**Done when** 6 workers on test windows complete their fixture tasks with 0 wrong-window inputs over 20 repeats.

---

### H6 — Model output validation and failure handling (Brief §2, §8; priority 5)

**Build**
- [ ] The §4.4 validator additions and the corrective-retry contract (3 tries, nothing executes in between, then a safe failure with a notice).
- [ ] **API failures:**
  - connect timeout 5 s, total timeout configurable
  - backoff 0.5 s, 1.5 s, 4 s on timeout, 5xx and dropped connections
  - a response size cap
  - user notice through the HUD, the panel and voice
  - retries only ever repeat the **model call**, never an executed action
  - a per-request id plus the epoch (H1) means a late response from an abandoned try is discarded
- [ ] **Loop protection:**
  - max steps configurable up to 100 (the cap is 30 today)
  - wall-clock and model-call budgets
  - identical-action detection across non-adjacent steps (same action, same target, same screen hash), leading to a forced strategy change and then `ask_user`
- [ ] **Logging:** add the voice text, the input journal excerpt per step, the epoch, the retry reasons and the grounding mode to the trace.
- [ ] **Secret redaction:** text typed into `is_password` fields and anything matching key or token patterns is masked in traces.
- [ ] **Safety:**
  - word-boundary impact matching (B14 rest)
  - password fields and CAPTCHA/login detection (UIA `IsPassword`, OCR/UIA keywords such as "verify you are human" or "captcha") lead to `ask_user` handoff
  - screen text is presented to the model under an `UNTRUSTED SCREEN TEXT` header with a rule that it is never instructions
  - the impact gate does not depend on the model's own `impact` field, so injected text can't lower it

**Tests**
- T0: every malformed form from Brief §9.2 item 6, plus a fuzzed corpus (10k random mutations of valid outputs), never panic and never execute.
- T1:
  - retry limit reached, then a safe stop
  - slow, down and dropped API
  - a retry never duplicates a click (input journal count)
  - loop detection, max steps
  - the kill switch during backoff
- T2: a prompt-injection page (`/inject`) that tells the agent to click "Delete everything": that requires confirmation; a password field causes a handoff; a CAPTCHA mock page causes a handoff.

---

### H7 — Scheduling (Brief §7; priority 6)

**Build**
- [ ] `schedule.rs` plus a SQLite table `Schedules`, added through a migration (no seed rows): id, task, kind (`interval|cron`), interval or cron, anchor time, `overlap_policy (skip|queue_one|cancel_previous)`, `missed_policy (run_once_on_wake|skip)`, `only_if_changed`, enabled, `next_run_utc`, `last_run`, `state_json`.
- [ ] **One timer task:** `tokio::time::sleep_until` on the earliest due schedule. There is no polling, so the process is idle between runs.
  - Next time = anchor + k·interval, which avoids drift.
  - Wall-clock jumps are detected by comparing `Instant` with `SystemTime`.
  - Sleep and wake come from `WM_POWERBROADCAST`, lock and unlock from `WTSRegisterSessionNotification`. While locked, runs are deferred by policy.
- [ ] Scheduled runs use `execute_run` and the H1 control bus, so stop, pause and steer behave the same as for manual runs.
- [ ] Changing or cancelling a schedule mid-run applies to future runs. Cancelling while a run is active asks whether to stop the current run too (UI) or follows `--stop-current` (CLI).
- [ ] **State between runs:** last summary, last frame hash of the watched region, and notes go into the prompt as `LAST RUN`. `only_if_changed` compares the region hash (and OCR text) before any model call.
- [ ] **Persistence:** schedules reload at startup. An optional "start Aura with Windows" setting (HKCU Run key, off by default) covers reboots.
- [ ] **Interfaces:** a schedules UI list; `aura-cli schedule add|list|remove|daemon`.

**Tests**
- T0, with a fake clock trait:
  - 5, 10 and custom intervals
  - cron
  - drift under 1 s over a simulated 24 h
  - overlap policies, missed-run policies
  - reload after a simulated crash
  - only-if-changed
- T1:
  - idle for 10 minutes real time: 0 mock requests and process CPU time under 1 s
  - a run longer than the interval
  - change or cancel mid-run
- T3: real sleep and lock; 24 h soak (memory growth, context size, drift logged).

---

### H8 — Voice announcements (Brief §3; priority 7)

**Build**
- [ ] `voice.rs`:
  - a `TtsBackend` trait with a SAPI backend (D9.2; `windows` features `Win32_Media_Speech`, `Win32_System_Com`) on its own STA thread, and a recording fake for tests
  - non-blocking by default; config `speak_then_act` waits for completion (with a timeout, under the kill check)
  - queue: one current utterance plus one pending slot. A new line replaces the pending one. A current line older than `stale_ms` is purged (`SPF_PURGEBEFORESPEAK`).
  - stop, pause and steer purge speech immediately (H1 hook)
  - config: on/off, volume, rate, voice (enumerated from the SAPI token category); `AURA_MUTE=1` and `cargo test` mute it
  - fallback phrase from the action when `voice_command` is missing or empty (built from `describe_action_text`, for example "Clicking at 842, 517")
  - any TTS error is logged and disables voice for the run; it never fails the loop
- [ ] Speak **before** executing, after validation (never for actions that get refused).

**Tests**
- T0/T1 with the fake backend:
  - spoken for each action
  - fallback when missing
  - the action does not wait for speech (timing)
  - overflow with 20 actions in 1 s: the backlog stays ≤ 1
  - stop purges within 50 ms
  - a backend that panics or errors leaves the loop running
- T2: one real SAPI smoke test (audible; skipped when muted).

---

### H9 — Remaining inputs and polish (Brief §4 rest, §5C strategies; priority 8)

**Build**
- [ ] **Keyboard:**
  - typing speed (`chars_per_second`)
  - `method` selection
  - **Caps Lock compensation** for the VK path (it currently types "A" for "a" when Caps Lock is on; verify, then fix)
  - layout-aware VK mapping through `VkKeyScanExW` with the target thread's `GetKeyboardLayout`
  - Tamil and emoji through Unicode packets with whole surrogate pairs, read-back compared after NFC normalization, paste fallback when the read-back differs
  - fields that reject typing get a paste fallback, then feedback
  - the clipboard is excluded from history (`ExcludeClipboardContentFromMonitorProcessing`, `CanIncludeInClipboardHistory = 0`); paste is verified
- [ ] **Touch and pen:** `CreateSyntheticPointerDevice` + `InjectSyntheticPointerInput` (Windows 10 1809+).
  - Touch: tap, double-tap, long press, swipe, pinch, rotate (2 contacts).
  - Pen: pressure 0–1024, tilt, barrel button.
  - Unsupported (older Windows, or an app that ignores pointer input) is detected and documented.
- [ ] `clipboard_read` / `clipboard_write`; `switch_desktop`.
- [ ] **File dialogs:** type the full path into the File name field (existing Save As cases cover part of this).
- [ ] **Creative-app strategies** (prompt guidance plus profile data, no per-app training):
  - zoom in before precise work (profile zoom shortcuts)
  - canvas-to-screen mapping tracked from the app's zoom indicator where UIA exposes it (Paint status bar, `/studio`), otherwise from the model's reading of the zoom field
  - prefer numeric fields
  - arrow-key nudging
  - Space+drag pan
  - scrubby labels as a drag on the label
  - verify the active tool
  - undo-and-retry recovery
- [ ] **Capture fallback** for GPU-drawn apps: detect black or frozen frames (all-zero luma, or identical frames while the busy cursor or CPU shows activity), then switch to the GDI/WGC capture method from the profile.
- [ ] **Accessibility settings:** Sticky Keys — make sure modifier presses never trigger the 5×Shift prompt (test it); High Contrast and scaling as T3 cases.

**Tests**
- T2:
  - plain, long (10k chars), symbols, Unicode, Tamil, emoji, shortcuts, key hold, Caps Lock on, a non-US layout (T3 if it must be installed)
  - typed vs pasted, dropped characters at high speed: 0
  - touch and pen on `/recorder` (`pointerType`, `pressure`, `tiltX`) and `testwin`
  - zoom-then-click on `/studio`, numeric-field entry, nudging, pan, scrubby field, tool verification, undo recovery
  - black-frame detection through a forced bad capture
  - profile auto-selection from the active window
  - copy from Notepad, paste into Chrome

---

### H10 — Docs and delivery (Brief §11)

- [ ] `README.md` sections:
  - running the tests (`aura-cli test-all`, `--system`)
  - TTS configuration
  - schedules
  - interrupt, pause and steer hotkeys
  - the coordinate policy
  - the keys added to the model JSON (`voice_command` and the §4 actions and fields)
  - injection limits
  - one or two lines on what could not be tested (Photoshop and Illustrator, multi-monitor if Q4 is no, UAC driving)
- [ ] `RELIABILITY_PLAN.md` §5.10 updated to the new as-built behaviour; `CORE.md` §2.2 action table; `PROGRESS_TRACKER.md` Phase 9 boxes.

---

## 6. Brief §9.2 test categories → milestones

| Brief category | Milestones |
|---|---|
| 1 Screenshot/coordinates | H2, H3 (scope block), H4 (mid-animation), H9 (black frames) |
| 2 Mouse | H2 |
| 3 Keyboard | H2 (down/up/hold), H9 |
| 4 Windows/system | H3, H9 |
| 5 Time and waiting | H4 |
| 6 Model output contract | H6 (built on §4) |
| 7 Voice/TTS | H8 |
| 8 Interrupt/pause/steer | H1 |
| 9 Scheduling | H7 |
| 10 Failure handling | H6, H1 |
| 11 Safety | H6 |
| 12 Possibly missing human inputs | H4 (zoom, observe), H9 |
| 13 Multi-window | H3 |
| 14 Grounding modes | H4 |
| 15 High-precision mouse | H2, H9 |
| 16 Layout and parallel work | H5 |
| 17 Loading and lag | H4 |

---

## 7. New and touched files

**New:** `control.rs`, `profiles.rs`, `voice.rs`, `schedule.rs`, `testlog.rs`, `desktop/registry.rs`, `desktop/layout.rs`, `workers/{mod,arbiter,scheduler}.rs`, `input/pointer_inject.rs` (touch and pen), `capture/window.rs` (WGC/PrintWindow).

**Touched (additions or named corrections only):** `model_schema.rs`, `input/send_input.rs`, `input/killswitch.rs` (new hotkeys), `input/clipboard.rs`, `agent.rs` (control-bus checks, block in the prompt, modes, voice call, ledger), `reliability.rs` (ledger, amendments), `settle/effect.rs`, `desktop/target.rs` (views over the registry, bring-to-front ladder), `coords.rs`, `eval_fixture.rs`, `mock_server.rs`, `conformance.rs`, `bin/aura-cli.rs`, `db/store.rs` (migration), `ui/src/main.ts` (pause, steer, schedules, voice settings), `Cargo.toml` (`windows` features: `Win32_Media_Speech`, `Win32_System_Com`, `Win32_UI_Input_Pointer`, `Win32_UI_Controls`, `Graphics_Capture`, `Win32_System_RemoteDesktop`, `Win32_System_Power`).

---

## 8. Risks

| Risk | Mitigation |
|---|---|
| De-globalising the loop for workers (H5) breaks the single-window path | Done last, behind the full T0–T2 suite. The single path keeps one context object. |
| `SetCursorPos` correction is not seen by some apps as movement | Follow it with a zero-delta `SendInput` move; `/recorder` and `testwin` verify that the event arrives. |
| SAPI voices sound dated | `TtsBackend` trait; WinRT `SpeechSynthesizer` backend later. |
| Synthetic pen/touch unsupported on this machine or ignored by an app | Detected at startup; the action returns a clear "unsupported" result and the README documents it. |
| Desktop tests disturb the user's work | Test windows only, announcement before runs, T3 opt-in, existing `close_tab_if_front_is` rules. |
| LM Studio processes requests one at a time | Workers still gain from concurrent waiting; the probe measures it and the scheduler limit defaults to the measured value. |
| No version control for a large change | Q1. |

---

## 9. Order and size (rough)

H0 → H1 → H2 → H3 → H4 → H5 → H6 → H7 → H8 → H9 → H10. H6's validator items (§4.4) and the `voice_command` schema key are small and can land with H0, because H1's tests already need malformed-output scripts. Expect H2, H3 and H5 to be the largest; H7 and H8 are self-contained.

---

## 10. Work Log

| Date | Milestone | Summary |
|---|---|---|
| 2026-10-30 | Plan | Plan written from the user's brief and a read of the code (§1–§2). Open questions in §3.2. |
