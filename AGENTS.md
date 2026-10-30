# AGENTS.md — Agent Aura Mandatory Agent Instructions

> **ATTENTION ALL AI CODING AGENTS (Claude Code, Cursor, Antigravity, Copilot, Windsurf, Roo, etc.):**  
> Before generating, modifying, or deleting ANY code in this repository, you MUST follow the instructions below.

---

## 🚨 MANDATORY FIRST STEP: Read the Specification Suite

Do NOT make assumptions about architecture, dependencies, or features. The entire system design, coordinate formulas, actuation rules, and implementation roadmap are already finalized and documented inside the [`docs/`](./docs/) directory.

You **MUST** read these documents in the following exact sequence before writing any code:

1. **[`docs/PROGRESS_TRACKER.md`](./docs/PROGRESS_TRACKER.md)**:
   * **Identify the current active Phase and Milestone.**
   * Do NOT skip ahead to future phases. Work strictly on the pending checklist items.
   * Update the checkboxes (`[ ]` $\rightarrow$ `[x]`) upon completing and testing milestones.

1b. **[`docs/RELIABILITY_PLAN.md`](./docs/RELIABILITY_PLAN.md)** (active Phase 8):
   * The audited ground truth (§2), the bug ledger (§3), the target design (§5) and the milestone checklists (§6).
   * Where it conflicts with CORE.md or FEATURES.md, the plan wins. Those docs describe the original design, and several of their features are not implemented in Rust.

2. **[`docs/CORE.md`](./docs/CORE.md)**:
   * Understand the core runtime, typed JSON action grammar, coordinate spaces, Win32 `SendInput` rules, and hardware cursor / optical frame settle checks.
   * **Rule:** NEVER use Python `exec()` or blind macros. Follow the strict JSON action protocol.

3. **[`docs/FEATURES.md`](./docs/FEATURES.md)**:
   * Review the exact functional requirements for the 11 Speed Features and 12 Human-Like Features.
   * Reference the database schemas, thresholds, and edge-case behaviors.

4. **[`docs/IMPLEMENTATION_PLAN.md`](./docs/IMPLEMENTATION_PLAN.md)**:
   * Follow the step-by-step phased instructions for the active milestone.
   * Adhere to the directory structure and technology decisions (Tauri v2 + Rust).

5. **[`docs/REVERSE_ENGINEERING_REPORT.md`](./docs/REVERSE_ENGINEERING_REPORT.md)**:
   * Reference context for what was reverse-engineered from Agent-S, what flaws to avoid, and why certain legacy modules were discarded.

---

## 🛠️ Technology Stack Rules

* **Core & Actuation:** 100% Rust (`windows-sys`, Direct3D 11 / DXGI, `SendInput`).
* **Packaging & Desktop Shell:** Tauri v2 (compiling to a standalone lightweight `.exe`).
* **Database & Vector Search:** Embedded SQLite (`rusqlite`) + local ONNX embeddings (`fastembed`).
* **VLM Gateway:** Async HTTP streaming (Tokio + `reqwest`).
* **Strict Avoidance:** No Python runtime in production, no slow GDI screen grabbing, no PyInstaller bundling.

---

## 🧪 Testing Rules (learned the hard way)

* **Test the harness by acting as the model.** Use `aura-cli look` to see the screen exactly as the model would, then give one order at a time with `aura-cli do <order>`. If the harness does the wrong thing, fix the harness and add a case to `conformance.rs`. A mock or a weak local model is not a substitute for this.
* **The model is not the problem under test.** The harness must run whatever the model orders; model mistakes are fed back as feedback, not special-cased. Never special-case a model name.
* **Never send destructive keys outside a verified window.** Ctrl+A, Delete, Ctrl+W, Alt+F4 and window closes are only allowed after checking that the front window is one the test itself opened (Notepad, Calculator, the "Aura Fixture" pages). A test cleanup once recycled the user's profile folders and a later one probably closed their Chrome. Use `close_tab_if_front_is` in `conformance.rs`.
* **Never touch windows the user owns.** When driving a browser as the model, open a separate window and close only that one.
* Say what a real-desktop test will touch before running it. Full details: `RELIABILITY_PLAN.md` §5.10 and §12.

---

## 📋 Agent Handover & Updating Protocol

Whenever you complete a coding session or milestone:
1. Run local tests or build checks (e.g. `cargo check`, `cargo test`).
2. Update the corresponding `[x]` checkbox in [`docs/PROGRESS_TRACKER.md`](./docs/PROGRESS_TRACKER.md).
3. Update the overall completion percentage in `PROGRESS_TRACKER.md`.
4. Leave a clear summary of what was completed and the exact next milestone to be executed.
