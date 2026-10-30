# Agent Aura — Master Implementation Plan (`IMPLEMENTATION_PLAN.md`)

**Document Version:** 1.1 (2026-10-30, as-built layout added)  
**Project:** Agent Aura  
**Target Environment:** Windows 10/11 x64  
**Primary Purpose:** Comprehensive, phase-by-phase implementation blueprint for coding agents to build, test, and package Agent Aura from scratch into a high-performance desktop application.

---

> **Status (2026-10-30).** Phases 0-7 below describe the original plan. The project is now in Phase 8 (`RELIABILITY_PLAN.md`), which supersedes them where they differ. In particular: the brain is Rust (`engine/*.py` is unwired and deprecated), there is one binary rather than a supervisor plus a Python worker, and the as-built layout is in section 2b.

---

## 1. Technology Stack Decisions

| Layer | Technology | Rationale |
| :--- | :--- | :--- |
| **Desktop Host & UI** | **Tauri v2 (Rust + HTML/TS)** | Minimal footprint (<15MB RAM), native OS integration, zero Electron bloat. |
| **Native Actuation & Capture**| **Rust (`windows-sys` / `winapi`)** | Sub-millisecond DXGI Desktop Duplication, Win32 `SendInput`, raw keyboard hooks. |
| **Visual Overlay / HUD** | **Direct2D / DirectX Layered Window** | True click-through overlay (`WS_EX_TRANSPARENT`), 60 FPS GPU-accelerated neon glow. |
| **Local Vector Search** | **Rust `fastembed` / ONNX Runtime** | 3ms CPU vector embeddings (`all-MiniLM-L6-v2`) without cloud dependencies. |
| **Database & Cache** | **SQLite via `rusqlite` + `sqlite-vec`** | Single local `.db` file, zero-latency in-memory and disk relational/vector indexing. |
| **Agent Reasoning Brain** | **Python (or Rust) Unified VLM Gateway**| Connects to Claude 3.7 Computer Use, Qwen2.5-VL, or Gemini 2.0 Flash via async HTTP. |
| **Computer Vision (Radar)** | **Rust `image` + OpenCV (NCC matching)**| 2ms local template matching for moved icons. |

---

## 2. Target Project Directory Structure

```
Agent Aura/
├── reference/                  # Read-only reference code (Agent-S)
│   └── Agent-S/
├── src-tauri/                  # Rust Supervisor & Native Core
│   ├── Cargo.toml
│   ├── src/
│   │   ├── main.rs             # Tauri application entry point & IPC router
│   │   ├── capture/
│   │   │   ├── mod.rs
│   │   │   └── dxgi.rs         # Direct3D 11 / DXGI Desktop Duplication capture
│   │   ├── input/
│   │   │   ├── mod.rs
│   │   │   ├── send_input.rs   # Win32 SendInput actuation (mouse, keys, unicode)
│   │   │   ├── clipboard.rs    # Win32 Clipboard bridge for instant bulk paste
│   │   │   └── killswitch.rs   # Low-level keyboard hook (Esc / Ctrl+Pause)
│   │   ├── overlay/
│   │   │   ├── mod.rs
│   │   │   ├── window.rs       # Layered click-through borderless window
│   │   │   └── render.rs       # Direct2D animated neon edge glow & halo cursor
│   │   ├── settle/
│   │   │   ├── mod.rs
│   │   │   ├── cursor.rs       # GetCursorInfo IDC_WAIT & IDC_APPSTARTING poll
│   │   │   └── quiescence.rs   # Perceptual frame differencing (pHash delta)
│   │   ├── radar/
│   │   │   ├── mod.rs
│   │   │   └── tracker.rs      # Local OpenCV NCC edge-gradient icon finder (2ms)
│   │   └── db/
│   │       ├── mod.rs
│   │       ├── schema.sql      # Database tables (Skills, Shortcuts, Guardrails)
│   │       ├── store.rs        # SQLite CRUD operations
│   │       └── vector.rs       # Local ONNX vector embedding & cosine search
├── engine/                     # Cognitive Agent Brain & Protocols
│   ├── requirements.txt
│   ├── vlm_client.py           # Unified single-pass API client (Claude/Qwen/Gemini)
│   ├── batcher.py              # Multi-action chain validator and parser
│   ├── prompt_builder.py       # Speed-optimized system prompt & shortcut injection
│   ├── cache_manager.py        # 3-tier semantic cache controller
│   ├── skill_compiler.py       # Apprenticeship Golden Path & Anchor extractor
│   └── bridge.py               # JSON-lines IPC bridge to Rust Supervisor
├── ui/                         # Lightweight Desktop HUD & Review Frontend
│   ├── index.html
│   ├── src/
│   │   ├── main.ts
│   │   ├── components/
│   │   │   ├── ReviewModal.ts  # 3-Way review modal (Perfect / Optimize / Mistake)
│   │   │   ├── Scrubber.ts     # Visual step scrubber with thumbnail carousel
│   │   │   └── CautionGate.ts  # Destructive action confirmation popup
│   │   └── styles/
│   │       └── hud.css         # Glassmorphic, modern styling
├── REVERSE_ENGINEERING_REPORT.md
├── CORE.md
├── FEATURES.md
├── IMPLEMENTATION_PLAN.md
└── PROGRESS_TRACKER.md
```

### 2b. Layout as built (2026-10-30)

```
src-tauri/src/
├── agent.rs            # the run loop (observe -> propose -> guard -> act -> settle -> read back), prompts, guards, `driver` module for tests
├── model_schema.rs     # JSON schemas for model replies, tolerant parsing, action primitives
├── perception.rs       # window context, UI Automation + OCR candidates, focused-element read-back, scroll position, window-relative geometry
├── marks.rs            # numbered elements: selection, ranking, drawing
├── coords.rs           # snapping of near misses, learned per-model pointing error
├── capture/            # DXGI duplication, GDI fallback
├── input/              # SendInput actuator, clipboard bridge, kill switch
├── desktop/            # target window tracking, window list, foreground and occlusion guards, display affinity, side panel
├── settle/             # cursor-busy check, quiescence, change-then-stable effect detector
├── overlay/            # screen-edge glow and halo cursor
├── radar/              # template tracker, used only for approved visual anchors
├── db/                 # SQLite: runs, steps, approved skills, guardrails
├── recipe.rs, reliability.rs, trace.rs   # auto recipes, impact classification and task session, per-run trace folders
├── eval.rs, eval_fixture.rs, mock_server.rs, loop_tests.rs   # eval suites, local test site, scripted fake model, end-to-end loop tests
├── conformance.rs, driver_cli.rs         # model-free harness tests; `aura-cli look` / `do`
└── bin/aura-cli.rs     # headless runner: run, eval, conformance, look, do
ui/                     # TypeScript frontend (live panel, approval card, review modal, scrubber)
dist/                   # the shipped agent-aura.exe, its manifest and WebView2Loader.dll
```

---

## 3. Phased Implementation Roadmap

```
  Phase 0: Scaffolding & Database Schema
     │
     ▼
  Phase 1: Native Actuation & DXGI Capture (Rust)
     │
     ▼
  Phase 2: Screen-Edge Glow HUD & Halo Cursor
     │
     ▼
  Phase 3: Settle Detection & Local Radar Tracker
     │
     ▼
  Phase 4: Brain Engine, Unified VLM & Action Batching
     │
     ▼
  Phase 5: 3-Tier Cache, Shortcuts & Apprenticeship DB
     │
     ▼
  Phase 6: 3-Way Review Modal & Step Scrubber UI
     │
     ▼
  Phase 7: End-to-End Testing, Hardening & Standalone Packaging
```

---

### Phase 0: Scaffolding, Toolchain & Database Setup
* **Milestone 0.1**: Initialize Tauri v2 project structure in `src-tauri` with clean Cargo configuration.
* **Milestone 0.2**: Create the SQLite schema file (`schema.sql`):
  * `Skills` table: `id`, `prompt`, `normalized_intent`, `golden_path_json`, `created_at`.
  * `VisualAnchors` table: `id`, `skill_id`, `step_index`, `crop_blob`, `coords_x`, `coords_y`.
  * `AppShortcuts` table: `id`, `app_process`, `semantic_intent`, `hotkey_combo`, `source`.
  * `Guardrails` table: `id`, `skill_id`, `faulty_step_index`, `forbidden_action`, `correction_text`.
  * `PromptCache` table: `sha256_hash`, `prompt_text`, `embedding_blob`, `skill_id`.
* **Milestone 0.3**: Populate seed shortcut data for Windows OS, Chrome, Edge, and Excel.

---

### Phase 1: Native Actuation & DXGI Capture (Rust)
* **Milestone 1.1: DXGI Desktop Duplication (`capture/dxgi.rs`)**:
  * Initialize D3D11 device and acquire `IDXGIOutputDuplication`.
  * Implement `capture_frame()` returning raw RGBA buffers in **< 5ms**.
  * Add automatic recovery for `DXGI_ERROR_ACCESS_LOST` (display mode changes).
* **Milestone 1.2: Native Input Actuation (`input/send_input.rs`)**:
  * Implement `click(x, y, button, clicks)` with absolute virtual desktop mapping.
  * Implement `drag_and_drop(x1, y1, x2, y2, duration)`.
  * Implement `type_unicode(text)` via `KEYEVENTF_UNICODE`.
  * Implement `hotkey(keys)` supporting `Ctrl`, `Alt`, `Shift`, `Win`, and function keys.
* **Milestone 1.3: Clipboard Bridge (`input/clipboard.rs`)**:
  * Direct Win32 clipboard setter for strings > 20 characters, followed by `Ctrl + V`.
* **Milestone 1.4: Low-Level Emergency Kill-Switch (`input/killswitch.rs`)**:
  * Register `WH_KEYBOARD_LL` hook.
  * If `Esc` or `Ctrl + Pause` is pressed, immediately set an atomic boolean flag, cancel pending input batches, and alert the supervisor.

---

### Phase 2: Screen-Edge Glow HUD & Halo Cursor (Direct2D)
* **Milestone 2.1: Transparent Layered Window (`overlay/window.rs`)**:
  * Create a top-level window spanning `GetSystemMetrics(SM_CXVIRTUALSCREEN)`.
  * Set flags: `WS_EX_TOPMOST | WS_EX_LAYERED | WS_EX_TRANSPARENT`.
  * Ensure mouse clicks pass through completely to underlying applications.
* **Milestone 2.2: Screen-Edge Animated Glow (`overlay/render.rs`)**:
  * Direct2D rendering loop for glowing outer display borders.
  * State transitions:
    * `Thinking`: Deep Electric Purple (1.5s sine wave breathing pulse).
    * `Working`: Neon Cyan (Steady glow with corner flow).
    * `Settling`: Warm Amber (Soft shimmer).
    * `Paused`: Safety Red (Double flash).
* **Milestone 2.3: Halo Cursor & Water Ripple**:
  * Track cursor position; render a 24px glowing circular halo at the pointer.
  * Expand ring into an outward ripple on click event.

---

### Phase 3: Settle Detection & Local Radar Tracker
* **Milestone 3.1: Hardware Cursor Monitor (`settle/cursor.rs`)**:
  * Query `GetCursorInfo`. If `hCursor` matches `IDC_WAIT` or `IDC_APPSTARTING`, enter wait loop.
* **Milestone 3.2: Optical Frame Quiescence (`settle/quiescence.rs`)**:
  * Compare consecutive 50ms downsampled frames.
  * When mean pixel delta $\Delta < 0.2\%$, trigger settle flag.
  * Enforce hard safety ceiling (max 2.5 seconds).
* **Milestone 3.3: Local Radar Icon Tracker (`radar/tracker.rs`)**:
  * Implement OpenCV Normalized Cross-Correlation (NCC) with edge-gradient contours.
  * Return $(X', Y')$ within 2ms if match confidence $\ge 0.88$.

---

### Phase 4: Cognitive Brain, Unified VLM & Action Batching
* **Milestone 4.1: Unified VLM Gateway (`engine/vlm_client.py`)**:
  * Implement asynchronous provider connectors (Claude 3.7 Computer Use, Qwen2.5-VL, Gemini 2.0 Flash).
  * Handle image payload downscaling to 1280px with WebP compression.
* **Milestone 4.2: Differential Visual Patching**:
  * When target window is known, crop only active window rect.
  * Reconstruct physical screen coordinates via $(X_{\text{screen}} = \text{Window}_{\text{left}} + X_{\text{crop}})$.
* **Milestone 4.3: Action Batching Parser (`engine/batcher.py`)**:
  * Enforce strict JSON action schemas.
  * Validate coordinate boundaries against physical screen limits.
* **Milestone 4.4: Destructive Action & Humility Gates**:
  * Flag dangerous actions (*"Delete"*, *"Format"*, *"Drop"*) and pause for confirmation.
  * If a step fails twice, trigger Humility Gate asking the user for 1 click.

---

### Phase 5: 3-Tier Cache, Shortcuts & Apprenticeship Engine
* **Milestone 5.1: 3-Tier Semantic Prompt Cache (`engine/cache_manager.py`)**:
  * Tier 1: SHA-256 exact match.
  * Tier 2: Levenshtein token similarity.
  * Tier 3: Local ONNX embedding cosine search (`all-MiniLM-L6-v2`).
* **Milestone 5.2: Mid-Task State Snapping**:
  * Implement Triple-Lock verification (Process match + Visual anchor match + Goal compatibility).
  * Switch dynamically from LLM exploration to muscle memory replay.
* **Milestone 5.3: Trajectory Distillation Compiler (`engine/skill_compiler.py`)**:
  * On successful task completion, prune away failed clicks and hesitations.
  * Extract the minimal Golden Path and save $64 \times 64$ visual anchors.

---

### Phase 6: 3-Way Review Modal & Step Scrubber UI
* **Milestone 6.1: Desktop Review Modal (`ui/src/components/ReviewModal.ts`)**:
  * Render modern glassmorphic modal with `✅ Perfect`, `⚡ Yes, but optimize it`, and `❌ Mistake made`.
* **Milestone 6.2: Shortcut Optimization Input**:
  * If `⚡ Yes, but optimize it` is clicked, accept user hotkey input, rewrite trajectory in SQLite, and generalize to `AppShortcuts`.
* **Milestone 6.3: Visual Step Scrubber (`ui/src/components/Scrubber.ts`)**:
  * If `❌ Mistake made` is clicked, display thumbnail carousel of all steps.
  * Allow user to select the faulty step, enter corrective instructions, and write to `GuardrailMemory`.

---

### Phase 7: Integration, Qualification & Standalone Packaging
* **Milestone 7.1: IPC Loop Integration**:
  * Connect Rust Tauri supervisor to Brain engine over named pipes / stdio.
* **Milestone 7.2: End-to-End Benchmark Suite**:
  * Verify 5 foundational desktop tasks (e.g. Open browser $\rightarrow$ Search $\rightarrow$ Save file $\rightarrow$ Clean desktop).
  * Assert DXGI capture < 5ms, input dispatch < 1ms, and prompt cache hit < 5ms.
* **Milestone 7.3: Standalone Windows `.exe` Packaging**:
  * Configure `tauri.conf.json` with embedded resources and permissions.
  * Build production release binary using `cargo tauri build`.
  * Audit executable using REA (`rea inspect-rust-binary` and `rea inspect-pe-resources`).
