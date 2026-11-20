#![windows_subsystem = "windows"]
mod command;
mod detail;
mod event;
mod focus;
mod gate;
mod hook;
mod listen;
mod phone;
mod pipe;
mod server;
mod procs;
mod settings;
mod state;
mod telegram;
mod ui;

fn main() {
    // Before any DLL loads by name (delay-loaded opengl32, GPU driver, COM): search only system32 and
    // the exe folder, never the current directory or PATH. Covers run-time LoadLibrary calls, which
    // /DEPENDENTLOADFLAG (load-time imports only) would not.
    use windows::Win32::System::LibraryLoader::{
        LOAD_LIBRARY_SEARCH_APPLICATION_DIR, LOAD_LIBRARY_SEARCH_SYSTEM32, SetDefaultDllDirectories,
    };
    let _ = unsafe { SetDefaultDllDirectories(LOAD_LIBRARY_SEARCH_SYSTEM32 | LOAD_LIBRARY_SEARCH_APPLICATION_DIR) };
    if std::env::args().nth(1).as_deref() == Some("hook") {
        std::process::exit(hook::run());
    }
    if std::env::args().nth(1).as_deref() == Some("listen") {
        let arg = |n| std::env::args().nth(n).unwrap_or_default();
        std::process::exit(listen::run(&arg(2), &arg(3)));
    }
    if std::env::args().nth(1).as_deref() == Some("forget") {
        if let Some(p) = settings::path() {
            settings::forget(&p);
        }
        println!("Removed the stored phone token, the pairing and the login entry. If the island is running, disconnect the phone on its settings card or end the agent-islands process.");
        std::process::exit(0);
    }
    ui::run();
}
