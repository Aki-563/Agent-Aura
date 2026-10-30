//! Headless runner for Agent Aura: runs one task through the real agent loop from a terminal.
//!
//! ```text
//! aura-cli run --url http://192.168.6.70:1234/v1 --model google/gemma-4-e2b --task "open notepad"
//! aura-cli run --openai --model gpt-6.1-terra --key sk-... --task "..." --max-steps 20
//! aura-cli eval --suite basic --url http://192.168.6.70:1234/v1 --model google/gemma-4-e2b
//! ```
//!
//! `eval` runs a suite of small reversible tasks and checks each result independently of the
//! model's own claim (see `agent_aura_lib::eval`), then prints a table and saves a JSON report.
//!
//! It drives the real desktop (mouse, keyboard, screenshots) exactly like the app does, and
//! writes the same per-run trace folder. Press Esc to stop. Risky-action approval requests are
//! denied unless `--auto-approve` is given, because nobody is looking at a panel.

use agent_aura_lib::agent::{execute_run, AgentRuntime, ApiProtocol, Host, HostBackend, ProviderConfigInput};
use agent_aura_lib::input::killswitch::KillSwitch;
use serde_json::Value;

struct CliHost {
    auto_answer: bool,
}

impl HostBackend for CliHost {
    fn emit_value(&self, event: &str, payload: Value) {
        match event {
            "agent-progress" => {
                let phase = payload.get("phase").and_then(Value::as_str).unwrap_or("");
                let step = payload.get("step").and_then(Value::as_u64).unwrap_or(0);
                let message = payload.get("message").and_then(Value::as_str).unwrap_or("");
                if let Some(thought) = payload.get("thought").and_then(Value::as_str) {
                    println!("[step {step}] {phase}: {message}\n          plan: {thought}");
                } else {
                    println!("[step {step}] {phase}: {message}");
                }
                if let Some(actions) = payload.get("actions").filter(|a| !a.is_null()) {
                    println!("          action: {actions}");
                }
            }
            "agent-confirm" => {
                let message = payload.get("message").and_then(Value::as_str).unwrap_or("");
                println!("[needs approval] {message}");
            }
            _ => {}
        }
    }

    fn auto_answer(&self) -> Option<bool> {
        Some(self.auto_answer)
    }
}

#[derive(PartialEq)]
enum Command {
    Run,
    Eval,
    Conformance,
    Daemon,
    Parallel,
    Soak,
}

struct Args {
    command: Command,
    suite: String,
    only: Option<String>,
    browser: Option<String>,
    repeat: u32,
    url: String,
    model: String,
    key: String,
    openai: bool,
    task: String,
    max_steps: u32,
    auto_approve: bool,
    windows: Vec<String>,
    model_slots: usize,
    minutes: u64,
}

fn parse_args() -> Result<Args, String> {
    let mut raw = std::env::args().skip(1);
    let command = match raw.next().as_deref() {
        Some("run") => Command::Run,
        Some("eval") => Command::Eval,
        Some("conformance") => Command::Conformance,
        Some("daemon") => Command::Daemon,
        Some("parallel") => Command::Parallel,
        Some("soak") => Command::Soak,
        _ => return Err(usage()),
    };
    let mut args = Args {
        command,
        suite: "basic".to_owned(),
        only: None,
        browser: None,
        repeat: 1,
        url: "http://localhost:1234/v1".to_owned(),
        model: String::new(),
        key: std::env::var("AURA_API_KEY").or_else(|_| std::env::var("OPENAI_API_KEY")).unwrap_or_default(),
        openai: false,
        task: String::new(),
        max_steps: 20,
        auto_approve: false,
        windows: Vec::new(),
        model_slots: 2,
        minutes: 60,
    };
    while let Some(flag) = raw.next() {
        let mut value = || raw.next().ok_or_else(|| format!("{flag} needs a value"));
        match flag.as_str() {
            "--url" => args.url = value()?,
            "--model" => args.model = value()?,
            "--key" => args.key = value()?,
            "--task" => args.task = value()?,
            "--suite" => args.suite = value()?,
            "--only" => args.only = Some(value()?),
            "--browser" => args.browser = Some(value()?),
            "--repeat" => args.repeat = value()?.parse().map_err(|_| "--repeat must be a number".to_owned())?,
            "--max-steps" => args.max_steps = value()?.parse().map_err(|_| "--max-steps must be a number".to_owned())?,
            "--window" => args.windows.push(value()?),
            "--minutes" => args.minutes = value()?.parse().map_err(|_| "--minutes must be a number".to_owned())?,
            "--model-slots" => args.model_slots = value()?.parse().map_err(|_| "--model-slots must be a number".to_owned())?,
            "--openai" => args.openai = true,
            "--auto-approve" => args.auto_approve = true,
            other => return Err(format!("unknown option {other}\n{}", usage())),
        }
    }
    if args.command == Command::Parallel && args.windows.is_empty() {
        return Err(format!("parallel needs one or more --window \"name=task\"\n{}", usage()));
    }
    if !matches!(args.command, Command::Conformance | Command::Soak) && (args.model.is_empty() || (args.command == Command::Run && args.task.is_empty())) {
        return Err(format!("--model is required (and --task for `run`)\n{}", usage()));
    }
    Ok(args)
}

fn usage() -> String {
    "usage: aura-cli run  --model <id> --task <text> [--url <base url>] [--openai] [--key <api key>] \
     [--max-steps <n>] [--auto-approve]\n       aura-cli parallel --model <id> --window \"Notepad=type hello\" --window \"Calculator=compute 6*7\" [--model-slots <n>]\n       aura-cli conformance [--only <group or name>]
       aura-cli soak --minutes <n> [--browser chrome]
       aura-cli eval --model <id> [--suite basic|intermediate|all] \
     [--only <task id>] [--repeat <n>] [--url <base url>] [--openai] [--key <api key>]\n       \
     (the key can also come from AURA_API_KEY or OPENAI_API_KEY)"
        .to_owned()
}

#[tokio::main]
async fn main() {
    let raw: Vec<String> = std::env::args().skip(1).collect();
    if raw.first().map(String::as_str) == Some("schedule") {
        std::process::exit(agent_aura_lib::schedule_cli::schedule_command(&raw[1..]));
    }
    if raw.first().map(String::as_str) == Some("control") {
        // Talks to a running `aura-cli run` (or the app) through its local control port.
        let port = std::env::var("AURA_CONTROL_PORT").ok().and_then(|p| p.parse().ok()).unwrap_or(47_900u16);
        let command = raw[1..].join(" ");
        match agent_aura_lib::control::send_command(port, &command) {
            Ok(reply) => {
                println!("{reply}");
                std::process::exit(if reply.starts_with("ok") { 0 } else { 1 });
            }
            Err(error) => {
                eprintln!("{error}");
                std::process::exit(2);
            }
        }
    }
    if matches!(raw.first().map(String::as_str), Some("look") | Some("do") | Some("demo") | Some("serve") | Some("pointer-check")) {
        agent_aura_lib::make_process_dpi_aware();
        if let Err(error) = KillSwitch::start_monitoring() {
            eprintln!("warning: the Esc kill switch could not be installed: {error}");
        }
        let code = tokio::task::spawn_blocking(move || agent_aura_lib::driver_cli::run(&raw)).await.unwrap_or(1);
        std::process::exit(code);
    }
    let args = match parse_args() {
        Ok(args) => args,
        Err(message) => {
            eprintln!("{message}");
            std::process::exit(2);
        }
    };
    agent_aura_lib::make_process_dpi_aware();
    if let Err(error) = KillSwitch::start_monitoring() {
        eprintln!("warning: the Esc kill switch could not be installed: {error}");
    }
    if args.command == Command::Soak {
        println!("soak: {} minute(s) of safe desktop cases in a separate Chrome, watching memory, handles and GDI objects. Do not use the mouse or keyboard; Esc stops.", args.minutes);
        agent_aura_lib::conformance::set_browser(args.browser.clone().or_else(|| Some("chrome".to_owned())));
        let minutes = args.minutes;
        let code = tokio::task::spawn_blocking(move || agent_aura_lib::soak::run(minutes)).await.unwrap_or(2);
        std::process::exit(code);
    }
    if args.command == Command::Conformance {
        println!("harness conformance: orders every input variation through the executor and checks the test page received it.
It drives the real screen through the default browser; press Esc to stop.
");
        let filter = args.only.clone();
        agent_aura_lib::conformance::set_browser(args.browser.clone());
        let outcome = tokio::task::spawn_blocking(move || {
            agent_aura_lib::conformance::run(filter.as_deref(), |r| {
                println!("{:<4} {:<7} {:<42} {:>5} ms  {}", if r.passed { "ok" } else { "FAIL" }, r.group, r.name, r.millis, r.detail)
            })
        })
        .await;
        match outcome {
            Ok(Ok(results)) => {
                let passed = results.iter().filter(|r| r.passed).count();
                println!("
{passed} / {} cases passed", results.len());
                let path = std::env::temp_dir().join("aura-conformance.json");
                let _ = std::fs::write(&path, serde_json::to_string_pretty(&results).unwrap_or_default());
                println!("report: {}", path.display());
                std::process::exit(if !results.is_empty() && passed == results.len() { 0 } else { 1 });
            }
            Ok(Err(error)) => {
                eprintln!("conformance could not start: {error}");
                std::process::exit(2);
            }
            Err(error) => {
                eprintln!("conformance crashed: {error}");
                std::process::exit(2);
            }
        }
    }
    let runtime = match AgentRuntime::new() {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("could not start Agent Aura: {error}");
            std::process::exit(2);
        }
    };
    let protocol = if args.openai { ApiProtocol::OpenAiResponses } else { ApiProtocol::OpenAiCompatibleChat };
    let base_url = if args.openai && args.url == "http://localhost:1234/v1" { String::new() } else { args.url.clone() };
    match runtime.set_provider(ProviderConfigInput { protocol, base_url, model: args.model.clone(), api_key: args.key.clone() }) {
        Ok(status) => println!("model: {} at {}", status.model.unwrap_or_default(), status.base_url.unwrap_or_default()),
        Err(error) => {
            eprintln!("provider settings rejected: {error}");
            std::process::exit(2);
        }
    }
    let host = Host::new(CliHost { auto_answer: args.auto_approve });
    if args.command == Command::Parallel {
        let mut jobs = Vec::new();
        for spec in &args.windows {
            match spec.split_once('=') {
                Some((window, task)) if !window.trim().is_empty() && !task.trim().is_empty() => jobs.push((window.trim().to_owned(), task.trim().to_owned())),
                _ => {
                    eprintln!("--window needs the form \"window name=task\" (got '{spec}')");
                    std::process::exit(2);
                }
            }
        }
        println!("{} worker(s); model calls overlap, mouse and keyboard take turns. Press Esc to stop.", jobs.len());
        match agent_aura_lib::agent::run_window_workers(&runtime, std::sync::Arc::new(host), jobs, args.max_steps, args.model_slots).await {
            Ok(report) => {
                for r in &report.results {
                    println!("worker {}: {} after {} step(s): {}", r.worker + 1, if r.completed { "COMPLETED" } else { "NOT COMPLETED" }, r.steps, r.message);
                }
                println!("input overlaps: {}, input turns: {}, waited {} ms, model requests: {}, peak concurrent: {}", report.input_overlaps, report.input_turns, report.input_wait_ms, report.model_requests, report.model_peak_concurrency);
                std::process::exit(if report.results.iter().all(|r| r.completed) { 0 } else { 1 });
            }
            Err(error) => {
                eprintln!("FAILED: {error}");
                std::process::exit(1);
            }
        }
    }
    if args.command == Command::Daemon {
        agent_aura_lib::schedule_cli::run_daemon(runtime, host).await;
        return;
    }
    if args.command == Command::Eval {
        println!("suite: {} (tasks open and close real windows; press Esc to stop)\n", args.suite);
        let rounds = args.repeat.max(1);
        let mut tally: std::collections::BTreeMap<String, (u32, u32)> = std::collections::BTreeMap::new();
        let mut all_passed = true;
        for round in 1..=rounds {
            println!("--- round {round} of {rounds} ---");
            let report = agent_aura_lib::eval::run_suite(&runtime, &host, &args.suite, args.only.as_deref(), &args.model).await;
            println!("\n{}", report.table());
            if let Some(path) = agent_aura_lib::eval::save_report(&runtime, &report) {
                println!("report: {}", path.display());
            }
            all_passed &= report.total > 0 && report.passed == report.total;
            for result in &report.results {
                let entry = tally.entry(result.id.clone()).or_default();
                entry.1 += 1;
                entry.0 += u32::from(result.passed);
            }
        }
        if rounds > 1 {
            println!("\npass rate over {rounds} rounds:");
            for (id, (passed, total)) in &tally {
                println!("  {id:<22} {passed}/{total}");
            }
        }
        std::process::exit(if all_passed { 0 } else { 1 });
    }
    println!("task: {}\nPress Esc to stop, Pause to pause or resume, Ctrl+Alt+Space to pause for a new instruction.\n", args.task);
    // Local control port: `aura-cli control stop|pause|resume|status|steer <text>` from another terminal.
    let control_port = std::env::var("AURA_CONTROL_PORT").ok().and_then(|p| p.parse().ok()).unwrap_or(47_900u16);
    match agent_aura_lib::control::serve_port(control_port).await {
        Ok(port) => println!("control port: 127.0.0.1:{port} (aura-cli control <stop|pause|resume|status|steer text>)"),
        Err(error) => eprintln!("warning: no control port: {error}"),
    }
    let outcome = execute_run(&runtime, &host, args.task.clone(), args.max_steps).await;
    println!("\ntraces: {}", runtime.data_root().join("runs").display());
    match outcome {
        Ok(response) => {
            let verdict = if response.completed {
                "COMPLETED"
            } else if response.stopped {
                "STOPPED"
            } else {
                "INCOMPLETE"
            };
            println!("{verdict} after {} executed step(s): {}", response.steps_executed, response.message);
            std::process::exit(if response.completed { 0 } else { 1 });
        }
        Err(error) => {
            eprintln!("FAILED: {error}");
            std::process::exit(1);
        }
    }
}
