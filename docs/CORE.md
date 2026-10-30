# Agent Aura — Core Engine Architecture (`CORE.md`)

**Document Version:** 1.1 (2026-10-30, status banner and tables brought up to date)  
**Project:** Agent Aura  
**Target Environment:** Windows 10/11 x64  
**Primary Languages:** Rust (everything that ships: capture, input, perception, the model loop, the overlay and the Tauri shell); the TypeScript UI in `ui/`. The Python in `engine/` is unwired and deprecated.  
**Primary Purpose:** Definitive technical specification of the foundational runtime, state machines, input actuation, coordinate transformations, and communication protocols for any coding agent implementing this system.

> **Status (2026-10-30).** This document is the original design. What actually ships is a **single Rust/Tauri binary**: there is no Python engine, no IPC between a supervisor and a worker (§6 is not implemented), the loop proposes **one action per step** (no batching), and the settle detector is the v2 change-then-stable design in `RELIABILITY_PLAN.md` §5.5. The as-built behaviour is described in `RELIABILITY_PLAN.md` §5.10; where the two differ, the plan wins.

---

## 1. Core Architectural Philosophy

Agent Aura is a **hybrid desktop computer-use agent** engineered to overcome the high latency, instability, and insecurity of legacy automation agents (e.g., Agent-S). 

It abandons Python `exec()` script injection and slow GDI screen grabbing in favor of a **Hardware-Coupled Dual Engine**:
1. **The Native Rust Supervisor (The Body)**: Handles ultra-low-latency screen capture via DXGI, direct Win32 `SendInput` event injection, the transparent click-through HUD overlay, hardware cursor state probing, and the low-level emergency kill-switch.
2. **The Cognitive Agent Engine (The Brain)**: Connects to frontier unified Vision-Language Models (Claude 3.7 / Qwen2.5-VL / Gemini 2.0 Flash), parses visual perception into structured JSON action batches, checks local semantic cache and muscle memory, and evaluates visual state settle transitions.

```
                           ┌────────────────────────────────────────────────────────┐
                           │               USER PROMPT / TASK GOAL                  │
                           └───────────────────────────┬────────────────────────────┘
                                                       │
                                                       ▼
                                   ┌──────────────────────────────────────┐
                                   │      3-Tier Semantic Cache & DB      │
                                   │  Exact Hash -> Levenshtein -> Vector │
                                   └───────┬──────────────────────┬───────┘
                              HIT (Cached) │                      │ MISS (New Task)
                                           ▼                      ▼
                   ┌──────────────────────────────┐    ┌──────────────────────────────────┐
                   │    Muscle Memory Replay      │    │  Unified VLM Reasoning Engine    │
                   │ (Visual Anchor Verification) │    │  (Action Batching + Shortcuts)   │
                   └──────────────┬───────────────┘    └──────────────────┬───────────────┘
                                  │                                       │
                                  └───────────────────┬───────────────────┘
                                                      │ Validated Action Batch
                                                      ▼
                                   ┌──────────────────────────────────────┐
                                   │      Native Rust Actuator Core       │
                                   │  • Screen-Edge Glow & Halo Cursor    │
                                   │  • Win32 SendInput (0ms delay)       │
                                   │  • Global Hook (Emergency Stop)      │
                                   └──────────────────┬───────────────────┘
                                                      │
                                                      ▼
                                   ┌──────────────────────────────────────┐
                                   │   State Settle & Stability Check     │
                                   │  • IDC_WAIT hardware cursor check    │
                                   │  • Optical pixel quiescence (<0.2%)  │
                                   └──────────────────┬───────────────────┘
                                                      │
                                                      ▼
                                   ┌──────────────────────────────────────┐
                                   │    Post-Step Visual Verification     │
                                   │   Did UI state change as expected?   │
                                   └──────────────────────────────────────┘
```

---

## 2. Structured Action Protocol (Replacing `exec()`)

Agent Aura strictly forbids raw Python or shell code execution emitted by LLMs. All physical interactions are modeled as **strongly typed JSON commands**, verified before dispatch.

### 2.1 Action Grammar Schema

*(As built, each model reply carries exactly one `action` object, not an `actions` array; a few fields below are therefore only illustrative.)*

```json
{
  "step_id": 1,
  "thought": "Focus the Chrome address bar to enter the destination URL.",
  "actions": [
    {
      "type": "hotkey",
      "keys": ["ctrl", "l"]
    },
    {
      "type": "type_text",
      "text": "https://github.com",
      "press_enter": true
    }
  ]
}
```

### 2.2 Allowed Action Primitives (as built, `model_schema.rs`)

Pointer actions take either a numbered `element` (preferred: it is bounded to the elements actually numbered on the screenshot) or raw `x`/`y` in screenshot pixels.

| Action Type | Required Fields | Optional Fields | Description |
| :--- | :--- | :--- | :--- |
| `click` | `element` or `x`,`y` | `button`: "left"\|"right"\|"middle", `clicks`: int, `hold_keys`: string[] | Moves to the point (confirmed) and clicks. Right and middle clicks, and Shift/Ctrl clicks, are variations of this. |
| `double_click` | `element` or `x`,`y` | `button` | Two clicks with a short gap. |
| `mouse_move` | `element` or `x`,`y` | — | Hover: moves the pointer without clicking (menus, tooltips). |
| `drag_and_drop`| `from_element`/`to_element` or `from_x`,`from_y`,`to_x`,`to_y` | `duration_ms`, `hold_keys` | Press, nudge, glide, rest on the target, release. |
| `type_text` | `text` | `press_enter`, `overwrite` | Types text: real key presses for ASCII, Unicode packets for the rest, clipboard paste (restored afterwards) for long text. |
| `hotkey` | `keys`: string[] | — | A key combination, held about 70 ms. A single string such as `"Ctrl+Shift+T"` is split into keys. |
| `key_press` | `key`: string | — | One key (names and aliases such as `esc`, `return`, `pgdn`, punctuation names). |
| `scroll` | `clicks`: int | `element` or `x`,`y`, `horizontal` | Wheel up (positive) or down (negative), sent in 3-notch chunks; with no point it is aimed at the target window. |
| `open_app` | `name` | — | Starts an application and waits until its window is in front and responsive. |
| `focus_window` | `name` | — | A title, a program name, or a number from the prompt's `WINDOWS OPEN` list; minimized windows are restored. |
| `open_url` | `url` | — | http(s) only, no spaces; reuses the current tab when a browser is in front. |
| `note` | `text` | — | Remembers a short fact; shown in every later prompt. |
| `ask_user` | `question` | — | Pauses and asks the user (Allow/Deny card with "Done, continue" / "I can't"). |
| `wait_settle` | — | `max_timeout_ms` / `seconds` | Waits for loading or animation to finish. |
| `done` | `summary` | — | Claims completion; a strict screen check can reject the claim. |
| `fail` | `reason` | — | Declares the task impossible. |

---

## 3. Coordinate Spaces & Geometric Transformations

Screen coordinates exist in multiple conflicting domains on Windows. Agent Aura eliminates coordinate drift through explicit geometric mapping.

### 3.1 The Three Coordinate Spaces
1. **Physical Screen Space**: The raw hardware display pixels (e.g., $3840 \times 2160$ on 4K, $1920 \times 1080$ on 1080p).
2. **Normalized VLM Canvas**: The aspect-preserved downsampled image sent to the vision model (long edge clamped to 1280px).
3. **Window-Relative Crop Space**: When Differential Visual Patching is active, coordinates are relative to the active window's top-left origin. *(Not used today: the screenshot is the whole monitor of the active window, scaled to at most 1280 px.)*
4. **Numbered Elements (Set-of-Mark)**: the preferred way to point. Up to 50 elements found by UI Automation and OCR are drawn with numbers on the screenshot and listed in the prompt; the model answers with a number and the harness converts it to an exact point. Raw x/y points are corrected by `coords.rs` (near misses snap onto an element; a model's consistent offset or scale error is learned and removed).

### 3.2 Transformation Formulas

#### A. Full-Screen VLM Coordinate Denormalization:
Given a model output point $(X_{\text{model}}, Y_{\text{model}})$ on an image of size $(W_{\text{canvas}}, H_{\text{canvas}})$:
$$X_{\text{screen}} = \text{round}\left(X_{\text{model}} \times \frac{W_{\text{physical}}}{W_{\text{canvas}}}\right)$$
$$Y_{\text{screen}} = \text{round}\left(Y_{\text{model}} \times \frac{H_{\text{physical}}}{H_{\text{canvas}}}\right)$$

#### B. Differential Window Crop Denormalization:
Given an active window located at physical screen rect $(L_{\text{win}}, T_{\text{win}}, W_{\text{win}}, H_{\text{win}})$:
$$X_{\text{screen}} = L_{\text{win}} + \text{round}\left(X_{\text{model}} \times \frac{W_{\text{win}}}{W_{\text{canvas}}}\right)$$
$$Y_{\text{screen}} = T_{\text{win}} + \text{round}\left(Y_{\text{model}} \times \frac{H_{\text{win}}}{H_{\text{canvas}}}\right)$$

#### C. Bounds Validation Rule:
Before any coordinate is passed to Win32 `SendInput`, it MUST satisfy:
$$0 \le X_{\text{screen}} < W_{\text{physical}} \quad \text{and} \quad 0 \le Y_{\text{screen}} < H_{\text{physical}}$$
Any coordinate outside these bounds triggers an immediate validation error without dispatching input.

---

## 4. Hardware Input Actuation Engine (Rust)

Input actuation is handled exclusively in native Rust via `windows-sys` or `winapi` bindings to `SendInput`.

### 4.1 Mouse Input Actuation
* Mouse events use `MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK`.
* Normalization to Win32 absolute mouse space ($0$ to $65535$):
  $$\text{Normalized } X = \text{round}\left(\frac{X_{\text{screen}} \times 65535}{W_{\text{physical}} - 1}\right)$$
  $$\text{Normalized } Y = \text{round}\left(\frac{Y_{\text{screen}} \times 65535}{H_{\text{physical}} - 1}\right)$$

### 4.2 Keyboard & Unicode Actuation
1. **ASCII & Virtual Keys**: Mapped via `VkKeyScanW` and dispatched using `KEYEVENTF_KEYDOWN` / `KEYEVENTF_KEYUP`.
2. **Unicode / Emojis**: Injected using `KEYEVENTF_UNICODE` without requiring clipboard or OS keyboard layout changes.
3. **Bulk Text (Clipboard Bridge)**: If string length exceeds 25 characters:
   * Sets text into the Windows Clipboard via `OpenClipboard`, `EmptyClipboard`, and `SetClipboardData(CF_UNICODETEXT)`.
   * Fires `Ctrl + V` via virtual keystrokes.
   * Execution time: **under 1 millisecond** for up to 50,000 characters.

---

## 5. Screen Settle & Quiescence Engine

> *As built:* `settle/effect.rs` waits for the screen to **change** and then to be **stable** (per-action timings in `RELIABILITY_PLAN.md` §5.5), judges the active window only, and reports the outcome (changed / settled / no visible effect / unstable) to the model as text. The cursor-busy check and frame difference below remain the building blocks.

To solve the **Render Race Condition** where actions are clicked but screenshots are taken before UI renders:

```
                  [ Action Fired by Actuator ]
                               │
                               ▼
               ┌───────────────────────────────┐
               │ 1. Mandatory Input Ingestion  │
               │    Wait 60ms (OS Event Queue) │
               └───────────────┬───────────────┘
                               │
                               ▼
               ┌───────────────────────────────┐
               │ 2. Hardware Cursor State Poll │
               │    Query `GetCursorInfo`      │
               └───────────────┬───────────────┘
             IDC_WAIT / IDC_APPSTARTING?
                               │
            YES ───────────────┼─────────────── NO
             │                                   │
             ▼                                   ▼
    [ App is Busy ]                     ┌───────────────────────────────┐
    Sleep 50ms, poll again              │ 3. Optical Frame Quiescence   │
    (Max timeout: 2500ms)               │    DXGI frame comparison      │
                                        └───────────────┬───────────────┘
                                                        │
                                        Diff between Frame(t) & Frame(t-50ms)
                                                        │
                                        ┌───────────────┴───────────────┐
                                        ▼                               ▼
                                   Diff > 0.2%                     Diff <= 0.2%
                                 (Still animating)              (Visual Stability)
                                        │                               │
                                  Sleep 50ms                            ▼
                               (Up to 1500ms)                 [ Screen is Settled! ]
                                                               Proceed to Capture
```

### 5.1 Hardware Cursor Inspection
Using Win32 `GetCursorInfo`:
* Extract cursor handle `hCursor`.
* Compare against system standard cursors:
  * `IDC_WAIT` (Hourglass / Full Spinner)
  * `IDC_APPSTARTING` (Pointer + Background Spinner)
* If active, thread yields until cursor returns to `IDC_ARROW` or `IDC_IBEAM`.

### 5.2 Optical Quiescence (Perceptual Frame Difference)
* Two frames captured 50ms apart are downsampled to a tiny $64 \times 36$ luminance matrix.
* Mean absolute pixel difference is calculated:
  $$\Delta = \frac{1}{N} \sum_{i=1}^{N} |P_t(i) - P_{t-50}(i)|$$
* If $\Delta < 0.002$ (less than 0.2% variation), the screen has settled.

---

## 6. The Supervisor-Worker IPC Boundary

> **Not implemented.** The brain runs inside the same Rust process (`agent.rs::execute_run`), and the same loop runs from the app or from `aura-cli`. This section is kept as the original design.

The architecture separates the low-level OS supervisor from the high-level cognitive agent using a typed bi-directional IPC channel (standard JSON-lines over stdin/stdout, or high-speed local loopback named pipes).

### 6.1 Supervisor Messages (Rust -> Engine)
* `{"type": "FRAME_READY", "frame_path": "...", "width": 1920, "height": 1080, "settled": true}`
* `{"type": "EMERGENCY_STOP_TRIGGERED", "reason": "User pressed ESC"}`
* `{"type": "WINDOW_CHANGED", "new_process": "chrome.exe", "title": "GitHub"}`

### 6.2 Engine Messages (Engine -> Rust)
* `{"type": "EXECUTE_BATCH", "actions": [...]}`
* `{"type": "SET_HUD_STATE", "state": "thinking" | "acting" | "settling" | "paused"}`
* `{"type": "REQUEST_RADAR_SCAN", "anchor_id": "btn_export_sales"}`
