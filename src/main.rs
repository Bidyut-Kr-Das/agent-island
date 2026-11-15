#![windows_subsystem = "windows"]
#[allow(dead_code)]
mod detail;
mod event;
mod focus;
mod hook;
mod pipe;
mod server;
mod procs;
#[allow(dead_code)] // Task 7 reads the rest (panel, cards)
mod state;
mod ui;

fn main() {
    if std::env::args().nth(1).as_deref() == Some("hook") {
        std::process::exit(hook::run());
    }
    ui::run();
}
