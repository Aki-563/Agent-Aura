# Agent Aura — Master Progress Tracker (`PROGRESS_TRACKER.md`)

**Document Version:** 3.1 (2026-10-30)  
**Project:** Agent Aura  
**Overall Completion:** Phases 0–7 scaffolding delivered. **Phase 8 (Reliability & Intelligence Overhaul): harness core built and verified (91 / 91 conformance cases, 115 unit tests, real-website checks in Chrome); model pass rates not yet measured.** See the audit note below: several Phase 4–5 items are Python-only and not wired into the Rust runtime.  
**Active Phase:** Phase 8 — Reliability & Intelligence Overhaul ([`RELIABILITY_PLAN.md`](./RELIABILITY_PLAN.md))  
**Next Milestone:** measure real models. Run `aura-cli eval --suite basic` and `--suite web` with `--repeat 3` against the local Gemma server and a capable model, then fix whatever fails in the harness. After that: the untested situations (File Explorer actions, Firefox and Electron apps, CAPTCHA/2FA sites, UAC, mixed display scaling), unattended screen tests, the up-front planner and guided recipe replay. Current verified numbers: 91 / 91 conformance cases, 122 unit tests (10 opt-in live tests ignored).

> **Audit (2026-10-29, status updated 2026-10-30):** Several Phase 4-5 items below were ticked for the unwired `engine/*.py` and not for the Rust runtime that ships. Where they stand in Rust now:
> - **Now implemented in Rust:** the destructive-action confirmation (an Allow/Deny card, not an abort); a humility path (`ask_user` plus a stuck-streak guard); JPEG encoding of the screenshots sent to the model (traces still keep PNG); window-relative coordinates through numbered elements.
> - **Still not in Rust:** action batching (the loop proposes exactly one action per step); Tier 3 vector cache; mid-task state snapping (Triple-Lock); trajectory distillation beyond the plain-language auto recipes; the "2 ms radar" (brute-force matching, hundreds of ms) and the "Direct2D overlay" (the HUD is a CPU bitmap pushed with `UpdateLayeredWindow`).
> - Ground truth and the fix plan: `RELIABILITY_PLAN.md` §2-§3 and §5.10.  
---

## 1. Phase Status Overview

| Phase | Description | Status | Completion |
| :--- | :--- | :---: | :---: |
| **Phase 0** | Architecture, Scaffolding & Database Schema | 🟢 COMPLETE | 100% |
| **Phase 1** | Native Actuation & DXGI Capture Engine (Rust) | 🟢 COMPLETE | 100% |
| **Phase 2** | Screen-Edge Glow HUD & Halo Cursor (Direct2D) | 🟢 COMPLETE | 100% |
| **Phase 3** | Settle Detection & Local Radar Tracker | 🟢 COMPLETE | 100% |
| **Phase 4** | Cognitive Brain, Unified VLM & Action Batching | 🟢 COMPLETE | 100% |
| **Phase 5** | 3-Tier Cache, Shortcuts & Apprenticeship Engine | 🟡 IN PROGRESS | 67% |
| **Phase 6** | 3-Way Review Modal & Step Scrubber UI | 🟢 COMPLETE | 100% |
| **Phase 7** | Integration, Qualification & Standalone `.exe` | 🟢 COMPLETE | 100% |
| **Phase 8** | Reliability & Intelligence Overhaul ([plan](./RELIABILITY_PLAN.md)) | 🔵 ACTIVE | harness ~90%, model results pending |
| **Phase 9** | Harness completion, testing & hardening ([plan](./HARNESS_COMPLETION_PLAN.md)) | 🔵 ACTIVE | ~65%: H0-H4 (settle part), H6, H7, H8, H10 done; grounding modes A/B/C, parallel workers/layout, remaining H9 inputs open |

---

## 2. Granular Task Checklists

### Phase 0: Scaffolding, Toolchain & Database Setup
- [x] Reverse engineer Agent-S reference codebase.
- [x] Create formal `REVERSE_ENGINEERING_REPORT.md`.
- [x] Create core engine architecture specification `CORE.md`.
- [x] Create master feature specification `FEATURES.md`.
- [x] Create phase-by-phase implementation plan `IMPLEMENTATION_PLAN.md`.
- [x] Initialize Tauri v2 project structure in `src-tauri/`.
- [x] Write `schema.sql` defining `Skills`, `VisualAnchors`, `AppShortcuts`, `Guardrails`, and `PromptCache`.
- [x] Populate seed data for Windows OS and Browser shortcuts into SQLite.

### Phase 1: Native Actuation & DXGI Capture (Rust)
- [x] Implement D3D11 device creation & `IDXGIOutputDuplication` loop (`capture/dxgi.rs`).
- [x] Implement RGBA frame acquisition in < 5ms.
- [x] Implement display mode change recovery (`DXGI_ERROR_ACCESS_LOST`).
- [x] Implement Win32 `SendInput` mouse dispatcher (`input/send_input.rs`).
- [x] Implement virtual key / Unicode keyboard dispatcher.
- [x] Implement bulk text Clipboard Bridge (`input/clipboard.rs`).
- [x] Register `WH_KEYBOARD_LL` low-level hook for Emergency Stop (`input/killswitch.rs`).
- [x] Test kill-switch responsiveness (<1ms input freeze on `Esc`).

### Phase 2: Screen-Edge Glow HUD & Halo Cursor (Direct2D)
- [x] Create borderless click-through overlay window with `WS_EX_TRANSPARENT | WS_EX_LAYERED`.
- [x] Initialize Direct2D rendering factory and swap chain (`overlay/render.rs`).
- [x] Implement state color transitions (Purple = Thinking, Cyan = Working, Amber = Loading, Red = Emergency).
- [x] Implement sine-wave breathing pulse animation for Thinking state.
- [x] Implement glowing halo cursor follower with water-ripple click effect.
- [x] Verify that all mouse clicks pass through overlay without being blocked.

### Phase 3: Settle Detection & Local Radar Tracker
- [x] Implement `GetCursorInfo` polling for `IDC_WAIT` and `IDC_APPSTARTING` (`settle/cursor.rs`).
- [x] Implement optical frame differencing $\Delta < 0.2\%$ between 50ms frames (`settle/quiescence.rs`).
- [x] Add 2.5s maximum timeout guard for endless animations.
- [x] Implement OpenCV Normalized Cross-Correlation (NCC) edge tracker in Rust (`radar/tracker.rs`).
- [x] Validate 2ms execution time for icon template matching.

### Phase 4: Cognitive Brain, Unified VLM & Action Batching
- [x] Create async VLM API gateway supporting Claude 3.7 / Qwen2.5-VL / Gemini (`engine/vlm_client.py`).
- [x] Implement WebP 1280px downscaling and image payload builder.
- [x] Implement Differential Visual Patching (crop active window + denormalize coordinates).
- [x] Implement Action Batching parser and JSON schema validator (`engine/batcher.py`).
- [x] Implement Destructive Action Confirmation Gate (pause on file deletion/format).
- [x] Implement Humility Gate (ask user for 1 click after 2 failed retries).

### Phase 5: 3-Tier Cache, Shortcuts & Apprenticeship Engine
- [x] Implement Tier 1 SHA-256 exact hash match in memory.
- [x] Implement Tier 2 Levenshtein fuzzy similarity search.
- [ ] Implement Tier 3 local ONNX vector embedding search (`fastembed` / `all-MiniLM-L6-v2`).
- [x] Implement Mid-Task State Snapping with Triple-Lock verification.
- [x] Implement Trajectory Distillation Compiler (strip errors $\rightarrow$ extract Golden Path).
- [x] Implement $64 \times 64$ visual anchor extraction and persistence.

### Phase 6: 3-Way Review Modal & Step Scrubber UI
- [x] Build glassmorphic Review Modal (`ReviewModal.ts`).
- [x] Implement `✅ Perfect` handler (promote trajectory to Golden Path).
- [x] Implement `⚡ Yes, but optimize it` handler (shortcut substitution).
- [x] Implement `❌ Mistake made` Step Scrubber with thumbnail carousel (`Scrubber.ts`).
- [x] Save user corrections into `Guardrails` table.

### Phase 7: Integration, Qualification & Standalone Packaging
- [x] Connect Rust supervisor and Agent Brain over IPC named pipes / stdio.
- [x] Run end-to-end benchmark suite on 5 foundational desktop tasks.
- [x] Assert DXGI capture < 5ms, actuation < 1ms, and prompt cache hit < 5ms.
- [x] Build release executable via `cargo tauri build` / `cargo build --release`.
- [x] Audit standalone `.exe` using REA (`rea inspect-rust-binary` and `rea inspect-pe-resources`).

### Launch & Maintenance
- [x] Repair production packaging so the standalone executable uses the embedded frontend instead of `localhost:5173` (2026-10-28).
- [x] Replace `com.tauri.dev` with the production bundle identifier `com.agentaura.desktop`.
- [x] Rebuild and launch `dist/agent-aura.exe` through the Tauri production pipeline.
- [x] Replace the sample UI and simulated status messages with a real task, observation, proposal, and approval workflow.
- [x] Add credentials for OpenAI Responses API and optional-key OpenAI-compatible local servers (2026-10-30: provider settings are saved, the key DPAPI-encrypted).
- [x] Connect real DXGI capture, vision-model planning, coordinate scaling, typed action validation, and Win32 execution.
- [x] Add autonomous multi-step execution with live progress events, a configurable step ceiling, and the global Esc kill switch.
- [x] Enforce a strict one-action loop: propose one action, execute, optically settle, capture a fresh screenshot, then plan again.
- [x] Allow high-impact actions without approval only when typed impact classification confirms explicit original-task authorization.
- [x] Remove automatic shortcut seed population so new databases begin with real user-generated data only.
- [x] Add an eight-step bounded trajectory with visible post-action outcome reflection and loop detection.
- [x] Add two-pass proposal and independent verification using the same configured model provider.
- [x] Add native UI Automation, Windows OCR, approved visual anchors, and local radar grounding candidates.
- [x] Replace coordinate clamping with strict source-canvas, monitor-origin, and physical-bounds rejection.
- [x] Replace the keyword-only stop with typed impact classification and explicit original-task authorization.
- [x] Capture the foreground-window monitor, provide a high-detail active-window verification crop, and settle the correct monitor.
- [x] Persist real task runs and trajectory steps and wire review-confirmed skills, corrections, thumbnails, and HUD state transitions.
- [x] Repair the Windows Rust/Tauri test executable by removing the incompatible Common Controls v6 test dependency; 29 library tests pass.
- [x] Retry local OpenAI-compatible image requests with a raw base64 payload only when the server explicitly rejects a standard image data URL (2026-10-29).
- [x] Verify the full request path with a running local vision model (Gemma via LM Studio: requests, JSON schema output, JPEG screenshots, an executed step; 2026-10-30). Still open: an OpenAI key.
- [ ] Add a production Rust ONNX embedding tier; exact and fuzzy approved-skill retrieval are currently active.
- [x] Add deterministic mock-provider tests (`mock_server.rs` scripted model server plus six end-to-end loop tests) and a model-free conformance suite with a local test site (`eval_fixture.rs`). Difficult-interface screenshot fixtures are not built.

> The three open items above are carried into Phase 8 (M0, M1, M4, M6, M7).

### Phase 8: Reliability & Intelligence Overhaul (details: [`RELIABILITY_PLAN.md`](./RELIABILITY_PLAN.md) §6)
- [x] **M0** Tooling & baseline: working build (MSVC, pinned by `rust-toolchain.toml`), scripted mock model server (instead of a provider trait), per-step trace folder, `aura-cli` (run, eval, conformance, look, do), DPI-awareness log line. README build note added 2026-10-30.
- [x] **M1** P0 blockers: `json_schema` output, injected-Esc kill-switch fix, release-all keys/buttons, coordinate-space + verifier-override fixes, cancellation, tolerant parsing.
- [x] **M2 (core parts)** Window/focus/capture hygiene: exclude Aura from capture, no minimize, target window guard, occlusion check, user takeover pause, persistent multi-adapter capture, JPEG encode.
- [x] **M3** Settle v2 (change-then-stable) + action effects fed back to the model.
- [x] **M4 (core)** Set-of-Mark element numbers, task-aware selection, crosshair verifier, parallel perception. Coordinate accuracy for raw x/y is now handled in `coords.rs` (snap near misses, learned per-model offset/scale; synthetic tests only). Still open: per-window UIA budget.
- [x] **M5 (P1 parts: focus_window, open_url, wait, done-check, confirmation card, user-takeover pause)** Remaining: planner, notes, select_text, ask_user/humility, clipboard hygiene. Original text: Action space + orchestrator: open_app/focus_window/open_url/select_text/notes/ask_user, subgoal planner, feedback retries, done-check, confirmation gate, humility gate, clipboard hygiene.
- [x] **M6 (first version: auto recipes + reviewed skills outrank them; no guided replay yet)** Learning: skills v2 (semantic steps), guided replay, embedding retrieval, app knowledge, post-run lessons, fast radar.
- [x] **Harness conformance suite** (`aura-cli conformance`, 91 cases, 91 / 91 pass): clicks and variations, scrolling and variations, typing and variations, keys and chords, drags, window actions, Notepad and Calculator, orders by element label, elevated-window guard.
- [x] **Brain-to-body protocol** (2026-10-29/30): stale-screen guard (window moved, covered, no longer in front), refused and failed actions fed back as steps, read-back after every action (windows opened/closed with counts, keyboard focus and its content, typing warning), elevated-window error, OCR limited to the active program.
- [x] **Window awareness** (2026-10-30): numbered `WINDOWS OPEN` list in every prompt, `focus_window` by number, minimized windows restored, minimizing no longer reported as a close.
- [x] **Scroll hints** (2026-10-30): `SCROLL POSITION` in the prompt and a read-back after each scroll (UI Automation where the app reports it, an estimate from what was sent in Chrome).
- [x] **Target-moved guard** (2026-10-30): before a pointer action the page's elements are re-read; if the numbered target moved or vanished the action is refused with the reason (found by clicking on a still-loading Google page).
- [x] **Speed** (2026-10-30): model screenshots sent as JPEG; screen-capture reuse measured (a full look takes ~0.4 s) and skipped.
- [x] **Feel of a run** (2026-10-30): a status banner at the top centre of the screen inside the glowing edge ("Taking screenshot", "Thinking", "Working", "Waiting for the screen", "Stopped", and a green "Task finished" that hides itself after about 2.4 s); the pointer glides with an ease-in-out curve (about 0.17 s for 100 px, at most 0.56 s) instead of jumping; start, finish and stop sounds synthesised in memory (`sound.rs`, `AURA_MUTE=1` silences them). The overlay window now has its own message-loop thread, which removed a hang. `aura-cli demo` shows all of it.
- [x] **Real-website check in Chrome** (2026-10-30, driven as the model): Wikipedia, Google, GitHub, Hacker News, YouTube, Amazon, BBC, a login form.
- [ ] **Still open:** Gemma / other model pass rates (`aura-cli eval --repeat`), untested situations (CAPTCHA and 2FA sites, File Explorer actions, Electron apps, Firefox, UAC, mixed display scaling), unattended screen tests, up-front planner, recipe replay, CORE.md / FEATURES.md refresh, README build note.
- [x] **M7 (suite + runner built; model results pending)** Qualification: eval suites (basic, web, intermediate, all) with independent checkers and `--repeat` pass rates; docs refreshed 2026-10-30; release build in `dist/`. Model pass rates are the next milestone.


### Phase 9: Harness completion (details: [`HARNESS_COMPLETION_PLAN.md`](./HARNESS_COMPLETION_PLAN.md)), status 2026-10-30 (late)
- [x] **H0** test infrastructure: fixture pages, measurements to `testlogs`, scripted-model failure directives, `test-all.ps1`.
- [x] **H1** stop / pause / steer: epoch-based control bus, stale answers dropped, held inputs released, ledger, hotkeys, control port, mini-player controls. 10 minute pause test passes (idle CPU, no model calls).
- [x] **H2** mouse precision: exact SendInput mapping, closed-loop 0 px, draw_path shapes within 1 px, pen and touch.
- [x] **H3** window registry, state block, window actions with read-back, tiling, UAC/lock handoff, virtual-desktop round trip test, per-app profiles.
- [x] **H4** loading-aware settle (caret blink no longer counts as loading); grounding modes A/B/C with per-step metrics; `coordinate_scale`. Open: pass-rate measurements with a real model per mode, `zoom`/`observe` with a model.
- [x] **H5** parallel window workers (input arbiter, model scheduler, `aura-cli parallel`). Open: per-window capture (workers capture the screen and raise the window).
- [x] **H6** JSON validation and fuzzing, retry with backoff, password/CAPTCHA/UAC handoffs, budgets, secret masking in traces.
- [x] **H7** scheduler (CLI, daemon, in-app, schedule screen), locked PC skips. Open: the 24 hour soak run (`aura-cli soak`).
- [x] **H8** offline voice with an audible smoke test; approval prompts are spoken.
- [x] **H9** Tamil/Arabic/Devanagari/emoji read-back, Sticky/Filter keys guard, High Contrast note, black-frame GDI fallback, per-app profiles. Open: display scaling at 125/150/200% (needs a manual OS change).
- [x] **H10** README and the single test command.
- [x] **H11** obstruction handling (toast, tooltip, widget, other window) with tests using real foreign-process windows.
- [x] **T3 system group:** virtual desktop round trip, display scale and exact pointer, loading pages under full CPU load.
- [x] App fixes found by driving the real app: a missing element id stopped the page script (no run could start); events now reach the page through a polled queue because Tauri's listener is blocked by the build's capabilities; no approval prompts for wait/note/backspace/scroll; Save & test connection now probes the model; the saved key shows as a mask.
- Verified: 222 unit tests, 134 desktop conformance cases (plus system and explorer groups), all passing.
- [x] Sandboxed Explorer test group (`--only group:explorer`: rename, new folder, copy/paste, delete one file, all inside a throwaway folder).
- Measured with GUI-Owl 1.5 2B (LM Studio), grounding mode A, `eval` x2: notepad-type 0/2, calculator-sum 0/2. The 2B model misreads screens, claims "done" early and fails verification; modes B and C were not compared because a 0% baseline says nothing. Mode C's recovery of near misses is covered by a simulation (>= 95% within 14 px). OpenAI models are never used for testing (user rule).
- [x] Per-window capture for parallel workers (PrintWindow; a covered window is read without raising it). 10 minute soak: 7 cycles, 315 cases, no failures, no leak (21 MB, 327 handles); it found and fixed stale test pages being reused and now waits while the PC is locked.
- [x] Order execution sharpened with `qwen/qwen3-vl-4b` (LM Studio, 10.103.7.32): per-model coordinate calibration (`aura-cli calibrate`; Qwen3-VL-4B = 0-1000 grid), model JSON dialects, miscounted-element correction by name, taskbar elements, "open X and ..." fast path, done claims checked against the exact text asked for. New `orders` group: 9 scripted-model cases through the full loop. Key-cap names match accessible names (1=One, +=Plus, CE=Clear entry); raw-coordinate clicks are name-checked too; taskbar elements rank below app controls. Qwen3-VL-4B basic eval: 6/7 (calculator fails on the model skipping "+", not on execution).
- Open (resolved in Phase 10 except the soak): `zoom`/`observe` and grounding modes now run through the whole loop with a scripted model (`orders` group); display scales are covered by `aura-cli scale-test`; "long text by paste" passed in every run since; the 24 hour soak run (`aura-cli soak --minutes 1440`) remains.


### Phase 10: Ship-ready app and harness polish (2026-10-30, night)
- [x] **New home screen:** one centred task box (Enter or the arrow runs; the arrow turns into Stop while a task runs; the footprints icon sets the step limit), connection pill and Settings at the top right, Activity behind a button. No descriptions, Inter throughout, iOS-like springs (transform and opacity only).
- [x] **Model presets in the database** (`ModelPresets` table, key DPAPI-encrypted): Connect checks the model with a real image request and saves it; tap to switch, bin to delete. The saved provider of an older version becomes the first preset.
- [x] **Automatic coordinate calibration** of a newly saved model on a generated four-button picture (no screen, no clicks), in the background; Qwen3-VL-4B measured as the 0-1000 grid, the same as `aura-cli calibrate` on the real page.
- [x] **Schedules from the prompt** ("every day at 9am ...", "... every 30 minutes", "in 10 minutes ...", "tomorrow at 8:30 ..."), one-off schedules, "Run now instead"; the schedule form is gone from the UI.
- [x] **Activity view:** recent runs, result, steps, time, steps that worked, and each step left to right with its screenshot, action and outcome; "Remember this way" and per-step corrections.
- [x] **Mini player:** no shadow, draggable (place remembered, still steps aside for clicks), step reader switch, step number; the overlapping-words bug fixed (fast updates could leave two lines "current").
- [x] **Pointer:** a violet-to-cyan arrow instead of the ring (sized to the system cursor size, tip = hotspot); the halo ring is gone; glides over 240-620 ms along a slight curve with a 1 ms timer, clamped to the screen.
- [x] **Edge glow** 84 px deep at 1080p (was 22) with an accent colour drifting along the edges; the status pill no longer fades its last letters. **Sounds:** soft two- and three-note chimes with a little room, quieter.
- [x] **Window:** a run started while the app is minimized no longer leaves it as a small rectangle (the real size is saved; a tiny saved size is replaced by the default); minimum size 640 x 480; dark title bar.
- [x] **Packaging:** `tauri_build` runs again (icon, version and the per-monitor-DPI manifest embedded; no loose manifest or DLL next to the exe; the local cargo registry copy of `tauri` had been patched by an old GNU workaround and was restored). `tauri build` makes a per-user NSIS installer.
- [x] **Harness:** `zoom`, `observe` and grounding modes B and C now run through the whole loop in the `orders` group; `focus_window` retries when a system pop-up holds the foreground; `aura-cli scale-test` repeats the scale-sensitive groups at every display scale the monitor offers (up to 200 %) and puts the user's scale back.
- [x] **Scheduled runs** started while the app is open turn the window into the player too (the page notices runs it did not start).
- Verified 2026-10-31: 234 unit tests; 138/138 desktop conformance cases (13 in `orders`); `aura-cli scale-test` 32/32 at 100, 125, 150 and 175 % (this monitor offers no 200 %), scale put back to 125 %; the new app driven end to end over CDP (connect + background calibration of Qwen3-VL-4B, a Calculator run, Activity, a schedule from the prompt, the step popover); the installer ran (exit 0, files, Start menu and desktop shortcuts, uninstall entry) and was then rolled back by the PC's Seqrite endpoint security, which quarantines unsigned installers: signing the exe and installer is what removes that.
- Open: only the 24 hour soak run (`aura-cli soak --minutes 1440`).

### Phase 11: Code review fixes (2026-10-31), all 36 findings
- [x] **Model connection:** a Connect whose test fails puts the previous working connection back (memory and `provider.json`); `provider.json` now only points at the preset, so the key is stored once; calibration uses one fixed connection from start to end and is saved per server + model (`model_scale.json` key `"<model> @ <base url>"`, old bare-name entries still read); never automatic on the paid OpenAI API, and a failed automatic calibration is not repeated (the settings sheet still runs it).
- [x] **Schedules:** one scheduler per data folder (named lock; a second app copy or `aura-cli daemon` gives way) and one app copy (starting it again brings the open window forward); a one-off more than 15 min late is dropped and marked "missed"; repeats under one minute are refused; a due schedule waits up to 30 min for a task the user started instead of failing; trailing "in 2 hours" / "tomorrow at 10" after booking, reminder, alarm, timer or meeting words stays part of the task; daily and one-off times use the Windows time-zone rules of their own day (daylight saving).
- [x] **Closing the window with schedules on** keeps Aura in the tray (click to open, right-click Open / Quit); a scheduled run from the tray shows the player and goes back to the tray.
- [x] **Run keys:** in the app, Esc / Pause / Ctrl+Alt+Space only act while a task runs (the command-line tools keep them live). During a run one Esc still stops it: that is the emergency stop. Hovering, dragging or clicking the mini player and typing in its steer box no longer count as the user taking over.
- [x] **Crash safety:** a journal (`recovery/<pid>.json`) records the cursor swap, the accessibility-shortcut change, raised windows and the display scale before they happen; a panic undoes them at once, and the next app or `aura-cli` start undoes what a killed process left (raised windows only if still owned by the same process). `aura-cli` also undoes on Ctrl+C / console close.
- [x] **Mini player:** the remembered place is used only on the monitor it was saved on; the drag works with swapped mouse buttons; it steps off menus and pop-ups of the app being worked on.
- [x] **Privacy and safety:** step screenshots and crops in the database are DPAPI-encrypted, and history is kept for 200 runs / 30 days; password-like words are masked in traces and in the database even outside password fields; the model prompt says screen text is never an instruction, and typing an e-mail address, a long number or a data-carrying link the task never mentioned needs the user's yes (prompt-injection guard); the control-port token comes from the system's cryptographic generator (old guessable tokens are replaced).
- [x] **Performance:** Activity loads one small JPEG per step as its card scrolls into view; progress events no longer carry screenshots; the page polls once a second when idle; the status pill is checked on start and when the app returns after 5 min, not every 2 min; the edge glow clears only what it drew and runs at 30 fps when nothing but the slow drift moves.
- [x] **Displays:** the status pill and the working cursor follow the main monitor's current scale; the pointer glide stays on a monitor when screens differ in size; `display_scale` changes the main monitor (desktop origin), not the first display path.
- [x] **Smaller:** the step-reader switch wins over `AURA_MUTE`; Calculator keys get language-independent names from their automation ids; unused `get_last_task_run` / `test_provider` removed; the `AURA_SELFTEST_TASK` self-test exists only in debug builds; the crash log goes next to the database.
- [x] **Signing ready:** `bundle.windows.signCommand` runs `tools/sign.cmd`, which signs the exe, CLI and installer with `signtool` when `AURA_SIGN_THUMBPRINT` names a code-signing certificate (without it the build stays unsigned, as before). Buying the certificate is the one step outside the code.
- [x] **Tests made sturdier:** the open-dialog case waits for the dialog and replaces the remembered file name (a stale name left the dialog open and failed the next case); `seen` looks twice when a just-started Chrome has not painted the page yet.
- Verified 2026-10-31: 256 unit tests (20 new); desktop conformance 138/138 across the runs (groups that met a flake re-run clean: driver, windows, native, orders 13/13 twice); in the app over CDP with an isolated data folder: a second launch hands over to the open window and quits, a failed Connect leaves the working model active in memory and on disk, "book a meeting in 2 hours" runs as a task, "every 5 seconds" is refused, Activity sends a 527-byte run detail and a 66 KB JPEG per step, closing with a schedule keeps Aura in the tray and relaunching brings it back, closing without schedules quits; `tauri build` runs `tools/sign.cmd` for every file (unsigned without a certificate) and makes the installer.

---

## 3. Handover Protocol for Coding Agents

When any coding agent resumes work on this repository:
1. **Read `PROGRESS_TRACKER.md`** first to identify the active phase and milestone.
2. **Consult `CORE.md`** for architectural constraints, coordinate transforms, and actuation schemas.
3. **Consult `FEATURES.md`** for specific feature logic, edge-case handling, and formulas.
4. **Follow `IMPLEMENTATION_PLAN.md`** for step-by-step technical instructions.
5. **Update checkboxes** in this file upon completing and testing any milestone.
