use tauri::Manager;

pub mod agent;
pub mod capture;
pub mod conformance;
pub mod control;
pub mod coords;
pub mod db;
pub mod desktop;
pub mod display_scale;
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
pub mod recovery;
pub mod reliability;
pub mod settle;
pub mod settings;
pub mod single_instance;
pub mod soak;
pub mod schedule;
pub mod schedule_app;
pub mod schedule_cli;
pub mod schedule_prompt;
pub mod sound;
pub mod system_state;
pub mod testlog;
pub mod trace;
pub mod vault;
pub mod voice;
pub mod voice_input;
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

fn data_root() -> std::path::PathBuf {
    std::env::var_os("LOCALAPPDATA").map(std::path::PathBuf::from).unwrap_or_else(std::env::temp_dir).join("AgentAura")
}

/// The window title, also used to find the open copy when the app is started a second time.
const WINDOW_TITLE: &str = "Agent Aura";

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    make_process_dpi_aware();

    // One copy of the app per data folder: a second one would run every schedule again and fight
    // the first over the mouse. Starting it again brings the open window forward instead.
    let Some(_only_app) = single_instance::try_acquire("app", &data_root()) else {
        single_instance::show_existing_window(WINDOW_TITLE);
        return;
    };

    // A panic puts the cursor, the accessibility shortcuts and raised windows back at once, and
    // is logged next to the database.
    std::panic::set_hook(Box::new(|info| {
        recovery::undo_own();
        let err_msg = format!("Agent Aura Fatal Error: {}\nLocation: {:?}\n", info, info.location());
        let _ = std::fs::create_dir_all(data_root());
        let _ = std::fs::write(data_root().join("agent-aura-crash.log"), &err_msg);
        eprintln!("{}", err_msg);
    }));

    // What a crashed or force-closed copy left behind (cursor, shortcuts, raised windows, display
    // scale) is put back; a previous run killed mid-way may also have left the working cursor.
    recovery::recover_leftovers();
    overlay::cursor::restore();

    let agent_runtime = agent::AgentRuntime::new().expect("failed to initialize Agent Aura runtime");

    tauri::Builder::default()
        .manage(agent_runtime)
        .invoke_handler(tauri::generate_handler![
            agent::configure_provider,
            agent::get_provider_status,
            agent::clear_provider,
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
            agent::submit_task_review,
            schedule_app::list_schedules,
            schedule_app::add_schedule,
            schedule_app::remove_schedule,
            schedule_app::set_schedule_enabled,
            schedule_app::schedule_from_prompt,
            agent::app::list_model_presets,
            agent::app::connect_model,
            agent::app::use_model_preset,
            agent::app::delete_model_preset,
            agent::app::ping_provider,
            agent::app::calibrate_model,
            agent::app::get_voice_enabled,
            agent::app::set_voice_enabled,
            agent::app::start_window_drag,
            agent::app::list_task_runs,
            agent::app::get_task_run_detail,
            agent::app::get_step_screenshot,
            voice_input::voice_input_status,
            voice_input::set_sarvam_key,
            voice_input::clear_sarvam_key,
            voice_input::listen_and_translate,
            voice_input::stop_listening,
        ])
        // Closing the window while schedules are on keeps Aura running in the tray, so they
        // still happen; with no schedules, closing quits as usual.
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                if window.label() == "main" && schedule_app::has_enabled_schedules() {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
        })
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
                        // Touching the mini player is not the user taking the computer back.
                        input::takeover::set_own_window(hwnd.0 as isize);
                        if let Err(error) = desktop::affinity::exclude_from_capture(hwnd.0 as _) {
                            eprintln!("[aura] main window could not be excluded from capture: {error}");
                        }
                    }
                    Err(error) => eprintln!("[aura] no window handle for capture exclusion: {error}"),
                }
            }

            crate::input::killswitch::KillSwitch::start_monitoring()
                .map_err(std::io::Error::other)?;
            // In the app, Esc / Pause / Ctrl+Alt+Space only act while a task runs.
            crate::input::killswitch::keys_only_during_runs(true);

            build_tray(app)?;

            // Saved schedules run while the app is open (idle between runs: nothing captured).
            schedule_app::start(app.handle().clone());

            // Self-test (debug builds only): run one task through the same path as the Run button
            // and log what happens.
            #[cfg(debug_assertions)]
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

fn show_main_window(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
}

/// The tray icon: click to open the window, right-click for Open / Quit. It is how Aura is reached
/// while it keeps schedules running with the window closed.
fn build_tray(app: &tauri::App) -> tauri::Result<()> {
    use tauri::menu::{Menu, MenuItem};
    use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
    let open = MenuItem::with_id(app, "open", "Open Agent Aura", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&open, &quit])?;
    let mut tray = TrayIconBuilder::with_id("aura")
        .tooltip("Agent Aura")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "open" => show_main_window(app),
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = event {
                show_main_window(tray.app_handle());
            }
        });
    if let Some(icon) = app.default_window_icon() {
        tray = tray.icon(icon.clone());
    }
    tray.build(app)?;
    Ok(())
}
