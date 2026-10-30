// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[cfg(windows)]
extern "system" {
    fn AttachConsole(dwProcessId: u32) -> i32;
}

fn main() {
    #[cfg(windows)]
    unsafe {
        // If launched from PowerShell or Command Prompt, attach console output to it.
        // ATTACH_PARENT_PROCESS = 0xFFFFFFFF
        AttachConsole(0xFFFFFFFF);
    }

    agent_aura_lib::run();
}
