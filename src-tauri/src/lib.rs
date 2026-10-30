use tauri::Manager;

pub mod agent;
pub mod capture;
pub mod conformance;
pub mod control;
pub mod coords;
pub mod db;
pub mod desktop;
pub mod driver_cli;
pub mod eval;
pub mod eval_fixture;
pub mod input;
pub mod marks;
pub mod mock_server;
#[cfg(test)]
mod loop_tests;
pub mod model_schema;
pub mod overlay;
pub mod perception;
pub mod profiles;
pub mod radar;
pub mod recipe;
pub mod reliability;
pub mod settle;
pub mod settings;
pub mod soak;
pub mod schedule;
pub mod schedule_app;
pub mod schedule_cli;
pub mod sound;
pub mod system_state;
pub mod testlog;
pub mod trace;
pub mod vault;
pub mod voice;
pub mod workers;

/// Per-monitor DPI awareness must be set before any window is created or any screen metric is
/// read. Otherwise Windows reports virtualized (scaled-down) sizes, so the HUD overlay covered only
/// 1/scale of the screen and pixel coordinates disagreed with the captured frames.
pub fn make_process_dpi_aware() {
    use windows_sys::Win32::UI::HiDpi::{
        SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
    };
    let ok = unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
    if ok == 0 {
        // Already set (for example by the manifest) or unsupported; both are fine.
        eprintln!("[aura] SetProcessDpiAwarenessContext did not change anything (already set?)");
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    make_process_dpi_aware();

    // Install global panic hook to capture any fatal panics to file and console
    std::panic::set_hook(Box::new(|info| {
        let err_msg = format!("Agent Aura Fatal Error: {}\nLocation: {:?}\n", info, info.location());
        let _ = std::fs::write("agent-aura-crash.log", &err_msg);
        eprintln!("{}", err_msg);
    }));

    // A previous run killed mid-way may have left the working cursor behind.
    overlay::cursor::restore();

    let agent_runtime = agent::AgentRuntime::new().expect("failed to initialize Agent Aura runtime");

    tauri::Builder::default()
        .manage(agent_runtime)
        .invoke_handler(tauri::generate_handler![
            agent::configure_provider,
            agent::get_provider_status,
            agent::clear_provider,
            agent::test_provider,
            agent::plan_next_step,
            agent::execute_pending_step,
            agent::discard_pending_step,
            agent::run_autonomous_task,
            agent::stop_autonomous_task,
            agent::pause_autonomous_task,
            agent::resume_autonomous_task,
            agent::steer_autonomous_task,
            agent::run_control_status,
            agent::poll_events,
            agent::set_steer_box_open,
            agent::set_mini_height,
            agent::keep_run_panel_on_top,
            agent::answer_confirmation,
            agent::get_last_task_run,
            agent::submit_task_review,
            schedule_app::list_schedules,
            schedule_app::add_schedule,
            schedule_app::remove_schedule,
            schedule_app::set_schedule_enabled,
        ])
        .setup(|app| {
            if cfg!(debug_assertions) {
                app.handle().plugin(
                    tauri_plugin_log::Builder::default()
                        .level(log::LevelFilter::Info)
                        .build(),
                )?;
            }

            // Ensure main window is visible and focused on startup
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.show();
                let _ = window.set_focus();
                // Keep Aura's own UI out of every screenshot the model sees.
                match window.hwnd() {
                    Ok(hwnd) => {
                        if let Err(error) = desktop::affinity::exclude_from_capture(hwnd.0 as _) {
                            eprintln!("[aura] main window could not be excluded from capture: {error}");
                        }
                    }
                    Err(error) => eprintln!("[aura] no window handle for capture exclusion: {error}"),
                }
            }

            crate::input::killswitch::KillSwitch::start_monitoring()
                .map_err(std::io::Error::other)?;

            // Saved schedules run while the app is open (idle between runs: nothing captured).
            schedule_app::start(app.handle().clone());

            // Self-test: run one task through the same path as the Run button and log what happens.
            if let Ok(task) = std::env::var("AURA_SELFTEST_TASK") {
                let handle = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    tokio::time::sleep(std::time::Duration::from_secs(3)).await;
                    let log = std::env::var_os("LOCALAPPDATA").map(std::path::PathBuf::from).unwrap_or_else(std::env::temp_dir).join("AgentAura").join("selftest.log");
                    let _ = std::fs::write(&log, "");
                    let Some(window) = handle.get_webview_window("main") else { return };
                    let state = handle.state::<agent::AgentRuntime>();
                    // Where are Aura's own windows (panel, glow overlay) while the run is going?
                    let dump_log = log.clone();
                    std::thread::spawn(move || {
                        use std::io::Write;
                        std::thread::sleep(std::time::Duration::from_secs(8));
                        unsafe extern "system" fn each(hwnd: windows_sys::Win32::Foundation::HWND, lparam: isize) -> i32 {
                            use windows_sys::Win32::UI::WindowsAndMessaging::{GetClassNameW, GetWindowRect, GetWindowThreadProcessId, IsWindowVisible};
                            let mut pid = 0u32;
                            GetWindowThreadProcessId(hwnd, &mut pid);
                            if pid == std::process::id() {
                                let mut class = [0u16; 80];
                                let n = GetClassNameW(hwnd, class.as_mut_ptr(), 80) as usize;
                                let mut r = std::mem::zeroed();
                                GetWindowRect(hwnd, &mut r);
                                let lines = &mut *(lparam as *mut Vec<String>);
                                lines.push(format!("window class={} visible={} rect=({},{} {}x{})", String::from_utf16_lossy(&class[..n]), IsWindowVisible(hwnd), r.left, r.top, r.right - r.left, r.bottom - r.top));
                            }
                            1
                        }
                        let mut lines: Vec<String> = Vec::new();
                        unsafe { windows_sys::Win32::UI::WindowsAndMessaging::EnumWindows(Some(each), &mut lines as *mut _ as isize) };
                        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&dump_log) {
                            for l in lines { let _ = writeln!(f, "OWN {l}"); }
                        }
                    });
                    let host = agent::selftest_host(window, log.clone());
                    let outcome = agent::execute_run(&state, &host, task, 6).await;
                    let _ = std::fs::write(log.with_extension("result"), format!("{outcome:?}"));
                });
            }

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while building tauri application");
}
