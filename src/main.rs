#![windows_subsystem = "windows"]
// Task 6 consumes the rest of state/ui; remove these allows as they land.
#[allow(dead_code)]
mod detail;
mod event;
mod hook;
mod pipe;
mod server;
mod procs;
#[allow(dead_code)]
mod state;
#[allow(dead_code)]
mod ui;

fn ui_main() {
    use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
    let (tx, rx) = std::sync::mpsc::channel();
    if server::spawn(tx, || {}).is_err() {
        return; // another island owns the pipe
    }
    let mut core = ui::Core::new();
    let now_ms = || SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64);
    loop {
        if let Ok(msg) = rx.recv_timeout(Duration::from_millis(250)) {
            core.handle(msg, now_ms());
        }
        if core.tick(Instant::now()) {
            return;
        }
    }
}

fn main() {
    if std::env::args().nth(1).as_deref() == Some("hook") {
        std::process::exit(hook::run());
    }
    ui_main();
}
