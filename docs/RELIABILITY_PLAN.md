# Agent Aura — Reliability & Intelligence Overhaul (`RELIABILITY_PLAN.md`)

**Document Version:** 1.1 (2026-10-30; sections 5.10, 12 and the milestone checkboxes brought up to date with the code)
**Phase:** 8 — Reliability & Intelligence Overhaul (active)
**Supersedes:** The "Launch & Maintenance" pending items in `PROGRESS_TRACKER.md`.
**Audience:** Coding agents (primarily Claude Code) continuing this work across sessions. Read this after `PROGRESS_TRACKER.md` and before touching code.

---

## 0. How To Use This Document

1. Find the first milestone in §6 with unchecked items, and work only on that milestone.
2. Before you change a module, read its entry in the bug ledger (§3) and the target design (§5).
3. When a milestone is done, run `cargo test --lib` from `src-tauri/`, tick the boxes here *and* in `PROGRESS_TRACKER.md` Phase 8, and add a dated line to the log in §11.
4. If the code disagrees with this document, the **code is the truth**. Fix the document in the same session.
5. Do not tick a box unless the feature is wired into the Rust runtime path (`run_autonomous_task`). The Python code in `engine/` does not count. It is not called from Rust.

---

## 1. Goal

Make Agent Aura **reliable in every common Windows scenario** and **smarter over time**. Concretely:

| Objective | Measurable target (on the §6 M7 eval suite) |
|---|---|
| Never acts on the wrong window or tab | 0 actions delivered to a non-target window. The foreground guard and occlusion check catch 100% of injected test interruptions. |
| Screenshots are always clean and fresh | Aura UI and HUD never appear in model images. Every post-action image comes from a frame presented after the action. |
| High task success with a small local model (`google/gemma-4-e2b`) | ≥ 60% on the "basic" suite. |
| High task success with a capable model | ≥ 85% on "basic", ≥ 60% on "intermediate". |
| No runaway or stuck states | 0 stuck modifier keys or mouse buttons after a stop. Esc stops within 100 ms, even mid model request. |
| Gets better with repetition | The 2nd run of an approved task uses ≥ 40% fewer model calls and ≥ 30% less wall time than the 1st. |
| Overhead per step, excluding model latency | ≤ 400 ms median (capture + perception + encode + guard). Settle time is reported separately. |

---

## 2. Current State (audit, 2026-10-29)

### 2.1 What actually runs
The production loop is entirely `src-tauri/src/agent.rs::autonomous_loop`:

```
minimize Aura → sleep 250ms → capture_screen() [new DXGI device, PNG, UIA+OCR]
→ propose (model call 1) → verify (model call 2) → override coords with text-matched candidate
→ SendInput → wait_for_visual_settle() [new DXGI device] → capture_screen() → repeat
→ (only at step limit) final verification (model call 3)
```

### 2.2 Documented but not in the Rust runtime
The following are ticked in `PROGRESS_TRACKER.md` but exist only in the unwired `engine/*.py`, or not at all:
- Tier 3 vector cache, mid-task state snapping (Triple-Lock), and trajectory distillation (`engine/cache_manager.py`, `engine/skill_compiler.py`).
- Action batching (the Rust loop proposes exactly one action).
- Humility gate (ask the user for one click).
- Destructive confirmation gate (Rust aborts the step instead of asking).
- WebP encoding (Rust encodes PNG into fields named `webp`).
- "2 ms radar" (the brute-force NCC takes hundreds of ms per anchor).
- Direct2D overlay (the overlay is actually a CPU DIB pushed with `UpdateLayeredWindow`).

**Status 2026-10-30:** the confirmation gate now exists as an Allow/Deny card, a humility path exists (`ask_user` plus a stuck-streak guard), and screenshots sent to the model are JPEG (traces keep PNG). Still missing: action batching, the Tier 3 vector cache, Triple-Lock snapping, distillation beyond plain-language recipes, a fast radar and a real Direct2D overlay.

### 2.3 Real strengths to keep
Typed action grammar (no `exec`), `SendInput` with virtual-desktop normalization, multi-monitor origins, UIA + Windows OCR candidates, the kill-switch hook, the HUD, memory-only API keys, persisted runs and trajectories, the review modal and scrubber UI, and a single Rust binary.

### 2.4 Local test environment (current)
- **Model server:** LM Studio on the LAN (`http://10.103.7.32:1234/v1` on 2026-10-30; the address changes, ask the user). Models seen: `google/gemma-4-e2b` (the test model), plus others the user may name. LM Studio rejects `response_format: {"type":"json_object"}` (HTTP 400); `json_schema` works and constrains the output.
- **Build:** working. MSVC toolchain pinned by `rust-toolchain.toml` (VS 2022 Build Tools with the C++ workload and Windows SDK). `cd src-tauri; cargo test --lib` passes (115 tests, 10 opt-in live tests ignored). The old GNU workaround is kept as `src-tauri/.cargo/config.toml.gnu-disabled` and is not used.
- **Version control:** the repo is not a git repository (decision D1 still open).
- **Scratch folders the user should delete:** `src-tauri/target-llvm`, `.target-test`, `C:\aura-target`, `C:\aura-target2`.

---

## 3. Bug Ledger

Each ID is referenced by the milestones in §6. Severity: **P0** breaks runs, **P1** frequent wrong behaviour, **P2** quality or performance.

| ID | Sev | Where | Defect | Fix milestone |
|---|---|---|---|---|
| B01 | P0 | `agent.rs` `request_model` (`response_format: json_object`) | LM Studio returns HTTP 400, so every local-model call fails. | M1 |
| B02 | P0 | `input/killswitch.rs` hook proc | Esc keystrokes injected by Aura itself trigger the kill switch (no `LLKHF_INJECTED` check). | M1 |
| B03 | P0 | `input/send_input.rs` `dispatch_inputs` | Key-up and mouse-up are refused after a kill, leaving Ctrl/Shift/Win or the left button stuck. | M1 |
| B04 | P0 | `agent.rs` `propose_and_verify` prompt input | UIA/OCR candidates are given in **physical** coordinates while the model must answer in **image** coordinates. | M1 (stopgap), M4 (element IDs) |
| B05 | P0 | `agent.rs` after `scale_action_checked` + `reliability.rs::best_grounding_candidate` | The verifier's corrected action is overwritten by a token-overlap match on the *proposal* target. Candidates include off-screen and other-window elements. | M1 |
| B06 | P1 | `agent.rs` (minimize/hide/show/set_focus around steps) | Aura minimizes itself, so Windows activates an arbitrary next window. Focus thrashes, and actions and UIA target the wrong window. | M2 |
| B07 | P1 | overlay + main window | The HUD glow and halo cursor are captured into model screenshots (no `SetWindowDisplayAffinity`). | M2 |
| B08 | P1 | `agent.rs::capture_screen`, `capture/dxgi.rs` | A new D3D device and duplication on every capture. Monitors probed by creating duplications. Default adapter only. Fresh-duplication first frame may be black or stale. ACCESS_LOST recursion is unbounded. | M2 |
| B09 | P2 | `agent.rs::capture_screen` | PNG plus Lanczos3 on the full frame, twice per step, sent twice per step, and also emitted to the UI. | M2 |
| B10 | P1 | `settle/quiescence.rs` | 64×36 nearest-neighbour mean diff misses small changes. "No change yet" counts as settled, so it screenshots before the UI reacts. | M3 |
| B11 | P1 | `agent.rs` retry loop | Retries reuse the same screenshot with no error feedback, so at temperature 0 they repeat the same failure. | M5 |
| B12 | P1 | `agent.rs` | The kill switch is not observed during HTTP calls (60 s timeout). | M1 |
| B13 | P1 | `agent.rs` terminal handling | `done` is accepted without any verification. | M5 |
| B14 | P1 | `reliability.rs` impact gate | Unauthorized impact makes the step fail instead of asking the user. Substring matching gives false positives ("remove filter", "postcode"). | M5 |
| B15 | P1 | `reliability.rs` structs (`deny_unknown_fields`) | Any extra field from a small model is a parse failure. | M1 |
| B16 | P2 | `perception.rs::collect_uia_candidates` | Walks the desktop root filtered by PID, depth 14, no visibility or enabled filter, first 120 in tree order, no time budget. | M4 |
| B17 | P2 | `perception.rs::collect_ocr_candidates` | Full-monitor OCR on every capture, including post-action captures. | M4 |
| B18 | P1 | `agent.rs::submit_task_review` | The golden path includes failed steps. It stores raw physical coordinates and is never replayed, only pasted as a hint. | M6 |
| B19 | P2 | `radar/tracker.rs` | Brute-force NCC over a full-frame grayscale conversion per anchor. Anchors are absolute coordinates, so they break on resolution, DPI or window moves. | M6 |
| B20 | P2 | `input/clipboard.rs` | Overwrites the user's clipboard with no restore. Pasted text goes into clipboard history. No paste verification. Ctrl+V fails in consoles. | M5 |
| B21 | P2 | `agent.rs` | Step limit is clamped to 30. History is text only, with no record of what changed after an action. | M5 |
| B22 | P2 | `perception.rs::foreground_window_context` | `GetWindowRect` includes the invisible DWM shadow border, so crops and rects are off by about 7 px. | M2 |
| B23 | P2 | docs | `CORE.md`, `FEATURES.md` and `PROGRESS_TRACKER.md` describe features that are not implemented. | M7 |

**Status 2026-10-30:** fixed: B01, B02, B03, B04, B05, B06, B07, B10, B11 (retries carry specific feedback and a fresh screenshot), B12, B13 (completion check on every `done`), B14 (card instead of failure; substring false positives reduced), B15, B20 (clipboard restored after a paste; no history exclusion or paste verification yet), B21 (history keeps measured effects), B22, B23 (this refresh). Partly fixed: B09 (model images are JPEG; trace and UI images are still PNG), B16 (UI Automation starts at the window handle and OCR is clipped to the active program; no time budget, none needed at about 0.4 s per observation), B17 (OCR still reads the whole monitor, then drops text that is not on the active program), B18 (successful runs are saved as plain-language recipes; no guided replay). Open: B08 (a new D3D duplication per capture; reuse was measured and judged not worth the risk, because a full observation takes about 0.4 s), B19 (radar).

---

## 4. Design Principles

1. **Deterministic first, model second.** Anything the OS can tell us exactly is never asked of the model. That includes element positions (UIA), whether something changed (frame diff), which window is in front, and whether text landed (ValuePattern). The model decides *what* to do. Rust decides *where* and *whether it worked*.
2. **Point at things by element ID, not pixels.** The model picks numbered elements (Set-of-Mark). Raw image coordinates are the fallback, and Rust converts and validates them.
3. **Constrained outputs.** Every model call uses `json_schema` structured output. Parsing is tolerant: unknown fields are ignored, and only the required fields are validated.
4. **One target window, guarded.** Aura always knows which window it is operating, and refuses to inject input into anything else.
5. **Aura is invisible to itself.** Its UI and HUD are excluded from capture. Aura never minimizes, hides or steals focus to take a screenshot.
6. **Every step leaves a trace.** Requests, responses, images, timings and effects are written to disk, so failures can be diagnosed afterwards, including by a coding agent reading the files.
7. **Ask instead of failing.** Unclear high-impact actions, repeated no-effect actions and the secure desktop all pause and ask the user. They don't abort.
8. **Small-model friendly by default.** Short prompts, one image, at most about 40 marked elements, textual effect summaries, and an explicit subgoal checklist.
9. **No Python in production. No unrequested seed rows in SQLite** (see the tracker decision). Built-in knowledge, if any, lives in read-only Rust constants (decision D3).

---

## 5. Target Architecture

### 5.1 Module layout (refactor `agent.rs` incrementally, extracting a piece only when a milestone touches it)

```
src-tauri/src/
├── agent/
│   ├── mod.rs          # Tauri commands only (thin)
│   ├── orchestrator.rs # Step state machine (§5.6)
│   ├── provider.rs     # Provider trait, OpenAI Responses + compatible chat, json_schema, cancellation, retries
│   ├── mock.rs         # Scripted MockProvider for tests (cfg(test) + aura-cli --mock)
│   ├── schemas.rs      # JSON Schemas for each model call
│   ├── prompts.rs      # Prompt builders (compact / full variants)
│   ├── observe.rs      # Builds an Observation (capture + perception + marks + encode)
│   ├── resolve.rs      # Model action → physical action (element id / coords / UIA pattern)
│   ├── verify.rs       # Pre-click zoom check, post-action effect analysis, done check
│   ├── session.rs      # TaskSession, subgoals, notes, history (replaces reliability.rs parts)
│   ├── safety.rs       # Impact classification + confirmation gate (from reliability.rs)
│   └── trace.rs        # Per-run debug folder writer
├── desktop/
│   ├── affinity.rs     # WDA_EXCLUDEFROMCAPTURE for Aura windows
│   ├── target.rs       # Target window tracking, foreground guard, occlusion check
│   ├── takeover.rs     # Detect real (non-injected) user input → pause
│   └── apps.rs         # open_app / focus_window / open_url (typed, no exec)
├── capture/
│   ├── service.rs      # Persistent multi-adapter DXGI service keyed by HMONITOR
│   ├── dxgi.rs         # (existing, hardened)
│   └── encode.rs       # Fast resize + JPEG/WebP encode, marks rendering hook
├── perception/
│   ├── uia.rs          # Cached, budgeted, filtered UIA snapshot from target HWND
│   ├── ocr.rs          # Window-scoped OCR (lazy)
│   └── marks.rs        # Element merge/dedupe/rank + Set-of-Mark drawing
├── settle/             # v2 change-then-stable detector (§5.5)
├── skills/             # Skill v2 compile, retrieve, guided replay, app knowledge, lessons
├── eval/               # Task suite runner + checkers (used by aura-cli)
└── bin/aura-cli.rs     # Headless runner: run/eval/calibrate, same orchestrator as the app
```

### 5.2 Observation contract (`agent/observe.rs`)

```rust
struct Observation {
    frame_id: u64,                 // monotonically increasing; post-action frames must be > pre-action
    present_time: i64,             // DXGI LastPresentTime of the frame used
    monitor: MonitorInfo,          // index, hmonitor, origin, size, dpi
    target: TargetWindow,          // hwnd, pid, process_name, title, class, visible_rect (DWM bounds)
    image: EncodedImage,           // bytes, mime, width, height, region (monitor | window crop), to_physical transform
    marked_image: Option<EncodedImage>, // same geometry with numbered boxes drawn
    elements: Vec<Element>,        // ranked, ≤ N, ids 1..=N stable within this observation
    focused: Option<ElementRef>,
    cursor: CursorKind,            // arrow / wait / appstarting / ibeam ...
    timings: ObserveTimings,
}

struct Element {
    id: u16, source: Uia | Ocr | Anchor,
    role: String,                  // "button", "edit", "link", "menuitem", "tab", "text" ...
    name: String, value: Option<String>, automation_id: Option<String>, class_name: Option<String>,
    rect_physical: Rect, rect_image: Rect,
    enabled: bool, focused: bool, is_password: bool,
    patterns: PatternSet,          // invoke, value(writable), toggle, select, expand, scroll
}
```

**Region policy:** by default, send the **target window crop**. Use the full monitor when the target is the desktop or shell, or when a popup or menu extends outside the window. Coordinates are always relative to the image actually sent. The `to_physical` transform is stored with it.

### 5.3 Model call schemas (`agent/schemas.rs`)

All calls go through `Provider::complete(ModelRequest { system, user_text, images, schema, max_tokens, temperature }) -> ModelResponse`.
- **Compatible chat (LM Studio):** `response_format: {"type":"json_schema","json_schema":{"name":..,"strict":true,"schema":..}}`.
- **OpenAI Responses:** `text.format: {"type":"json_schema", ...}`.
- If the server rejects the schema, fall back to plain text + tolerant JSON extraction (first `{` to its matching `}`), and record `schema_supported=false` on the provider.

**Step schema** (flat on purpose: no `oneOf`, so weak grammar engines cope):

```json
{
  "previous_outcome": "not_applicable|succeeded|failed|uncertain",
  "observation": "string (≤ 2 sentences about the current screen)",
  "subgoal_index": "integer",
  "thought": "string (≤ 2 sentences)",
  "action": {
    "type": "click|double_click|right_click|hover|type_text|hotkey|key_press|scroll|drag|open_app|focus_window|open_url|select_text|wait|note|ask_user|done|fail",
    "element_id": "integer|null",
    "x": "integer|null", "y": "integer|null",
    "to_element_id": "integer|null", "to_x": "integer|null", "to_y": "integer|null",
    "text": "string|null",
    "keys": "array<string>|null",
    "direction": "up|down|left|right|null", "amount": "integer|null",
    "overwrite": "boolean|null", "press_enter": "boolean|null",
    "name": "string|null", "url": "string|null",
    "start_phrase": "string|null", "end_phrase": "string|null",
    "seconds": "number|null"
  },
  "target_description": "string",
  "expected_outcome": "string",
  "impact": "routine|destructive|financial|credential|communication",
  "confidence": "number 0..1"
}
```

The other schemas are: `plan` (subgoals list), `point_check` (`{on_target: bool, corrected_element_id: int|null, reason}`), `done_check` (`{complete: bool, missing: string, evidence: string}`), `lessons` (≤ 3 `{process, trigger, lesson}`), `calibration` (`{x, y}`).

Rust validates each response after parsing. It checks that the fields required for the chosen `type` are present, that `element_id` exists, and that coordinates fall inside the image. Any failure goes back to the model as **specific feedback** on the retry (the Agent S `call_llm_formatted` pattern).

### 5.4 Action resolution (`agent/resolve.rs`)

Order of precedence for pointer actions:
1. `element_id` → the element's clickable point. Prefer the UIA `GetClickablePoint`, else the rect center, inset if the rect is large.
2. `x,y` in image space → physical space via `to_physical`. Then **snap only if** the point lies inside exactly one interactive element. Never snap by text similarity.
3. Validate: inside the monitor, inside the target window or one of its owned popups (§5.7), and not over an Aura window.

Text entry: `type_text` with `element_id` → focus that element (click), type, then read back through ValuePattern. If the read-back doesn't match and the ValuePattern is writable, fall back to `SetValue`.

Named actions are resolved in `desktop/apps.rs` (§6 M5). There is never any shell command execution.

### 5.5 Settle v2 (`settle/`)

Inputs: the pre-action frame `F0` (captured right before injecting input) and an action class. The detector is a state machine:

```
WAIT_FOR_CHANGE ──(tile change vs F0)──► WAIT_FOR_STABLE ──(no tile change for stable_ms)──► SETTLED
      │ change_timeout elapsed, no change                    │ max_ms elapsed
      ▼                                                      ▼
  NO_EFFECT (reported, not an error)                      UNSTABLE (reported, proceed)
```

- **Tile diff:** a 32×18 grid. Each tile is sampled at a 4-px stride at full resolution, using mean absolute luma difference. A tile counts as "changed" above 3%. Changes smaller than 0.05% of the screen area are ignored as caret/noise.
- **Busy cursor** (`IDC_WAIT` / `IDC_APPSTARTING`) extends WAIT_FOR_STABLE up to `max_ms`.
- **Additional change signals:** the foreground HWND, the window title, a new top-level window from the target PID, and a UIA focus change.

Per-action-class defaults (ms):

| Class | change_timeout | stable_ms | max_ms |
|---|---|---|---|
| type_text / key_press | 300 | 150 | 1500 |
| click / hover / select_text | 700 | 250 | 4000 |
| scroll | 400 | 200 | 2000 |
| hotkey | 1200 | 300 | 5000 |
| open_app / open_url / focus_window | 3000 | 500 | 12000 |
| wait | — | — | `seconds` |

Output (`ActionEffect`, also rendered as text for the next prompt):

```rust
struct ActionEffect {
    settle: SettleOutcome,                // Settled | NoEffect | Unstable, elapsed_ms
    changed_area_pct: f32, changed_near_target: bool,
    title_before: String, title_after: String,
    foreground_changed: bool, new_windows: Vec<String>,
    focused_before: Option<String>, focused_after: Option<String>,
    value_readback_ok: Option<bool>,
}
// e.g. "After your last action: a new window 'Save As' appeared; focus moved to Edit 'File name'."
```

### 5.6 Orchestrator state machine (`agent/orchestrator.rs`)

```
Init ─► AcquireTarget ─► Plan(subgoals) ─► [Skill lookup → GuidedReplay?]
  └─► loop:
        Observe ─► Propose (≤3 attempts, feedback on each) ─► Validate (schema/ids/bounds/loop)
        ─► SafetyGate (confirm with user if impact not authorized) ─► PointCheck (conditional)
        ─► Guard (foreground restore + occlusion + takeover check) ─► Execute ─► Settle/Effect
        ─► Record (trace + DB) ─► Progress policy
  done  ─► DoneCheck (model + deterministic) ─► Finish / continue with "missing" feedback
```

Progress policy:
- If 2 consecutive `NoEffect` or `failed` outcomes, the next prompt gets a forced strategy change (for example "keyboard instead of mouse", "scroll", "different element").
- After 4, or after a repeated identical action, the agent asks the user (humility gate, via `ask_user`).

Budgets: `max_steps` defaults to 40 (cap 100), plus a wall-clock limit (default 15 min) and a model-call limit.

**Cancellation:** one `CancellationToken` is shared by the kill switch, the UI stop button and takeover. Every `await`, including HTTP, runs under `tokio::select!` against it.

**Conditional PointCheck** (a zoom crop with the marker drawn on it): runs only when (a) the action used raw coordinates, (b) `confidence < 0.6`, (c) the impact is non-routine, or (d) the previous step failed.

### 5.7 Window, focus and capture hygiene (`desktop/`)

- **Exclude from capture:** at startup, call `SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE)` on the Tauri main window and the overlay. Verify with `GetWindowDisplayAffinity`. On older Windows without support, fall back to hiding only during the capture call itself. Never minimize.
- **Mini panel:** while a run is in progress, the main window shrinks to a compact always-on-top panel in a monitor corner. It is not focusable (`set_focusable(false)`). If a planned click lands under the panel, the panel moves to the opposite corner before input.
- **Target window:**
  - A `SetWinEventHook(EVENT_SYSTEM_FOREGROUND)` tracks the last non-Aura foreground window. At run start, the target is that window, restored to the front. If there is none, the target is the desktop.
  - The target switches when a foreground change is *caused by our action*, meaning it happened within the settle window of an executed action.
  - Other changes (a toast, a UAC prompt, the user) are flagged.
- **Foreground guard** before each input:
  - If the target is not in front: `ShowWindow(SW_RESTORE)` if it is minimized, then `SetForegroundWindow`, using the documented Alt-tap or `AttachThreadInput` workaround for the foreground lock. Then confirm.
  - If it still fails, re-observe instead of acting.
- **Occlusion check** for pointer actions:
  - `WindowFromPoint` → `GetAncestor(GA_ROOT)` must be the target, a window it owns, a popup or menu of the same PID, or a shell surface the action expects (taskbar or Start for `open_app`).
  - Otherwise abort, re-observe, and tell the model what is covering the target.
- **User takeover:** a low-level mouse/keyboard hook sees non-injected input (no `LLKHF_INJECTED` / `LLMHF_INJECTED`) during a run and auto-pauses. The HUD turns amber and the UI offers Resume. Esc still means a hard stop.
- **Window bounds:** use `DwmGetWindowAttribute(DWMWA_EXTENDED_FRAME_BOUNDS)` for the visible window rect.
- **Secure desktop** (UAC, lock screen): if DXGI reports access denied or lost, pause and ask the user to resolve it. Never try to drive the secure desktop.
- **Browser tabs:**
  - `open_url` reuses the current tab of the target browser (Ctrl+L, type, Enter) when the target is a browser. Otherwise it uses `ShellExecuteW` with an `http(s)` URL only.
  - The prompt says to prefer the existing tab and not to open new tabs or windows unless the task requires it.

### 5.8 Persistent capture service (`capture/service.rs`)

- `CreateDXGIFactory1` → enumerate **all** adapters and their outputs → map `DXGI_OUTPUT_DESC.Monitor` (HMONITOR) to a lazily created duplication. There is one D3D device per adapter.
- Keep the duplication alive across steps. Frames are stored as `Arc<Frame>` in BGRA and converted on demand. No 33 MB clones.
- **Freshness:** a post-action observation must use a frame whose `LastPresentTime` is after the input timestamp. Otherwise wait for the next frame (up to the settle `max_ms`).
- For a fresh duplication, loop until `LastPresentTime != 0` (≤ 200 ms) to avoid black first frames.
- ACCESS_LOST is recreated with bounded retries (3), then reported as an error.
- Fallback when DXGI is unavailable: Windows.Graphics.Capture per window (decision D4) or GDI `BitBlt`.
- **Encoding:** SIMD resize (`fast_image_resize`) to `max_image_edge` (default 1280, configurable per provider, e.g. 1024 for Gemma), then JPEG q85. Real WebP is optional later. Store trace images at the same encoding.

---

## 5.9 Active Scope (set by the user 2026-10-29): CORE LOOP FIRST

The user wants only the core loop strong and testable: **screenshot -> brain -> input -> harness -> work -> screenshot -> repeat**, reliable in every scenario. Work order until the user says otherwise:

1. **Stage 1 (clean loop):** rest of M0 (per-step trace folder), M2 (window/focus/capture hygiene), M3 (settle v2 + action effects).
2. **Stage 2 (accurate clicking):** M4 (numbered elements, calibration, conditional point-check).
3. **Deferred until the core is proven:** M5 (extra actions, planner, gates), M6 (learning), M7 (eval suite). Only pull individual M5 items forward if testing shows the core needs them (for example `wait`, retries with fresh screenshots).

**Model-agnostic rule (user, 2026-10-29):** the harness is built for *all* models. Never special-case a model name (including `gemma`). Per-provider differences (coordinate convention, max image edge, prompt size, schema support) are **measured or configured settings** (calibration run, UI field), never hard-coded. `google/gemma-4-e2b` is only the local test model; passing there is a floor, not the target. Findings that look model-specific (for example Gemma's wrong raw pixels) are solved by making the harness robust (element IDs, calibration, verification), not by tuning for that model.

After each stage, give the user a concrete manual test script and read the trace folders they send back.

**Update (user, 2026-10-30):** priorities are (1) the harness can execute whatever a model orders, in every situation, flawlessly (clicking, scrolling, typing and their variations); (2) the brain-to-body connection matters most; (3) model mistakes are not the problem under test. Testing therefore means acting as the model (see §12), not running a weak model and guessing.

## 5.10 Behaviour as built (2026-10-30)

This is what the code does today. If it disagrees with the target design above, this section is the truth.

**Observation (every step):** the screenshot is the whole monitor that holds the active window, scaled to at most 1280 px on its long edge and sent as JPEG (q88). UI Automation (from the window handle, depth 14) and OCR run in parallel; OCR text is kept only where the window at that point belongs to the active program. Up to 50 numbered elements are drawn on the screenshot (`marks.rs`): words from the task pull matching elements forward, scroll-bar parts and similar window furniture come last, and list/tree/table rows have their own allowance of 15. The prompt also carries the active window (title and process only), `WINDOWS OPEN: N` (numbered front-to-back list of up to 16 windows with ACTIVE / minimized / on-another-monitor flags), `SCROLL POSITION` (UI Automation scroll data under the mouse or focus, or "not reported by this app"), the last steps with their measured effects, notes and the recipe hint.

**Actions (one per step, `model_schema.rs`):** click, double_click, mouse_move, drag_and_drop, type_text, hotkey, key_press, open_app, focus_window, open_url, note, ask_user, scroll, wait_settle, done, fail. Pointer actions name a numbered `element` (preferred) or raw x/y. `focus_window` takes a title, program name or a number from `WINDOWS OPEN`; minimized windows are restored. Shortcuts written as one string (`Ctrl+Shift+T`) are split into keys. A plain "open <app>" task skips the model.

**Coordinates (`coords.rs`):** element-number points are never altered. A raw x/y that just misses an element (14 px) snaps onto it. Each model's consistent offset or scale error is learned from its own near misses (at least 6 consistent samples, must cut the error by 40%) and removed; samples are stored per model name in `%LOCALAPPDATA%\AgentAura\coord_calibration.json`. So far verified on synthetic data only.

**Guards before input (`agent.rs::prepare_input`, in order):** elevated target window gives a clear error; the window the model looked at must still be in front (restored if briefly stolen) and unmoved; the point must not be covered by another program (taskbar and desktop stay valid); the numbered target must still be where it was (the page's elements are re-read; moved or vanished gives a refusal naming where it is now; OCR targets are compared vertically only); none of Aura's own windows may cover the point (the panel moves away). A refusal is fed back to the model as a failed step with the reason; three in a row stop the run.

**Actuation (`input/send_input.rs`):** real key presses for ASCII (Calculator ignores Unicode packets), Unicode packets for everything else, Enter/Tab for line breaks, extended-key flags and scan codes, chords held about 70 ms, pointer move confirmed before a click, drag nudge and rest on the target before release, scroll sent in 3-notch chunks (a scroll with no point is aimed at the target window), clipboard paste for long text with the previous clipboard restored, `open_app` waits until the new window is in front and responsive.

**Read-back after every action:** the measured screen effect (`settle/effect.rs`: changed / settled / no visible effect / unstable), windows opened or closed with the count (`windows open: 4 -> 5`; minimizing is not closing), the window that is now in front, what has keyboard focus and what it contains (UI Automation value or text), a WARNING when text was typed but focus is not on a text field, and after a scroll the position (UI Automation, or an estimate from what was sent with "the page did not move, so you are at the bottom" when Chrome reports nothing).

**Feel:** while a run is active the screen edge glows in the state colour and a banner at the top centre of the primary screen says what the harness is doing: "Taking screenshot" (blue), "Thinking" (purple), "Working" (cyan), "Waiting for the screen" (amber), "Stopped" (red), and "Task finished" (green, hides itself after about 2.4 s). The banner is part of the HUD overlay, which is excluded from screen capture, so the model never sees it. The pointer glides to every target along a straight line with an ease-in-out curve (`send_input.rs::glide_to`; 160-560 ms depending on distance; `AURA_FAST_MOUSE=1` turns it off), then the exact final position is confirmed as before. A run plays a rising start sound, a chime when it finishes and a soft falling sound when it ends any other way (`sound.rs`, synthesised in memory, played asynchronously; `AURA_MUTE=1` silences them). The overlay window runs on its own thread with a message loop, so showing or hiding it never blocks the run loop.

**Safety:** Esc kills everything (injected Esc is ignored); consequential actions the task did not clearly ask for need the user's Allow; the loop pauses while the user uses the mouse or keyboard; API keys live in memory only.

---

## 6. Milestones

> Order matters. M0 and M1 make testing possible; M2 and M3 fix the screenshot and focus behaviour the user reported; M4 and M5 make the agent smarter; M6 makes it improve over time; M7 proves it.

### M0 — Tooling & Baseline (prerequisite)
- [x] **Restore a working build** (see §2.4 and D6). Either install VS 2022 Build Tools (C++ workload + Windows SDK) and pin MSVC with `rust-toolchain.toml`, or recover the GNU workaround and commit it to `src-tauri/.cargo/config.toml`. Document the chosen setup in `README.md`. **Done 2026-10-29: MSVC installed, `rust-toolchain.toml` pins it; the old GNU config is kept as `src-tauri/.cargo/config.toml.gnu-disabled`. `README.md` note still to add.**
- [x] Record the baseline: the `cargo test --lib` result in §11. Baseline on MSVC: 39 passed, 0 failed, 1 ignored (live test).
- [x] Scripted mock model: done as `mock_server.rs` (a local HTTP server that answers like a model) with six end-to-end loop tests; there is no `Provider` trait.
- [x] `trace.rs`: `%LOCALAPPDATA%/AgentAura/runs/<run_id>/` containing `run.jsonl` plus `step_NNN/{obs.jpg, marked.jpg, request.json, response.json, action.json, effect.json, timings.json}`. **Done: `src/trace.rs`, wired into the loop (obs/post PNGs, raw model text, proposal/verification/action JSON, effect, timings; keeps last 30 runs).**
  - API keys are never written. Images are referenced by filename.
  - Keep the last 30 runs (configurable).
- [x] `src/bin/aura-cli.rs` headless runner sharing the loop (`run`, `eval`, `conformance`, `look`, `do`; `calibrate` not built). It has no Tauri window; it uses the kill switch and the HUD.
  - `aura-cli run --base-url http://192.168.6.70:1234/v1 --model google/gemma-4-e2b --task "..." [--max-steps N] [--mock script.json]`
  - `aura-cli calibrate ...` (M4), `aura-cli eval --suite basic` (M7)
- [x] DPI awareness is set at startup and logged (`SetProcessDpiAwarenessContext did not change anything (already set?)` means the manifest already set it).
- **Acceptance:** a mock-provider run executes a scripted 3-step task against a fixture without a real model, and writes a complete trace folder.

### M1 — P0 Blockers (B01, B02, B03, B04 stopgap, B05, B12, B15)
- [x] Structured output via `json_schema` for both protocols, with automatic fallback (§5.3).
- [x] Tolerant parsing: drop `deny_unknown_fields` on model-facing structs. Add the validation-feedback retry.
- [x] Kill switch ignores injected keys (`LLKHF_INJECTED` / `LLKHF_LOWER_IL_INJECTED`).
- [x] `InputActuator::release_all()`: track held keys and buttons in a static set, release them bypassing the kill check, and call it on kill, on error and at the end of every action.
- [x] Remove the candidate override that uses the proposal target text. Snap only by point-in-element (§5.4).
- [x] Stopgap until M4: send candidates in **image** coordinates, filtered to on-screen and enabled elements.
- [x] `CancellationToken` wired to the kill switch and the stop command. HTTP calls abort within 100 ms. Implemented by polling the kill switch in a `tokio::select!` (no new crate), so `Esc` and the UI stop button both cancel in-flight requests.
- **Tests:** unit tests for injected-key filtering (simulated flags), release-all after a simulated kill mid-hotkey, schema builder output, tolerant parsing with extra fields, and coordinate conversion of candidates.
- **Acceptance:** a real run against LM Studio completes at least the observe → propose → execute → observe cycle 3 times with no HTTP 400. The agent pressing Esc does not stop the run. **Status: proposal request verified live (HTTP 200, parseable, 4.4 s); the full in-app loop run is still to do and needs the GUI or `aura-cli`. Gemma-4-e2b raw-pixel click accuracy was poor (asked for (498,453), target center (680,330)) which is what M4 addresses.**

### M2 — Window, Focus & Capture Hygiene (B06, B07, B08, B09, B22)
- [x] `desktop/affinity.rs`: exclude the main window and overlay from capture. Remove every minimize/hide/show/set_focus call around capture and execution. **Done: main window and HUD excluded via `SetWindowDisplayAffinity`; no minimize/hide/show/focus calls remain in the run path.**
- [x] Mini-panel mode during runs, non-focusable and always on top, with the move-out-of-the-way rule. **Done: `desktop/panel.rs` (440x620 DIP corner panel, non-focusable, moves to a free corner when it would cover a click).**
- [x] `desktop/target.rs`: foreground event hook, target acquisition at start, target-follow rule, foreground guard, occlusion check. **Done (z-order based target; no foreground event hook yet): target tracking, foreground guard, occlusion check vs Aura windows.**
- [x] Auto-pause on real user input (real, non-injected input only; auto-resume after 2.5 s of quiet, then re-observe). Built inside the loop, not as `takeover.rs`.
- [ ] (Not built; reuse judged not worth the risk, see B08) `capture/service.rs`: persistent multi-adapter service with HMONITOR mapping, freshness by `LastPresentTime`, bounded ACCESS_LOST handling, and secure-desktop detection.
- [x] `capture/encode.rs`: SIMD resize + JPEG. Rename the `webp` fields and the DB column semantics to `image` (migration v3). **Partly done 2026-10-30: images sent to the model are JPEG; trace and UI images are still PNG and the field names still say `webp`.**
- [x] Use DWM extended frame bounds for window rects. **Done in perception.**
- [ ] Progress events send a small thumbnail (≤ 480 px), not the full image.
- **Tests:**
  - Unit: target-follow rule, occlusion decision table, freshness check with fake present times.
  - Manual script in `docs/` checklist: open Notepad behind Aura, run "type hello", confirm Notepad receives the text and Aura never minimizes. Pop a window over Notepad mid-run and confirm the occlusion abort. Move the mouse mid-run and confirm the pause.
- **Acceptance:** trace images never contain the Aura UI or HUD. Zero focus changes not caused by actions. Median capture + encode ≤ 60 ms at 1080p.

### M3 — Settle v2 & Action Effects (B10)
- [x] Pre-action frame capture inside `Execute`. **Done: baseline = the frame the model saw.**
- [x] The change-then-stable detector from §5.5, with per-action-class timings. **Done: `settle/effect.rs` (state machine + driver, 10 unit tests, text input has a more sensitive threshold).**
- [x] `ActionEffect` computation, including window and title diffs, the new-window list and the UIA focus diff. **Done 2026-10-30: outcome, changed %, window title change, windows opened/closed with counts, UIA keyboard-focus and its content (read-back, see §5.10).**
- [x] Textual effect summary injected into the next prompt and stored in the trace. **Done: stored on `StepRecord.effect`, shown in the next prompt.**
- [x] `NoEffect` feeds the progress policy. **Partly: the model is told, a STUCK warning appears after 2 and the run stops after 4 no-effect steps; no automatic strategy switch.**
- **Tests:** synthetic frame sequences covering no change, delayed change at 400 ms, an animating spinner, and caret blink noise. Assert the states and timings.
- **Acceptance:** on the fixture "click File menu", the post-action image shows the open menu in 10/10 trials. A no-op click reports `NoEffect` within `change_timeout`.

### M4 — Perception & Grounding (B04 final, B16, B17)
- [x] `perception/uia.rs`: **Done enough: UIA and OCR run in parallel (observe about 0.4 s); the search starts at the window handle (falls back to the process); OCR is clipped to the active program. No time budget needed at that speed.**
  - Start from `ElementFromHandle(target)` plus same-PID owned and popup top-level windows (menus).
  - CacheRequest for the needed properties and patterns, ControlView, a 300 ms budget with partial results.
  - Filter out off-screen, disabled, zero-size and out-of-window elements.
- [ ] `perception/ocr.rs`: OCR only the window crop, lazily (when UIA yields fewer than 8 named interactive elements, or for `select_text`). Merge OCR lines that don't overlap UIA rects. Check whether word-level boxes are available.
- [x] `perception/marks.rs`: **Done as `src/marks.rs`: relevance to the task + per-region spread + dedupe, cap 50, numbered boxes drawn on the screenshot, 9 tests.**
  - Merge, dedupe (IoU > 0.8 or nested same-name) and rank (interactive > named text; focused window first).
  - Cap N (40 by default, 60 for capable models).
  - Draw numbered high-contrast boxes, avoid label collisions, and produce the compact element list text (`[12] button "Save"`).
- [x] Provider coordinate convention `{ImagePixels | Normalized1000 | Normalized1}`, configured per provider. **Replaced 2026-10-30 by `coords.rs`: near misses snap onto elements and a per-model offset/scale error is learned and removed (see §5.10).**
- [x] `aura-cli calibrate`: synthetic images with markers at known positions. Choose the convention with the lowest error and report the mean pixel error. Persist non-secret provider settings (convention, max edge, schema support) in a settings file. Never persist the key. **NOT done as a CLI command; `coords.rs` learns the error from live use instead.**
- [x] PointCheck (§5.6 conditions): a 300×300 crop at 2× zoom with a red crosshair → `point_check` schema → accept, correct to an element ID, or replan. **Done in a simpler form: the verifier (only for risky/unlisted/done actions) sees a red crosshair where the click will land.**
- **Tests:** UIA filter and rank on recorded snapshot JSON fixtures, mark layout without overlaps, all three calibration conventions with the mock.
- **Acceptance:** on fixture screenshots (Notepad, Explorer, Settings, Chrome), the mock-selected element IDs map to the correct physical points (±2 px). With Gemma, a click on a named button succeeds ≥ 8/10 via element ID.

### M5 — Action Space, Orchestrator & Intelligence (B11, B13, B14, B20, B21)
**Status 2026-10-30:** done: `mouse_move` (hover), drag, `open_app`, `focus_window`, `open_url`, `ask_user`, `note`, `wait`, session notes, clipboard restore, retry with fresh observation and specific feedback, done-check, the confirmation card, the stuck-streak guard, step limit and run limits. Not done: `select_text`, subgoal planner, `plan` call, ValuePattern `SetValue` fallback, clipboard-history exclusion and paste verification, compact/full prompt variants, a forced strategy switch, `right_click` as its own action (it is `click` with `button`).
- [ ] New actions:
  - `right_click`, `hover`, `drag` (element or coordinates), `select_text` (OCR phrase → drag or shift-select), `wait`, `note`.
  - `open_app(name)`: if already running, `focus_window`. Otherwise Start search (Win, type, settle, Enter), then verify a new window from a matching process.
  - `focus_window(name)` via EnumWindows fuzzy match on title or process.
  - `open_url(url)` per §5.7.
  - `ask_user(question)` → UI prompt → the answer goes into the session notes.
- [ ] Text entry: ValuePattern read-back, SetValue fallback, and per-character Unicode typing for consoles (conhost, WindowsTerminal) and password fields.
- [ ] Clipboard: save and restore text, mark the content excluded from clipboard history and monitoring (`ExcludeClipboardContentFromMonitorProcessing`, `CanIncludeInClipboardHistory=0`), and verify the paste.
- [ ] Subgoal planner: one `plan` call at start (3–8 subgoals). Each step reports `subgoal_index`. The plan is re-run when the model or the progress policy says it is invalid.
- [ ] Session notes buffer (Agent S `save_to_knowledge` equivalent), shown in every prompt.
- [ ] Retry with a fresh observation plus specific feedback (validation error, loop detection, point-check rejection).
- [ ] Progress policy (§5.6): forced strategy change, then `ask_user` (humility gate).
- [ ] DoneCheck: a `done_check` call on the current screen with the subgoals and task. Deterministic hints feed it, such as a title containing the expected filename. If incomplete, continue with the `missing` text.
- [ ] Safety gate: word-boundary and phrase matching. Unauthorized impact → pause + confirmation modal (action text + zoom image), not failure. Typing into `is_password` fields always requires confirmation.
- [ ] Prompts (`prompts.rs`): a *compact* variant for small models (≤ 700 tokens of system prompt, one few-shot example) and a *full* variant.
  - Both contain the rules: prefer hotkeys, one action per step, stay in the current tab or window, verify before done, never invent elements.
  - Include the last 3 steps with their effects, plus the subgoals, notes and app tips (M6).
- [ ] Budgets: `max_steps` default 40 (cap 100), wall-clock limit, model-call limit, all shown in the UI.
- **Tests:** orchestrator transitions with MockProvider (retry feedback, forced strategy change, ask_user path, done-check rejection then completion, confirmation deny and approve).
- **Acceptance:** with Gemma, "open Notepad, type a sentence, save it as `%TEMP%/AuraEval/a.txt`" succeeds ≥ 4/5 trials. With a capable model, it succeeds 5/5.

### M6 — Learning That Improves Runs (B18, B19)
**Status 2026-10-30:** first version only: successful runs are saved as plain-language recipes (no coordinates, failed steps dropped) and shown as a hint to similar tasks; reviewed skills outrank them. Everything below is still open.
- [ ] Schema migration v4: `SkillsV2`, `SkillSteps`, `AppKnowledge`, `Lessons`, an embedding column. Keep the v1 tables read-only for compatibility.
- [ ] Skill compiler. It takes only steps whose outcome is `succeeded`, confirmed by `ActionEffect` or the model, and drops undo pairs.
  - Each step stores the action template plus a target signature: automation_id, name, role, class, window class, process, window-relative normalized position, and a 64×64 JPEG anchor.
  - Literals that appear verbatim in the task (URLs, filenames, quoted text) become parameters.
- [ ] Promotion: an auto-verified completion becomes a **candidate** skill (low trust). A review verdict of "Perfect" or "Optimize" makes it **trusted**. "Mistake" stores a guardrail with process scope and the step signature.
- [ ] Retrieval: embeddings via the provider `/v1/embeddings` (configurable model, default `text-embedding-nomic-embed-text-v1.5` on LM Studio). Fall back to normalized Levenshtein. Threshold 0.85 plus app availability.
- [ ] Guided replay (the practical form of mid-task snapping):
  - For each step, re-resolve the target: UIA signature match → anchor NCC in a window-relative ROI → otherwise give the step to the model as "expected next step" with the element list.
  - Check each step's expected effect. On a deviation, drop to the normal loop with the remaining steps as hints.
  - Trusted skills run without per-step model calls when every target resolves deterministically.
- [ ] App knowledge: the top 3 tips for the target process are injected into prompts.
  - Tips come from reviews ("Optimize" text), post-run lessons, and (D3) optional built-in constants.
  - Confidence goes up on successful runs that used the tip and down on failed ones. Tips below a threshold are pruned.
- [ ] Post-run lessons (Agent S4 idea, simplified): after failed or corrected runs, one `lessons` call over the compact trajectory text produces ≤ 3 candidate tips. Optionally route this call to a stronger "analysis provider".
- [ ] Radar performance: a grayscale ROI only, coarse-to-fine (¼ scale search, then refine), integral images for patch statistics. Target ≤ 10 ms per anchor. Anchors are matched window-relative with DPI scaling.
- [ ] Fix `submit_task_review` to use the compiler. Rename the legacy "golden path" hint.
- **Tests:** compiler on recorded trajectories (failed steps removed, parameters extracted), replay resolution order with fixtures, tip confidence updates, radar accuracy and time on fixtures.
- **Acceptance:** on 3 eval tasks, the 2nd and 3rd trusted runs meet the §1 targets (≥ 40% fewer model calls, ≥ 30% less time) with equal success.

### M7 — Qualification, Docs & Release (B23)
**Status 2026-10-30:** done: eval runner (`aura-cli eval`, suites basic / web / intermediate / all, independent checkers, `--repeat N` pass rates, JSON report under `%LOCALAPPDATA%\AgentAura\eval`), the model-free conformance suite, docs refresh, release build. Not done: model pass rates (no Gemma or other numbers yet), robustness fault-injection suite, Settings/Paint/Explorer tasks.
- [ ] `eval/` suite in `eval/tasks/*.json`. Each task has `{id, prompt, setup[], checker, max_steps, timeout}`.
  - Setup uses typed steps (create a temp folder, launch an app). There is no arbitrary shell.
  - Checkers: `file_contains`, `file_exists`, `window_title_contains`, `clipboard_equals`, `registry_value`, `process_running`.
  - Everything is sandboxed under `%TEMP%/AuraEval`.
- [ ] Suites:
  - **basic** (~12 tasks): Notepad write/save, Calculator result to clipboard, Explorer create and rename a folder, Settings toggle (e.g. dark mode, then restored), browser search plus a copied result title, Paint draw a line, and so on.
  - **intermediate** (~8 tasks): multi-app copy/paste, a Save As dialog, menus and context menus, scroll-to-find, form fill.
  - **robustness** (fault injection): a popup over the target mid-run, a minimized target, 150% DPI, a second monitor, a slow-loading page, user mouse movement (expect a pause).
- [ ] Metrics report: success rate, steps, model calls, wall time, misclick rate (point-checks rejected and wrong-window aborts), NoEffect rate. Saved to `eval/results/<date>-<model>.json`.
- [ ] Docs: update `CORE.md` (action grammar v2, observation, settle v2, window hygiene), `FEATURES.md` (mark what is implemented) and `PROGRESS_TRACKER.md` (truthful completion).
- [ ] Release build via the Tauri pipeline. Smoke-test `dist/agent-aura.exe` with LM Studio.
- **Acceptance:** the §1 targets are met or the gaps are documented with eval evidence.

---

## 7. Agent S Techniques Adopted (traceability)

| Agent S origin | Aura adaptation | Milestone |
|---|---|---|
| `call_llm_formatted` format-feedback retry | Validation-feedback retry with fresh observation | M1/M5 |
| Reflection agent (cycle detection) | Progress policy + effect summaries + forced strategy change | M3/M5 |
| `grounding_width/height` | Provider coordinate convention + calibration | M4 |
| Behavior Narrator (marked action + zoom) | PointCheck zoom crop with marker | M4 |
| `open`, `switch_applications` | `open_app`, `focus_window` via native APIs | M5 |
| `highlight_text_span` (OCR word ids) | `select_text` via Windows OCR | M5 |
| `save_to_knowledge` notes | Session notes buffer | M5 |
| Procedural prompt rules (hotkeys, one action, verify before done) | `prompts.rs` compact/full | M5 |
| Image-history flush (last k images) | Last 3 steps as text + effects; optional previous thumbnail for capable models | M5 |
| S4 evolved interfaces | Parameterized skills + app knowledge + post-run lessons | M6 |
| OSWorld evaluation discipline | `eval/` suites with automatic checkers | M7 |

Rejected on purpose: `exec()` of model code, the code agent with arbitrary shell, the two-model planner + grounder as the default (UIA element IDs replace it; an optional grounding provider can be revisited after M7), and BBoN parallel rollouts (they need VMs).

---

## 8. Performance Budget per Step (excluding model latency)

| Stage | Budget |
|---|---|
| Frame acquire (persistent DXGI) | ≤ 10 ms |
| UIA snapshot (cached, budgeted) | ≤ 300 ms (hard cap, partial results allowed) |
| OCR (only when triggered) | ≤ 250 ms on the window crop |
| Mark drawing + resize + JPEG | ≤ 50 ms |
| Foreground guard + occlusion | ≤ 20 ms |
| Trace write (async, off the critical path) | 0 ms blocking |

Model calls per step: **1** normally. +1 for a PointCheck only under §5.6 conditions. One `plan` call per task and one `done_check` per done claim.

---

## 9. Risks

| Risk | Mitigation |
|---|---|
| `gemma-4-e2b` too weak for planning even with marks | The compact prompt and element IDs reduce the load. Also evaluate a stronger local VLM to separate model limits from Aura bugs. Allow a separate "analysis provider". |
| LM Studio grammar rejects complex schemas | Flat schema with no `oneOf`. Text fallback. `schema_supported` flag. |
| `WDA_EXCLUDEFROMCAPTURE` unsupported (pre-2004) | Detect it, then fall back to hiding only during the capture call. |
| Foreground lock prevents restoring the target | Alt-tap / `AttachThreadInput` workaround. If it still fails, re-observe and report. Never inject into the wrong window. |
| UIA trees are huge or slow (Chrome, VS Code) | Start from the target HWND, cache request, ControlView, 300 ms budget, OCR fallback. |
| Chrome exposes no UIA content | Chrome enables accessibility when queried. Otherwise fall back to OCR + coordinates + PointCheck. |
| A large refactor without version control | Decision D1 (git). Refactor incrementally. Keep tests green after each milestone. |

---

## 10. Open Decisions (defaults used until the user says otherwise)

- **D1 — Version control (resolved 2026-10-30: yes).** Git initialised on `main`; baseline commit `8f52e4c`. See `HARNESS_COMPLETION_PLAN.md` §3.2.
- **D2 — Legacy `engine/` Python folder:** it is unwired and contradicts "no Python". *Default:* leave it in place and mark it deprecated in docs. Delete or move it only with user approval.
- **D3 — Built-in app tips** (Notepad, Explorer, Chrome/Edge, Office, VS Code shortcuts) as read-only Rust constants, not DB seed rows. *Default:* include them, with a settings toggle to disable.
- **D4 — Windows.Graphics.Capture** fallback (per-window capture of occluded windows). *Default:* defer until after M3 unless DXGI problems show up in testing.
- **D5 — Auto-promotion of auto-verified runs to candidate skills.** *Default:* on, as low-trust candidates. Only reviewed skills are "trusted".
- **D6 — Rust toolchain (resolved 2026-10-29: MSVC).** MSVC is the toolchain Tauri officially supports and avoids the GNU proc-macro DLL problems. It needs VS 2022 Build Tools installed by the user. *Default:* recommend MSVC and ask the user. Until then, try to recover a GNU configuration that builds.

---

## 11. Work Log

| Date | Milestone | Summary |
|---|---|---|
| 2026-10-29 | Plan | Audit of Aura vs Agent S3/S4; bug ledger B01–B23; this plan created. Verified LM Studio json_object rejection and json_schema support. |
| 2026-10-29 | Baseline | `cargo test --lib` FAILS. GNU: E0463 `tauri_macros` not found. MSVC: `link.exe` fails (no Build Tools). The 29 passing tests recorded earlier in the tracker cannot be reproduced. Blocks M0. |
| 2026-10-29 | M0 (build) | Diagnosed `E0463 tauri_macros`: UCRT WinLibs gcc shadows rustup's msvcrt mingw, so proc-macro DLLs fail to init (error 1114). Added `src-tauri/.cargo/config.toml` (self-contained linker + `~/.cargo/bin/dlltool.exe` shim). **Still broken:** dev profile -> rustc `STATUS_ACCESS_VIOLATION` compiling `tauri`; release profile -> compiles, but the test exe crashes at startup (SIGSEGV in `std::rt::lang_start_internal`; it lacks the `WebView2Loader`/`comctl32`/`bcrypt` imports of the old working exes). `cargo check --release --lib` passes. Recommended fix: install VS 2022 Build Tools and switch to MSVC (D6). |
| 2026-10-29 | M1 (code, UNVERIFIED) | Written and compiling under `cargo check --release`, but **no test has run**: `json_schema` structured output + fallback; new `model_schema.rs` (schemas, lenient parsing, specific validation errors, JSON extraction, unit tests); injected-key filter in kill switch (+tests); tracked key/button release + `release_all()` after every action; candidates sent in screenshot coordinates; text-match click override removed; Esc cancels in-flight model requests; error feedback on retry. Not yet done in M1: user-visible cancellation test against LM Studio, B13 etc. belong to later milestones. |
| 2026-10-29 | M0 (build) DONE | Installed VS 2022 Build Tools (needed an admin terminal; the SDK finished installing after the MSVC part). Pinned MSVC via `rust-toolchain.toml`; GNU workarounds disabled. `cargo test --lib`: 39 passed, 0 failed. D6 resolved: MSVC. |
| 2026-10-29 | M1 DONE (unit + live request) | Fixed extractor bug found by tests (stray `{x}` in `<think>` blocks). Added candidate-space test and an opt-in live test (`AURA_LIVE_URL`/`AURA_LIVE_MODEL`, `cargo test --lib live_server -- --ignored --nocapture`). Live Gemma result: valid, parseable JSON, but wrong click coordinates -> M4 priority. |
| 2026-10-29 | Stage 1 (code) | Trace folder; desktop hygiene (capture exclusion, corner panel, target tracking, foreground + occlusion guard); settle v2; DXGI `LastPresentTime == 0` bug fixed (pointer-only frames were returned as garbage, differing on 100% of the screen); GDI fallback when a fresh duplication has no frame; model-agnostic prompt using measured effects. 59 unit tests pass. Live test on a real `cmd` window: typing -> Settled in 297 ms, idle -> NoEffect in 306 ms. Not yet verified end-to-end in the GUI app. |
| 2026-10-29 | Stage 2 (M4 core) | Numbered elements (`marks.rs`) drawn on the screenshot, `element` ids in the action schema (click/double_click/drag/scroll) resolved to centres, verifier with crosshair, parallel UIA+OCR, forced-opaque alpha (a transparent DXGI frame made the screenshot look blank), prompt rules for element numbers, `open_app` action, side-docked live status panel, no-effect streak guard (4 -> stop and tell the user; 2 -> STUCK warning in the prompt). 75 unit tests pass. Live on the real desktop with Gemma-4-e2b: 1 of 3 tasks picked the exact right element (picks always resolve to a real element). |
| 2026-10-29 | P1 batch | `focus_window`, `open_url` (http/https only; reuses the current tab when a browser is in front), `wait` with seconds; strict completion check on every `done` claim (rejected claims are fed back as a step; gives up after 3); Allow/Deny approval card in the live panel for consequential actions the task did not clearly ask for (replaces the old silent rejection; 120 s timeout = deny); pause when the user uses the keyboard or mouse (real input only, auto-resume after 2.5 s quiet, re-observes afterwards). 84 unit tests pass. |
| 2026-10-29 | P2 batch | `note` and `ask_user` actions (notes shown every step; ask_user uses the approval card with "Done, continue" / "I can't"); element numbers in the schema are bounded to the real range and offered only when elements exist (a live test showed models inventing ids 0 and 1005 otherwise); schema variants ordered app/keyboard actions first; OCR failures are now logged; deterministic fast path for plain "open <app>" tasks (no model call; 6 recogniser tests); live checks on the real desktop: `open_app("notepad")` opens and focuses Notepad in about 2.5 s; observation timing in release is about 400 ms (UIA ~350 ms, OCR ~165 ms in parallel), so no UIA time budget is needed yet. 88 unit tests pass. Not done: calibration for raw x/y, up-front planner, mock provider + aura-cli, eval suite (M7), learning (M6). |
| 2026-10-29 | Headless runner + eval + learning | `Host` interface so the same loop runs in the app or a terminal (`execute_run`); `aura-cli run` and `aura-cli eval --suite basic|intermediate|all` (checks outcomes independently of the model: foreground title, visible text via UIA+OCR, file contents; cleans up only windows it opened; JSON report under `%LOCALAPPDATA%\AgentAura\eval`); scripted fake model server (`mock_server.rs`) and six end-to-end loop tests that run the real loop without sending input (they found 3 real bugs: notes never reached the prompt, loop detector misfired on `done`, rejected-done counted as failure); settle now judges the active window only (a real run waited 12 s because the IDE streamed text elsewhere, now 4.2 s); readable connection errors; auto-saved recipes of successful runs (plain-language steps, no coordinates, failed steps dropped) shown as a hint to similar tasks, reviewed skills outrank them. 104 unit tests pass (plus 10 opt-in live tests). Live: eval `open-notepad` against the scripted model PASSED on the real desktop in 4.9 s. The Gemma/OpenAI eval numbers are not measured yet because the LM Studio server was unreachable. |
| 2026-10-29 | Harness conformance (model-free) | New `aura-cli conformance [--only x]` orders every input variation through the real executor and checks the result with a local test server (`eval_fixture.rs`: lab page reporting clicks, keys, text, scroll offsets, drags) plus real apps (Notepad, Calculator): 72 cases (14 click, 7 scroll, 12 type, 9 key, 7 drag, 5 window, 7 native app, 11 orders-by-label), **72 / 72 pass** on this machine. Executor fixes it drove: real key presses for ASCII (Calculator ignores Unicode packets), Unicode packets for the rest; Enter/Tab for newlines/tabs; extended-key flags and scan codes; shifted characters; pointer move confirmed before click; drag nudge + rest on the target before release; scrolls sent in 3-notch chunks; scroll with no position aimed at the target window; chords held ~70 ms; clipboard restored after a paste; new `mouse_move` (hover) action and `hold_keys` in the schema. Perception fix: UI Automation now searches below the window handle, because apps that draw in another process than their frame (Calculator, Settings) exposed only their title bar. `agent::driver` lets tests give orders by element label through the same observe, number, scale, guard, execute and settle path as the model (no model). Elevated windows now return a clear error instead of silently dropped input. **Incident:** a test cleanup pressed Ctrl+A + Delete in File Explorer and recycled 12 profile folders; restored from the Recycle Bin; select-all/delete is now only ever sent when the front window is Notepad (see memory `never-automate-select-all-delete`). |
| 2026-10-29 | Brain-to-body connection | Acting as the model through `aura-cli look` (screen as the model sees it + marked PNG) and `aura-cli do <order>` (one order through the real pipeline). Findings fixed: (1) **stale-screen guard**: before any input the window the model looked at must still be in front (restored if briefly stolen), unmoved, and the target point not covered by another program, otherwise the action is refused with the reason and fed back as a step (3 refusals in a row stop the run); (2) **failed actions are feedback**, not the end of the run; (3) **read-back after every action**: windows that opened or closed (`a new window opened: 'Save As'`), what has keyboard focus and what it contains, and a WARNING when typed text went to something that is not a text field; (4) OCR text is kept only if the window at that point belongs to the active program (background windows no longer pollute the element list); (5) shortcuts written as one string (`Ctrl+Shift+T`, key_press `ctrl+s`) are split into keys; (6) `open_app` waits for the new window to be in front and responsive; (7) taskbar/desktop stay valid targets. 79 conformance cases (7 new `body` cases) and 107 unit tests pass. |
| 2026-10-30 | Window awareness, scroll hints, pointing accuracy, JPEG, tough situations | Tested by acting as the model (`aura-cli look` / `do`). (1) **Window list in every prompt**: `WINDOWS OPEN: N` with a numbered, front-to-back list (title, program, ACTIVE / minimized / on another monitor); `focus_window` takes a list number (`2`, `w2`, `#2`) or a title and now restores minimized windows (it silently could not before); read-back reports `windows open: 4 -> 5`, and minimizing is no longer reported as a close. Bug found by hand: numbering was only recorded when the prompt was built, so a one-off `do focus 5` failed; `observe` now records it and the lookup falls back to a fresh list. (2) **Scroll hints**: `SCROLL POSITION` line in the prompt and a position read-back after every scroll, from UI Automation scroll data (works in Edge). (3) **Pointing accuracy** (`coords.rs`): a raw x/y that just misses an element snaps onto it (14 px); each model's consistent offset or scale error is learned from its own near misses (needs 6 consistent samples, must cut the error by 40%) and removed; element-number points are never touched; samples kept per model name in `%LOCALAPPDATA%\AgentAura\coord_calibration.json`. (4) **Speed**: model screenshots are sent as JPEG (quality 88) instead of PNG; a full observation (capture + UI Automation + OCR) takes about 0.4 s, so reusing the screen capture across steps was measured and skipped as not worth the risk. (5) **Element selection fix** found with the Open dialog: wide list rows (files) were crowded out of the 50 numbered slots by scroll-bar parts and toolbar buttons; window furniture is now last and rows win size ties. (6) **New cases** (90 total): browser alert, confirm (OK and Cancel), link that opens a new tab, hover menu, content that arrives late, disabled button, Open dialog (walk into a folder, pick a file), window list, focus a minimized window by number, minimizing is not closing, scroll read-back. 113 unit tests pass. |
| 2026-10-30 | Real websites in Chrome, driven as the model | Wikipedia (search, scroll, follow), Google (search, results), GitHub (repo, Issues), Hacker News, YouTube (permission pop-up, search), Amazon (search, scroll), BBC (open a story), a practice login form (typed username and password, submit): all worked. Findings fixed: (1) **page moved before the click** (Google results were still loading; the click landed on a YouTube link): new guard re-reads the page's elements right before a pointer action and refuses with `the screen changed since you looked: 'X' is now around (x, y)` or `no longer on the screen`; OCR targets are compared vertically only; first attempt used UI Automation hit-testing and was dropped because Chromium answers it unreliably (alert OK button, shifts not noticed). (2) **Scroll position in Chrome**: Chrome reports its page as not scrollable, so the read-back falls back to an estimate from what was sent (`about N wheel notches below where this page opened`, and `did not move, so you are at the bottom`). (3) File rows in the Open dialog lost their slots to toolbar buttons and text elsewhere: list/tree/table rows now have their own allowance of 15. (4) `aura-cli do` takes `--task <text>` (task words pull matching elements forward, as in the real loop) and `#N` to name an element by number. **Incident:** after test runs the user's Chrome (with their Claude chat) was found closed; Chrome offered Restore pages, restored via the `claude` profile; cause not proven (the PC also rebooted at 01:18, after that). Test cleanup hardened: Ctrl+W only when the front window is one a test opened, cleanup closes only Notepad, Calculator and test pages. 91 conformance cases, 115 unit tests pass. |
| 2026-10-30 | Docs refresh | All docs read and brought up to date: PROGRESS_TRACKER (status, M0-M7, audit), this plan (§2.2, §2.4, bug-ledger status, §5.9, new §5.10 and §12, milestone checkboxes), CORE, FEATURES, IMPLEMENTATION_PLAN, REVERSE_ENGINEERING_REPORT banners, README build and test section, AGENTS testing rules. |
| 2026-10-30 | HUD banner, smooth pointer, sounds | Top-centre status banner in the overlay (GDI text rendered once into a coverage mask, drawn as an anti-aliased pill with a state-coloured border and pulsing dot; new `Capturing` and `Finished` HUD states, the latter hides itself); `Capturing` is set before every screen capture. Pointer glide with smootherstep easing, time-based (accurate with Windows' 15 ms sleep granularity), kill switch checked on every step. Start/finish/stop sounds synthesised as WAV in memory and played with `PlaySoundW` (async); muted under `cargo test`. Found and fixed: the overlay was shown and hidden with blocking `ShowWindow` from other threads and the auto-hide ran while holding the state lock, which hung `aura-cli demo` and could hang the app; the overlay now owns a message-loop thread and uses `ShowWindowAsync`. New `aura-cli demo`. 122 unit tests, 91 / 91 conformance cases (with the glide). |

---

## 12. Testing the Harness

**Rule:** test the harness by acting as the model. Run `aura-cli look`, read the windows list, element list and `%TEMP%\aura-look.png`, give one order with `aura-cli do <order>`, read the measured effect, and when the harness does the wrong thing fix the harness and add a conformance case. Say what a real-desktop run will touch before starting it. A mock or a weak model is for loop tests, not for judging the body.

**Commands** (from `src-tauri/`, after `cargo build --release`): `aura-cli demo` (HUD banners, pointer glide and sounds, about 6 s), `aura-cli conformance [--only <group or name> | --only group:click,scroll,...] [--browser chrome|edge]` (`group:` selects exact groups; `--browser` opens the lab page in a separate browser instance with a throwaway profile under `%TEMP%\aura-test-browser-*`, so it never joins the user's own browser), `aura-cli look [--task <text>] [--candidates <word>]`, `aura-cli do <order> [--task <text>] [--enter] [--overwrite]`, `aura-cli run --model <id> --task <text> [--url <base>] [--max-steps N]`, `aura-cli eval --model <id> [--suite basic|web|intermediate|all] [--only <task>] [--repeat N] [--url <base>]`. Orders: click, dblclick, rclick, mclick, shiftclick, ctrlclick, hover, clickxy, drag, dragxy, scroll, type, press, hotkey, open_app, open_url, focus, wait. A label can be given as `#N` to name an element by its number, as the model does; `--task` tells the screen reader what the task is about, as the real loop does.

**Conformance suite** (`conformance.rs`, 91 cases, all passing on 2026-10-30): groups `click`, `scroll`, `type`, `key`, `drag`, `window` (lab page served by `eval_fixture.rs` and checked by what the page itself reports), `native` (Notepad, Calculator, Save As), `driver` (orders by element label: forms, links, cookie banner, hover menu, late content, disabled button, browser alert and confirm, new tab, layout shift, Open dialog) and `body` (stale screen, window gone, covered click, typed-text read-back, dialog opening/closing, window list, focus a minimized window by number, minimizing is not closing, scroll read-back). The lab pages are `/click /form /scroll /mouse /drag /cookies /links /lab /tough /shift` on `127.0.0.1:47831`.

**Real-site checks** (2026-10-30, driven as the model in a separate Chrome window): Wikipedia, Google, GitHub, Hacker News, YouTube, Amazon, BBC News and a practice login form all worked.

**Debug aids:** `look --candidates <word>` lists everything perception found (numbered or not); `AURA_DEBUG_SCROLL=1` prints the UI Automation scroll chain; `AURA_DEBUG_GUARD=1` prints what the target-moved guard compared; every run writes a trace folder under `%LOCALAPPDATA%\AgentAura\runs`.

**Safety rules for tests:** never send Ctrl+A, Delete, Ctrl+W, Alt+F4 or a window close unless the front window is verified to be one the test opened (Notepad, Calculator, "Aura Fixture" pages; `close_tab_if_front_is`); cleanup closes only those; never drive File Explorer with typing or Enter; open a separate browser window for manual checks and close only that one. Incidents: 2026-10-29 a cleanup recycled the user's profile folders (restored from the Recycle Bin); 2026-10-30 the user's Chrome was found closed after test runs (restored from Chrome's "Restore pages"; cause not proven).

**Not covered yet:** CAPTCHA and two-factor sites, payment pages, File Explorer actions, Firefox, Electron apps, UAC and other secure prompts, mixed display scaling, an app that freezes mid-action, the Chrome profile picker. Model pass rates are unmeasured.
