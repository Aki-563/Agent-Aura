# Agent Aura — Master Feature Specification (`FEATURES.md`)

**Document Version:** 1.1 (2026-10-30, implementation status added)  
**Project:** Agent Aura  
**Target Environment:** Windows 10/11 x64  
**Primary Purpose:** Comprehensive, exhaustive technical specification of all features designed for Agent Aura, split into **Extreme Speed & Low Latency** and **Human-Like Behavior & Self-Learning**. Every coding agent must use this document as the single source of truth for feature functionality, database schemas, and integration points.

---

## Implementation status (2026-10-30)

This file describes the intended features. What is actually built and working in the Rust app:

| Feature | Status |
| :--- | :--- |
| 1.1 DXGI capture | Built (a fresh duplication per step with a GDI fallback; about 0.4 s for a full observation including UI Automation and OCR, so the 3-5 ms figure is not the end-to-end number) |
| 1.2 `SendInput` dispatch | Built and hardened (see CORE §2.2); conformance-tested |
| 1.3 Unified single-pass VLM | Built (one model call per step; a second critique call only for risky or unlisted targets) |
| 1.4 Action batching | **Not built** (one action per step) |
| 1.5 Differential visual patching | **Not built** (whole monitor of the active window); a crop is only used as extra evidence for the verifier |
| 1.6 Shortcut knowledge base | Partly: shortcuts are in the prompt rules; the SQLite seed rows were removed on purpose |
| 1.7 Clipboard injection | Built (previous clipboard restored afterwards) |
| 1.8 3-tier prompt cache | Exact and fuzzy retrieval of approved skills and auto recipes; **no vector tier** |
| 1.9 Mid-task state snapping | **Not built** |
| 1.10 2 ms radar | **Not as described** (brute-force matching) |
| 1.11 WebP downscaling | JPEG (q88) at a 1280 px long edge instead of WebP |
| 2.1 Apprenticeship learning | Partly: plain-language auto recipes of successful runs |
| 2.2-2.4 Review modal, scrubber, guardrails | Built |
| 2.5 Confirmation gate | Built as an Allow/Deny card for consequential actions the task did not ask for |
| 2.6 Humility gate | Partly: `ask_user`, a stuck-streak warning after 2 and a stop after 4 no-effect steps |
| 2.7 Settle detection | Built as settle v2 (change then stable) |
| 2.8 Anchor self-healing | **Not built** |
| 2.9-2.10 HUD glow and halo cursor | Built (CPU bitmap, not Direct2D) |
| 2.11 Dynamic task policy | **Not built** |
| 2.12 Kill switch | Built (injected Esc ignored; keys and buttons released) |

### Added in Phase 8 (not in the original design)
Numbered elements on the screenshot (Set-of-Mark); a numbered `WINDOWS OPEN` list and `focus_window` by number; scroll-position hints; the guards that refuse an action on a stale screen (window moved, covered, no longer in front, or the target moved since the screenshot); a read-back after every action (windows opened or closed with counts, keyboard focus and its content, a typing warning); per-model pointing-error correction; the model-free conformance suite and `aura-cli look` / `do` for testing as the model. See `RELIABILITY_PLAN.md` §5.10 and §12.

---

# SECTION 1: Extreme Speed & Low Latency Features ⚡

```
┌────────────────────────────────────────────────────────────────────────────┐
│                  SPEED & LATENCY OPTIMIZATION MATRIX                       │
├──────────────────────────────────┬───────────────────┬─────────────────────┤
│ Feature                          │ Legacy Agent-S    │ Agent Aura Target   │
├──────────────────────────────────┼───────────────────┼─────────────────────┤
│ 1. Screen Capture Pipeline       │ 150ms – 300ms     │ 3ms – 5ms (DXGI)    │
│ 2. Input Actuation               │ 50ms – 100ms      │ < 1ms (SendInput)   │
│ 3. VLM Reasoning & Grounding     │ 4000ms – 6000ms   │ 900ms – 1400ms      │
│ 4. Multi-Step Navigation Flow    │ 3 turns (15s)     │ 1 turn (1.2s batch) │
│ 5. Payload Transfer Size         │ 10 MB (PNG)       │ 120 KB (WebP crop)  │
│ 6. App Navigation / Shortcuts    │ 3-5 visual clicks │ 50ms single hotkey  │
│ 7. Repeated Task Retrieval       │ 4000ms (Re-think) │ 0.01ms (Hash Cache) │
│ 8. Moved Icon Recovery           │ 4000ms (LLM call) │ 2ms (Local Radar)   │
└──────────────────────────────────┴───────────────────┴─────────────────────┘
```

---

### 1.1 Direct DXGI Desktop Duplication (Windows Native GPU Capture)
* **Problem in Legacy**: Python `pyautogui.screenshot()` uses legacy GDI `BitBlt`, which runs on the CPU, causes frame tearing, takes 150–300ms, and stutters under multi-monitor DPI virtualization.
* **Agent Aura Solution**:
  * Implemented in Rust via `windows-sys` accessing the Direct3D 11 Desktop Duplication API (`IDXGIOutputDuplication`).
  * Acquires the desktop frame directly from GPU VRAM (`AcquireNextFrame`).
  * Direct texture mapping into shared memory without CPU copy.
* **Latency**: **3ms to 5ms** at 60 FPS.
* **Multi-Monitor Handling**: Captures the specific `HMONITOR` where the active target window resides.

---

### 1.2 Native Rust `SendInput` Dispatch
* **Problem in Legacy**: Python `pyautogui` introduces serialization delays, GIL pauses, and lacks atomic multi-key synchronization.
* **Agent Aura Solution**:
  * Calls Windows `SendInput` in Rust with pre-allocated `INPUT` structures.
  * Injects mouse moves, clicks, and keystrokes directly into the kernel input message queue.
  * Absolute coordinate translation using `MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK`.
* **Latency**: **< 0.1ms** execution overhead.

---

### 1.3 Unified Single-Pass VLM
* **Problem in Legacy**: Agent-S calls Model 1 (Planner, e.g. GPT-4o) to decide *"click search bar"*, then calls Model 2 (Spotter, UI-TARS) to find $(X, Y)$. Two roundtrips = double latency and double token billing.
* **Agent Aura Solution**:
  * Connects to native unified vision-action models (Claude 3.7 Computer Use / Qwen2.5-VL / Gemini 2.0 Flash).
  * The model outputs the **decision AND the pixel coordinates in a single turn**:
    ```json
    {"thought": "Click search bar", "action": "click", "x": 640, "y": 85}
    ```
* **Latency**: Eliminates an entire 3-second LLM call per step.

---

### 1.4 Action Batching / Macro Chaining
* **Problem in Legacy**: Simple operations like visiting a website take 3 separate rounds: (1) Click address bar, (2) Type URL, (3) Press Enter. Each turn takes 4 seconds (Total: 12 seconds).
* **Agent Aura Solution**:
  * The system prompt instructs the Brain to output **deterministic multi-step chains**:
    ```json
    {
      "step_id": 1,
      "thought": "Open website via URL bar hotkey",
      "actions": [
        {"type": "hotkey", "keys": ["ctrl", "l"]},
        {"type": "type_text", "text": "https://news.ycombinator.com"},
        {"type": "key_press", "key": "enter"}
      ]
    }
    ```
  * Rust executes all 3 actions sequentially with 30ms inter-step spacing.
* **Speed Gain**: Reduces a 12-second operation to **1.2 seconds total**.

---

### 1.5 Differential Visual Patching (Crop-Only Transmission)
* **Problem in Legacy**: Sending full 4K screenshots burns ~3,000 vision tokens per step and takes 400ms just to upload to the API.
* **Agent Aura Solution**:
  * When the active task occurs inside a known target window (e.g. Notepad, Calculator, VS Code), Rust queries `GetWindowRect` and crops **only that active window bounding box**.
  * Encodes to WebP (Quality 85). File size drops from 10MB to ~80KB.
  * **Coordinate Reconstruction Math**:
    $$X_{\text{screen}} = \text{Window}_{\text{left}} + \text{round}\left(X_{\text{crop\_model}} \times \frac{\text{Window}_{\text{width}}}{\text{Canvas}_{\text{width}}}\right)$$
    $$Y_{\text{screen}} = \text{Window}_{\text{top}} + \text{round}\left(Y_{\text{crop\_model}} \times \frac{\text{Window}_{\text{height}}}{\text{Canvas}_{\text{height}}}\right)$$
* **Speed Gain**: 80% fewer vision tokens burned, 90% faster network payload upload.

---

### 1.6 Pre-Loaded & Learned Shortcut Knowledge Base
* **Problem in Legacy**: Agents visually search the screen for menus that already have instant keyboard shortcuts.
* **Agent Aura Solution**:
  * Pre-loaded SQLite database of universal hotkeys (`Win+D`, `Win+E`, `Ctrl+L`, `Ctrl+S`, `Ctrl+F`).
  * Whenever a task matches a known hotkey intent, it bypasses visual scanning and executes the hotkey directly.
* **Speed Gain**: Action executes in **50ms** at **zero token cost**.

---

### 1.7 Direct Clipboard Injection for Long Text
* **Problem in Legacy**: Typing 100 words via `pyautogui.typewrite()` takes 15–20 seconds.
* **Agent Aura Solution**:
  * For strings longer than 20 characters, the engine copies the string to the Windows Clipboard using the Win32 Clipboard API and fires `Ctrl + V`.
* **Speed Gain**: Pastes 50,000 words in **under 1 millisecond**.

---

### 1.8 3-Tier Semantic Fuzzy Prompt Cache
* **Problem in Legacy**: Rephrased user prompts ("tidy up my screen" vs "clean desktop") fail exact matching and trigger full LLM re-planning.
* **Agent Aura Solution**:
  * **Tier 1 (Exact Hash, <0.01ms)**: SHA-256 hash match against past prompts.
  * **Tier 2 (Fuzzy Levenshtein, <0.5ms)**: Token-sort similarity ratio > 0.90.
  * **Tier 3 (Local Vector Embedding, <4ms)**: Runs an embedded ONNX `all-MiniLM-L6-v2` model in Rust. Computes cosine similarity in vector space. If score > 0.88, matches the cached Golden Path.
* **Speed Gain**: Instant workflow recall without cloud latency.

---

### 1.9 Mid-Task State Snapping (Triple-Lock Verification)
* **Problem in Legacy**: If an agent starts a task without a cache match, it must stay in slow LLM reasoning mode for all 20 steps.
* **Agent Aura Solution**:
  * At each step, the agent checks if the current screen matches an intermediate milestone of a known skill.
  * **The Triple-Lock Gate**:
    1. Active process and window title match.
    2. Landmark visual anchors present on screen.
    3. High-level goal compatibility matches.
  * If all 3 pass, it **snaps out of slow reasoning** and replays the remaining steps via muscle memory.

---

### 1.10 2ms Local Radar Tracker (Moved Icon Resolver)
* **Problem in Legacy**: If an icon moves 50 pixels, the agent calls the cloud LLM to re-locate it.
* **Agent Aura Solution**:
  * Runs a local OpenCV Normalized Cross-Correlation (NCC) template match over the frame using edge-gradient contours.
  * Finds the icon's new coordinates $(X', Y')$ in **2ms on CPU**.
  * Confidence threshold $\ge 0.88$. If occluded or missing, safely falls back to Brain.

---

### 1.11 Token-Optimized WebP Downscaling
* **Agent Aura Solution**:
  * When full-screen capture is required, the image is scaled to a max long edge of **1280px** using Lanczos filtering and encoded to WebP.
  * Preserves sub-pixel font legibility while slashing token billing by 70%.

---

# SECTION 2: Human-Like Behavior & Self-Learning Features 🧠

```
┌────────────────────────────────────────────────────────────────────────────┐
│                    HUMAN-LIKE COGNITIVE ARCHITECTURE                       │
├────────────────────────────────────────────────────────────────────────────┤
│  [First Try: Apprentice] ──► Explores, observes, makes mistakes, recovers  │
│                                  │                                         │
│  [Task Done]             ──► 3-Way Review Modal pops up                    │
│                                  │                                         │
│  [Human Feedback]        ──► ✅ Perfect | ⚡ Optimize | ❌ Pinpoint Error  │
│                                  │                                         │
│  [Compilation]           ──► Prunes mistakes, saves Golden Path + Anchors  │
│                                  │                                         │
│  [Second Try: Master]    ──► Executes in Muscle Memory at 10x speed!       │
└────────────────────────────────────────────────────────────────────────────┘
```

---

### 2.1 The Apprenticeship Learning Engine (Trial $\rightarrow$ Mastery)
* **Core Concept**: The agent starts out like an unskilled apprentice:
  * On the **first run**, it takes cautious steps, explores the screen, and logs every action and observation.
  * If it misclicks, it notices the error, hits `Esc`, and recovers.
* **Post-Task Distillation**:
  * Once the task succeeds, a background compiler strips out all failed attempts, misclicks, and hesitations.
  * It distills the clean **Golden Path** (minimal action sequence) into the SQLite skill database.
* **Result**: On the second run, it behaves like a seasoned expert.

---

### 2.2 3-Way Human Review Modal
At the conclusion of any task, Agent Aura displays a floating, elegant review modal:

```
┌─────────────────────────────────────────────────────────────────────────┐
│                       ⚡ AGENT AURA TASK REVIEW                         │
├─────────────────────────────────────────────────────────────────────────┤
│ Task: "Export quarterly sales report to PDF"                            │
│                                                                         │
│   Did I complete this task correctly?                                   │
│                                                                         │
│   [ ✅ Perfect ]      [ ⚡ Yes, but optimize it ]      [ ❌ Mistake made ]│
└─────────────────────────────────────────────────────────────────────────┘
```

* **`✅ Perfect`**: Immediately seals the distilled trajectory as a Golden Path skill.
* **`⚡ Yes, but optimize it`**: Opens a short instruction field:
  * User writes: *"Don't click 3 menus to save; just press Ctrl+S next time."*
  * The compiler substitutes the 3 mouse steps with the hotkey and saves the optimization.
* **`❌ Mistake made`**: Opens the Step Scrubber.

---

### 2.3 Visual Step Scrubber & Mistake Pinpointer
If the user clicks `❌ Mistake made`, the modal expands into a visual scrubber:

```
┌─────────────────────────────────────────────────────────────────────────┐
│                        🔍 PINPOINT THE MISTAKE                          │
├─────────────────────────────────────────────────────────────────────────┤
│ What went wrong?                                                        │
│ [ You clicked 'Print' instead of 'Save As PDF'                        ] │
│                                                                         │
│ Select the step where I went off-track:                                 │
│ ┌──────────────┐   ┌──────────────┐   ┌──────────────┐                  │
│ │   STEP 1     │   │   STEP 2     │   │ ⚠️ STEP 3    │                  │
│ │ [Thumbnail]  │   │ [Thumbnail]  │   │ [Thumbnail]  │ ◄── [SELECTED]   │
│ │ Click "File" │   │ Click "Print"│   │ Click "Send" │                  │
│ └──────────────┘   └──────────────┘   └──────────────┘                  │
│                                                                         │
│ What should I do at this step next time?                                │
│ [ At Step 2, click 'Export' -> 'Create PDF' instead                   ] │
│                                                                         │
│                       [ 💾 SAVE CORRECTION ]                            │
└─────────────────────────────────────────────────────────────────────────┘
```

---

### 2.4 Anti-Pattern / Mistake Guardrail Memory
* The user's correction is saved into the `GuardrailMemory` table.
* On any future run of that task, the Brain's prompt is injected with an explicit guardrail warning:
  > **⚠️ PAST MISTAKE TO AVOID**: In the previous attempt at Step 2, you clicked 'Print'. DO NOT CLICK 'Print'. Click 'Export' $\rightarrow$ 'Create PDF' instead.
* **Guarantees the agent never repeats a known mistake.**

---

### 2.5 Destructive Action Confirmation Gate (Human Caution)
* If the agent plans an action with permanent consequences (e.g., clicking *"Delete"*, *"Empty Recycle Bin"*, *"Drop Table"*, *"Format"*, or sending an email):
* The screen edge pulses **Amber**, the mouse freezes, and a confirmation prompt asks:
  > **⚠️ Human Confirmation Required**: About to execute: *"Permanently delete 14 files"*. Allow this action? `[ Yes, Proceed ]` `[ No, Cancel ]`

---

### 2.6 Humility & Asking for Direction (Anti-Loop Gate)
* If the agent retries an action **2 times** and fails to locate the target or settle:
* Instead of burning API tokens in an infinite loop, the screen edge pulses gently and a floating toast asks:
  > *"I'm having trouble finding the 'Submit' button on this updated page. Could you click it once, and I will take over from there?"*
* Once the user clicks, the agent logs the new visual anchor at that position and resumes automatically.

---

### 2.7 Visual State Settle Detection (Hardware Cursor + Quiescence)
* Eliminates the Render Race Condition by inspecting:
  1. Win32 `GetCursorInfo` for `IDC_WAIT` or `IDC_APPSTARTING`.
  2. Frame difference $\Delta < 0.2\%$ between 50ms frames.
* Ensures the agent never acts on half-loaded screens.

---

### 2.8 Visual Anchor Verification & Self-Healing
* For every cached step, saves a $64 \times 64$ crop of the target button.
* If an app layout updates and the anchor is missing, it **halts muscle memory safely** and wakes up the Brain to find the new button and auto-update the database.

---

### 2.9 Screen-Edge Glow (Aura HUD)
* A borderless, transparent, click-through overlay window created via `WS_EX_TOPMOST | WS_EX_LAYERED | WS_EX_TRANSPARENT`.
* Renders an animated neon glow border around the 4 edges of the physical screen:
  * **Electric Purple (Breathing sine wave)**: Brain is thinking / reasoning.
  * **Neon Cyan (Crisp glow)**: Agent is actively moving mouse / typing.
  * **Warm Amber (Soft shimmer)**: Waiting for page to settle or caution gate.
  * **Safety Red (Double flash)**: Emergency stop triggered.

---

### 2.10 The Glowing Halo Cursor
* Replaces jarring cursor jumps with smooth cubic-bezier mouse glide.
* A subtle glowing ring surrounds the mouse tip:
  * Expands outward like a water ripple upon clicking.
  * Morphs into a vertical glowing cursor bracket when typing.

---

### 2.11 Dynamic Task Policy Strategies
* Distinguishes between static UI tasks (clicking fixed ribbons) and dynamic content tasks (e.g. *"Clean Desktop"*).
* Dynamic tasks store **universal keyboard strategies** (`Win+D` $\rightarrow$ `Ctrl+A` $\rightarrow$ deselect shortcuts $\rightarrow$ Move) rather than fragile pixel coordinates.

---

### 2.12 Hardware Emergency Kill-Switch
* Low-level Windows hook via `SetWindowsHookExW(WH_KEYBOARD_LL)`.
* Pressing `Esc` or `Ctrl + Pause` **instantly seizes control**, kills pending input queues, and resets the mouse to the human user in under 1 millisecond.
