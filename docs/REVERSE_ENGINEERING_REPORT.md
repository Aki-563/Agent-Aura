# Reverse Engineering Report: Agent-S Architecture & Modernization Blueprint

**Document Version:** 1.1 (2026-10-30, historical note added)  
**Target Analyzed:** `reference/Agent-S` (`gui-agents v0.3.2`, Simular AI)  
**Target Project:** `Agent Aura`  
**Host Environment:** Windows 11 (x64)  
**Analysis Engine:** Reverse Engineer Anything (REA) + Static AST Deconstruction  

> **Historical document (2026-10-30).** This is the 2026-10 teardown of Agent-S that started the project; its findings are unchanged. The modernization blueprint in section 5 was the starting plan, not a status report: for what is built, see `PROGRESS_TRACKER.md` and `RELIABILITY_PLAN.md`. Differences worth knowing: there is no Python worker or IPC (section 6), action batching and the vector cache were not built, and the capture pipeline is DXGI with a GDI fallback at roughly 0.4 s for a full observation once UI Automation and OCR are included.

---

## 1. Executive Summary

This report documents the architectural reverse-engineering of the **Agent-S** computer-use repository. The goal of this analysis is to deconstruct its perception, reasoning, grounding, and execution pipelines, identify critical performance bottlenecks and architectural vulnerabilities, and establish the foundational design for **Agent Aura**—a modernized, ultra-fast, human-like desktop agent built on native Windows APIs and Rust.

---

## 2. Codebase Decomposition: Core vs. Bloat

Our inspection identified that **over 85% of the repository consists of academic evaluation overhead** that is unnecessary for a production desktop agent.

```
reference/Agent-S/
├── gui_agents/
│   ├── s1/, s2/, s2_5/         [❌ DEPRECATED] Legacy iterations with slower hierarchical loops.
│   └── s3/                     [🌟 CORE ENGINE] Production OSWorld baseline.
│       ├── agents/
│       │   ├── agent_s.py      [🌟 CORE] High-level orchestrator.
│       │   ├── worker.py       [🌟 CORE] Cognitive step generator & visual trajectory manager.
│       │   ├── grounding.py    [🌟 CORE] OSWorldACI - UI-TARS coordinate resolver & action definitions.
│       │   └── code_agent.py   [OPTIONAL] Local python/bash script executor for spreadsheets.
│       ├── bbon/               [❌ BLOAT] Behavior Best-of-N parallel VM rollout judge.
│       ├── core/mllm.py        [🌟 CORE] Multimodal LLM provider connector (OpenAI/Anthropic/Gemini).
│       └── utils/
│           ├── common_utils.py [🌟 CORE] Screenshot downscaling, DPI math, and retry logic.
│           └── local_env.py    [🌟 CORE] Local OS execution wrapper.
├── agent_s4/                   [❌ BLOAT] Genetic algorithm research harness (crossover/mutation).
├── osworld_setup/              [❌ BLOAT] Ubuntu/Docker VM evaluation environment.
├── scienceboard_setup/         [❌ BLOAT] Academic benchmark dataset and harness.
├── evaluation_sets/            [❌ BLOAT] Static benchmark test suites.
└── setup.py                    [REFERENCE] Entry point: `agent_s = gui_agents.s3.cli_app:main`.
```

---

## 3. The 4 Foundational Pillars of Agent-S

```
                  ┌─────────────────────────────────────┐
                  │        1. PERCEPTION (Eyes)         │
                  │   pyautogui.screenshot() -> PNG     │
                  └──────────────────┬──────────────────┘
                                     │
                                     ▼
                  ┌─────────────────────────────────────┐
                  │        2. REASONING (Brain)         │
                  │  Worker LLM outputs semantic intent │
                  │     e.g., click("Chrome icon")      │
                  └──────────────────┬──────────────────┘
                                     │
                                     ▼
                  ┌─────────────────────────────────────┐
                  │        3. GROUNDING (Spotter)       │
                  │   UI-TARS model outputs coordinates │
                  │            (X=420, Y=1055)          │
                  └──────────────────┬──────────────────┘
                                     │
                                     ▼
                  ┌─────────────────────────────────────┐
                  │         4. ACTION (Muscles)         │
                  │   pyautogui.click() via exec()      │
                  └─────────────────────────────────────┘
```

### Pillar 1: Perception (Screen Capture)
* **Mechanism**: Uses `pyautogui.screenshot()` (GDI-based).
* **Processing**: Resizes using `PIL.Image.LANCZOS` to fixed dimensions ($1920 \times 1080$), saves to a memory `BytesIO` buffer, and encodes to base64.
* **OCR Fallback**: Uses `pytesseract` and `paddleocr` to build word-level bounding boxes for text spans.

### Pillar 2: Visual Grounding (Coordinate Resolution)
* **Implementation**: `gui_agents/s3/agents/grounding.py` (`OSWorldACI`).
* **Prompt Schema**:
  ```text
  Query: <element_description>
  Output only the coordinate of one point in your response.
  ```
* **Coordinate Parsing**: Extracts integers via regex (`re.findall(r"\d+", response)`).
* **Scaling Equation**:
  $$X_{\text{screen}} = \text{round}\left(X_{\text{model}} \times \frac{\text{Width}_{\text{screen}}}{\text{Width}_{\text{grounding}}}\right)$$
  $$Y_{\text{screen}} = \text{round}\left(Y_{\text{model}} \times \frac{\text{Height}_{\text{screen}}}{\text{Height}_{\text{grounding}}}\right)$$

### Pillar 3: Planning & Memory
* **Implementation**: `gui_agents/s3/agents/worker.py`.
* **State Management**: Maintains a rolling buffer of up to 8 screenshot turns.
* **Reflection Engine**: Calls an auxiliary model to compare before/after images and reflect on previous action outcomes.

### Pillar 4: Action Execution Space
* **Primitives**: `click`, `type`, `drag_and_drop`, `scroll`, `hotkey`, `hold_and_press`, `switch_applications`, `open`, `wait`, `done`, `fail`.
* **Dispatch Mechanism**: Assembles raw Python strings and runs `exec(code[0])`.

---

## 4. Critical Bottlenecks & Flaws in Agent-S

1. **Catastrophic Security Risk (`exec`)**:
   Agent-S directly executes raw Python strings emitted by the model. A hallucinated or prompt-injected payload can execute arbitrary system commands (`os.remove`, `rmdir`, PowerShell scripts).
2. **High Latency Two-Stage Bottleneck (4–6s per click)**:
   Every single click requires **two separate LLM API roundtrips**: Step 1 for the Planner LLM, and Step 2 for the UI-TARS Grounding model.
3. **Sluggish Capture Pipeline (~300ms)**:
   GDI-based `pyautogui.screenshot()` blocks the main thread for 150–300ms per frame.
4. **The Render Race Condition & Silent Misses**:
   Agent-S fires clicks and immediately screenshots before Windows UI animations render or window focus shifts, causing the agent to miss actions and blindly march forward.
5. **No Graphical HUD or Safety Hooks**:
   The user has no visual feedback of what the agent is targeting until the mouse suddenly jumps, and no reliable low-level hardware kill-switch.

---

## 5. Agent Aura Modernization Blueprint

| Component | Agent-S (Legacy) | Agent Aura (Modernized) | Benefit |
| :--- | :--- | :--- | :--- |
| **Capture Pipeline** | `pyautogui.screenshot()` (300ms) | **DXGI Desktop Duplication in Rust (5ms)** | 60x faster screen capture; zero CPU load. |
| **Action Safety** | Arbitrary `exec(code_str)` | **Strict Typed JSON Action Protocol** | Zero arbitrary execution; 100% verified. |
| **Input Dispatch** | Python `pyautogui` | **Native Rust Win32 `SendInput`** | 0ms kernel-level input latency. |
| **Vision Model** | 2-Stage (Planner + UI-TARS) | **Unified Single-Pass VLM (Claude/Qwen/Gemini)** | 50% lower network latency & API costs. |
| **Action Speed** | 1 action per LLM call | **Action Batching / Macro Chaining** | 3x to 5x faster navigation sequences. |
| **Bandwidth/Tokens**| Raw 4K upload (10MB) | **Differential Visual Patching + 1280px WebP** | 80% lower token consumption. |
| **Shortcuts Engine**| Mouse click everything | **Pre-Loaded & Learned Hotkey Database** | 50ms instant actions at 0 token cost. |
| **Prompt Cache** | None (always calls LLM) | **3-Tier Semantic Fuzzy Cache (ONNX Vector)** | Instant 0ms–4ms recall for known tasks. |
| **Self-Learning** | None (static prompt) | **Apprenticeship Engine & Golden Path Store** | Flawless execution on second attempt. |
| **Human Review** | None (terminal exit) | **3-Way Review Modal & Step Scrubber** | User can optimize or pinpoint errors. |
| **Safety Guards** | Unchecked clicks | **Destructive Action Confirmation Gate** | Prevents accidental file/database deletion. |
| **Failure Recovery**| Loops until step limit | **Humility Gate (Asks user for 1 click)** | Eliminates infinite loops; logs anchor. |
| **Visual Feedback** | Terminal logs only | **Screen-Edge Glow (Aura HUD) & Halo Cursor** | Clear state visibility and human-like feel. |
| **Emergency Stop** | `Ctrl+C` (unreliable) | **Global Low-Level Win32 Hook (`Esc`)** | Instant mouse and keyboard freeze. |

---

## 6. Implementation Architecture

```
Agent Aura/
├── src-tauri/ (or native Rust core)
│   ├── src/
│   │   ├── capture/        # DXGI Desktop Duplication (<5ms screen capture)
│   │   ├── input/          # Win32 SendInput dispatch & keyboard hooks
│   │   ├── overlay/        # WS_EX_TRANSPARENT screen glow & halo cursor
│   │   ├── radar/          # Local OpenCV NCC template tracker (2ms icon match)
│   │   ├── memory/         # Embedded SQLite + sqlite-vec (skill & shortcut DB)
│   │   └── main.rs         # IPC Supervisor & state machine
├── engine/ (Python or Rust)
│   ├── brain.py            # Unified VLM gateway (Claude 3.7 / Qwen2.5-VL / Gemini)
│   ├── batcher.py          # Action batching & macro chain executor
│   ├── settle.py           # Cursor IDC_WAIT & frame quiescence detector
│   └── review.py           # 3-way review & step scrubber state manager
└── ui/
    └── index.html          # Lightweight desktop HUD & review modal
```

---

## 7. Sign-Off & Status

* **Reverse-Engineering Status:** **COMPLETE**
* **Target Usability:** All key components, algorithms, and flaws extracted.
* **Next Action:** Initialize the clean-room **Agent Aura** codebase and implement Phase 1 (Capture + Input Engine).
