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

API keys are kept only in Rust process memory. They are not written to SQLite, browser storage, logs, or repository files. Press **Esc** while actions are executing to trigger the emergency stop.
