#![windows_subsystem = "windows"]
// Later tasks consume the whole API; remove these allows as they land.
#[allow(dead_code)]
mod detail;
#[allow(dead_code)]
mod event;
#[allow(dead_code)]
mod hook;
#[allow(dead_code)]
mod pipe;
#[allow(dead_code)]
mod procs;
#[allow(dead_code)]
mod state;

fn ui_main() {}

fn main() {
    if std::env::args().nth(1).as_deref() == Some("hook") {
        std::process::exit(hook::run());
    }
    ui_main();
}
