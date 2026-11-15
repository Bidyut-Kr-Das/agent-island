use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

const EXE: &str = env!("CARGO_BIN_EXE_agent-islands");

fn pipe_exists(pipe: &str) -> bool {
    // A probe connection is harmless: the island's connection thread just ends on EOF.
    std::path::Path::new(pipe).exists()
}

fn wait(mut cond: impl FnMut() -> bool, secs: u64) -> bool {
    let end = Instant::now() + Duration::from_secs(secs);
    while Instant::now() < end {
        if cond() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    cond()
}

/// Runs `agent-islands hook` and asserts it exits and its stdout hits EOF within 3 s.
fn hook(pipe: &str, fixture: &str) {
    let input = std::fs::read(format!("{}/tests/fixtures/{fixture}", env!("CARGO_MANIFEST_DIR"))).unwrap();
    let mut child = Command::new(EXE)
        .arg("hook")
        .env("AGENT_ISLANDS_PIPE", pipe)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(&input).unwrap();
    let mut out = child.stdout.take().unwrap();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut s = Vec::new();
        let _ = out.read_to_end(&mut s);
        let _ = tx.send(());
    });
    assert!(rx.recv_timeout(Duration::from_secs(3)).is_ok(), "hook stdout did not reach EOF: island holds it");
    assert!(wait(|| child.try_wait().unwrap().is_some(), 3), "hook did not exit");
}

struct EndOnDrop(String);
impl Drop for EndOnDrop {
    fn drop(&mut self) {
        if pipe_exists(&self.0) {
            hook(&self.0, "SessionEnd.json");
        }
    }
}

#[test]
fn island_starts_on_hook_and_exits_after_session_end() {
    let pipe = format!(r"\\.\pipe\agent-islands-e2e-{}", std::process::id());
    let _guard = EndOnDrop(pipe.clone());
    hook(&pipe, "SessionStart.json");
    assert!(wait(|| pipe_exists(&pipe), 3), "island did not start");
    hook(&pipe, "SessionEnd.json");
    assert!(wait(|| !pipe_exists(&pipe), 5), "island did not exit");
}
