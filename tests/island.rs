use std::io::{Read, Write};
use std::path::{Path, PathBuf};
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

fn hook(pipe: &str, fixture: &str) {
    hook_with(Path::new(EXE), pipe, fixture);
}

/// Runs `<exe> hook` and asserts it exits and its stdout hits EOF within 3 s.
fn hook_with(exe: &Path, pipe: &str, fixture: &str) {
    let input = std::fs::read(format!("{}/tests/fixtures/{fixture}", env!("CARGO_MANIFEST_DIR"))).unwrap();
    let mut child = Command::new(exe)
        .arg("hook")
        .env("AGENT_ISLANDS_PIPE", pipe)
        // A paired phone keeps an island alive and messages the real chat: hide the dev's token and settings.
        .env("APPDATA", env!("CARGO_TARGET_TMPDIR"))
        .env_remove("CLAUDE_PLUGIN_OPTION_TELEGRAM_BOT_TOKEN")
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

/// (pid, lowercased path) of the islands running from under `dir`. Never sees a real island.
fn islands_under(dir: &Path) -> Vec<(u32, String)> {
    let out = Command::new("powershell")
        .args(["-NoProfile", "-Command", r#"Get-Process agent-islands -ErrorAction SilentlyContinue | ForEach-Object { "$($_.Id) $($_.Path)" }"#])
        .output()
        .unwrap();
    let dir = dir.to_string_lossy().replace('/', r"\").to_lowercase();
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| {
            let (pid, path) = l.trim().split_once(' ')?;
            let path = path.to_lowercase();
            path.starts_with(&dir).then(|| Some((pid.parse().ok()?, path)))?
        })
        .collect()
}

/// Stops whatever a failed test left running, then clears its folder.
struct KillUnder(PathBuf);
impl Drop for KillUnder {
    fn drop(&mut self) {
        for (pid, _) in islands_under(&self.0) {
            let _ = Command::new("taskkill").args(["/F", "/PID", &pid.to_string()]).output();
        }
        std::thread::sleep(Duration::from_millis(200));
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn takeover_goes_only_forward() {
    let pipe = format!(r"\\.\pipe\agent-islands-takeover-{}", std::process::id());
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("takeover-{}", std::process::id()));
    let _guard = KillUnder(dir.clone());
    let install = |version: &str| {
        let exe = dir.join("agent-islands").join(version).join("bin").join("agent-islands.exe");
        std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
        std::fs::copy(EXE, &exe).unwrap();
        exe
    };
    let (old, new) = (install("0.0.1"), install("0.0.2"));
    // A different build: otherwise same_exe calls the two installs identical.
    std::fs::OpenOptions::new().append(true).open(&new).unwrap().write_all(&[0]).unwrap();
    let running = |version: &str| {
        let list = islands_under(&dir);
        (list.len() == 1 && list[0].1.contains(&format!(r"\{version}\bin\"))).then(|| list[0].0)
    };

    hook_with(&old, &pipe, "SessionStart.json");
    if !wait(|| running("0.0.1").is_some(), 5) {
        // shortcut: the GitHub runner cannot keep the island's window up, so this test only proves anything on a desktop.
        assert!(std::env::var_os("CI").is_some(), "old island did not start");
        eprintln!("skipped: no island on this CI machine");
        return;
    }

    hook_with(&new, &pipe, "SessionStart.json");
    assert!(wait(|| running("0.0.2").is_some(), 5), "newer hook did not replace the island: {:?}", islands_under(&dir));
    let pid = running("0.0.2");

    hook_with(&old, &pipe, "SessionStart.json");
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(running("0.0.2"), pid, "older hook disturbed the newer island");

    hook_with(&new, &pipe, "SessionEnd.json");
    // 3 s empty timer, seen on idle frames 3 s apart: up to 6 s.
    assert!(wait(|| !pipe_exists(&pipe), 10), "island did not exit");
}
