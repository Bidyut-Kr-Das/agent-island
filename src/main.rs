#![windows_subsystem = "windows"]
#[allow(dead_code)]
mod detail;
mod event;
mod focus;
mod gate;
mod hook;
mod pipe;
mod server;
mod procs;
#[allow(dead_code)] // Task 7 reads the rest (panel, cards)
mod state;
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
    ui::run();
}
