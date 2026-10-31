# Agent Aura

A Windows desktop agent. You type (or say) a task like "open Notepad and write a shopping list", and Aura does it on your screen. It takes a screenshot, asks a vision model what to do next, and then moves the mouse and types on the keyboard the way a person would.

- **Rust + Tauri v2.** The agent, screen capture, mouse and keyboard input, model calls, voice, schedules and the CLI are written in Rust. The window is a small TypeScript page.
- **Works with any vision model:** a local model in LM Studio (for example `qwen/qwen3-vl-4b`), any OpenAI-compatible server, or the OpenAI API.
- **Built to be safe to use:** Esc stops a task at once, risky actions ask you first, and passwords and keys are masked in history and logs.

> **Heads-up:** Aura controls your real mouse and keyboard. While it runs a task, keep your hands off them and press **Esc** to stop it.

---

## Requirements

| What | Why |
|---|---|
| Windows 10 or 11 (64-bit) | Aura only runs on Windows. |
| [Rust](https://rustup.rs) | To build the app. `rust-toolchain.toml` picks the MSVC toolchain automatically. |
| [Visual Studio 2022 Build Tools](https://visualstudio.microsoft.com/visual-cpp-build-tools/) with **Desktop development with C++** and a Windows SDK | Rust on Windows needs this linker. |
| [Node.js](https://nodejs.org) 22 or newer | To build the window's page and run the Tauri CLI. |
| WebView2 | Windows 11 and up-to-date Windows 10 already have it. The installer adds it if it's missing. |
| A vision model | For example [LM Studio](https://lmstudio.ai) running `qwen/qwen3-vl-4b`, or an OpenAI API key. |

## Getting started

### 1. Get the code and install the build tools

```powershell
git clone https://github.com/Aki-563/Agent-Aura.git
cd Agent-Aura
npm install                # Tauri CLI
npm --prefix ui install    # the window's page
```

### 2. Run it in development mode

```powershell
npm run tauri dev
```

The first build compiles all the Rust dependencies, so it takes a few minutes. Later builds are quick. When it's done, the Agent Aura window opens.

### 3. Connect a model

**Using LM Studio (local, free):**
1. In LM Studio, download a vision model (for example `qwen/qwen3-vl-4b`), load it, and start the server from the **Developer** tab. The default port is `1234`.
2. In Aura, click the **gear** (top right), then **Add model**.
3. For the address, enter `http://<lm-studio-pc-ip>:1234/v1` (or `http://localhost:1234/v1` if it's on the same PC). For the model ID, enter `qwen/qwen3-vl-4b`. Leave the key empty.
4. Click **Connect**. Aura sends the model a real image request, saves the model, and works out its coordinate system in the background.

**Using OpenAI:** choose the OpenAI API in **Add model**, enter a vision model ID and your API key, then click **Connect**. Keys are encrypted for your Windows account and never written to logs.

When the pill at the top right turns green, the model is ready.

### 4. Give it a task

Type a task in the box and press **Enter**. For example:

- `open notepad and type hello from aura`
- `open calculator and compute 12 * 7`
- `every day at 9am open my inbox` (this saves it as a schedule)

While the task runs, the window shrinks to a small player that you can pause, steer or stop. Press **Esc** to stop at any time. When the task ends, the window comes back just as it was.

**Optional: speak your tasks.** Add a [Sarvam AI](https://www.sarvam.ai) key in Settings → **Speak tasks**, then tap the mic. You can speak in English, Tamil, Tanglish or another Indian language.

---

## Build an installer

```powershell
npm run tauri build
```

This builds the installer at `src-tauri\target\release\bundle\nsis\Agent Aura_<version>_x64-setup.exe`. It installs per user, so it needs no admin rights. The plain app is at `src-tauri\target\release\agent-aura.exe`.

The build is unsigned unless you set `AURA_SIGN_THUMBPRINT` (see [`tools/sign.cmd`](tools/sign.cmd)). Unsigned builds can trigger SmartScreen warnings, and some antivirus programs quarantine them.

## The command-line tool (`aura-cli`)

The same build also produces `aura-cli.exe`. Run these from `src-tauri\`:

```powershell
cargo build --release
target\release\aura-cli.exe doctor        # check the saved model answers
target\release\aura-cli.exe look          # what the model would see: windows, numbered elements, screenshot
target\release\aura-cli.exe demo          # pointer glide, overlay and sounds (about 6 s)
target\release\aura-cli.exe calibrate --url http://<host>:1234/v1 --model <id>   # measure a model's coordinate system
target\release\aura-cli.exe eval --model <id> --suite basic --url http://<host>:1234/v1   # model pass rates
target\release\aura-cli.exe schedule list
```

## Tests

```powershell
cd src-tauri
cargo test --lib          # unit tests (about a minute; they never send input)
cd ..
.\test-all.ps1            # unit tests + the desktop suite
```

The desktop suite **drives your real screen**. It uses its own Chrome with a throwaway profile, and its own Notepad and Calculator. Don't use the PC while it runs. Esc stops it.

## Project layout

```
src-tauri/          Rust app (Tauri v2)
  src/agent.rs      the task loop and model prompts (src/agent/app.rs: the app's commands)
  src/bin/          aura-cli
  src/...           capture, input, perception, overlay, voice, schedules, storage
  tauri.conf.json   window and installer settings
ui/                 the window (TypeScript + Vite)
tools/              signing script, icon maker, DevTools driver for the app's page
docs/               design docs, plans and progress tracker
test-all.ps1        runs every test
```

## Where Aura keeps its data

Everything is stored in `%LOCALAPPDATA%\AgentAura\`: the database (`agent-aura.db`, holding models, history and skills), settings (`harness.json`, `profiles.json`, `schedules.json`) and test logs. API keys and step screenshots are encrypted with Windows DPAPI.

## Learn more

- **[docs/USER_GUIDE.md](docs/USER_GUIDE.md):** the full reference. It covers every app feature, schedules, voice, steering, grounding modes, parallel windows, the model JSON and diagnostics.
- **[docs/CORE.md](docs/CORE.md):** architecture, coordinates and input.
- **[docs/PROGRESS_TRACKER.md](docs/PROGRESS_TRACKER.md):** what's done and what's next.
- **[AGENTS.md](AGENTS.md):** rules for AI coding agents working on this repo.
