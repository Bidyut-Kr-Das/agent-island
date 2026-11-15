use crate::{pipe, procs};
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Read, Write};
use std::os::windows::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

const DETACHED_PROCESS: u32 = 0x8;
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;

pub enum Reply {
    Allow,
    Deny,
}

pub fn decision_output(reply: Option<Reply>, verified: bool) -> Option<String> {
    if !verified {
        return None;
    }
    let decision = match reply? {
        Reply::Allow => r#"{"behavior":"allow"}"#,
        Reply::Deny => r#"{"behavior":"deny","message":"Denied from Agent Islands"}"#,
    };
    Some(format!(
        r#"{{"hookSpecificOutput":{{"hookEventName":"PermissionRequest","decision":{decision}}}}}"#
    ))
}

fn canon(p: &Path) -> String {
    p.canonicalize().unwrap_or_else(|_| p.to_path_buf()).to_string_lossy().to_lowercase()
}

pub fn same_exe(a: &Path, b: &Path) -> bool {
    canon(a) == canon(b)
}

fn spawn_island() {
    let Ok(exe) = std::env::current_exe() else { return };
    let go = |flags: u32| {
        Command::new(&exe)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(flags)
            .spawn()
    };
    let base = DETACHED_PROCESS | CREATE_NO_WINDOW;
    if go(base | CREATE_BREAKAWAY_FROM_JOB).is_err() {
        let _ = go(base);
    }
}

fn connect_retry(name: &str, spawn: bool) -> Option<(std::fs::File, u32)> {
    let mut spawned = false;
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        match pipe::connect_to(name) {
            Ok(c) => return Some(c),
            Err(e) if pipe::is_not_found(&e) && spawn && Instant::now() < deadline => {
                if !spawned {
                    spawn_island();
                    spawned = true;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(_) => return None,
        }
    }
}

fn read_reply(f: std::fs::File) -> Option<Reply> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut line = String::new();
        let _ = BufReader::new(f).read_line(&mut line);
        let _ = tx.send(line);
    });
    let line = rx.recv_timeout(Duration::from_secs(35)).ok()?;
    match serde_json::from_str::<Value>(&line).ok()?["decision"].as_str()? {
        "allow" => Some(Reply::Allow),
        "deny" => Some(Reply::Deny),
        _ => None,
    }
}

/// Forwards the hook event; returns the stdout text to print, if any.
fn run_with(name: &str, spawn: bool, input: &str) -> Option<String> {
    let event: Value = serde_json::from_str(input).ok()?;
    let (mut f, server_pid) = connect_retry(name, spawn)?;
    let verified = match (procs::exe_path(server_pid), std::env::current_exe()) {
        (Some(a), Ok(b)) => same_exe(&a, &b),
        _ => false,
    };
    if !verified {
        return None;
    }
    let wants_reply = event["hook_event_name"] == "PermissionRequest"
        && event["tool_name"] != "AskUserQuestion";
    let chain = procs::ancestors(std::process::id());
    let mut line = json!({"event": event, "chain": chain}).to_string();
    line.push('\n');
    f.write_all(line.as_bytes()).ok()?;
    if !wants_reply {
        return None;
    }
    decision_output(read_reply(f), verified)
}

pub fn run() -> i32 {
    let mut input = String::new();
    if std::io::stdin().read_to_string(&mut input).is_ok() {
        if let Some(out) = run_with(&pipe::pipe_name(), true, &input) {
            let mut so = std::io::stdout();
            let _ = so.write_all(out.as_bytes());
            let _ = so.flush();
        }
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipe::Server;
    use std::io::{BufRead, BufReader, Write};
    use std::time::{Duration, Instant};

    const ALLOW: &str = r#"{"hookSpecificOutput":{"hookEventName":"PermissionRequest","decision":{"behavior":"allow"}}}"#;

    #[test]
    fn allow_verified_prints_allow_json() {
        assert_eq!(decision_output(Some(Reply::Allow), true).as_deref(), Some(ALLOW));
    }

    #[test]
    fn deny_verified_prints_deny_with_message() {
        let want = r#"{"hookSpecificOutput":{"hookEventName":"PermissionRequest","decision":{"behavior":"deny","message":"Denied from Agent Islands"}}}"#;
        assert_eq!(decision_output(Some(Reply::Deny), true).as_deref(), Some(want));
    }

    #[test]
    fn unverified_prints_nothing() {
        assert_eq!(decision_output(Some(Reply::Allow), false), None);
        assert_eq!(decision_output(Some(Reply::Deny), false), None);
    }

    #[test]
    fn no_reply_prints_nothing() {
        assert_eq!(decision_output(None, true), None);
    }

    #[test]
    fn connect_failure_prints_nothing() {
        let name = format!(r"\\.\pipe\ai-test-nobody-{}", std::process::id());
        let t = Instant::now();
        let out = run_with(&name, false, r#"{"hook_event_name":"PermissionRequest","tool_name":"Bash"}"#);
        assert_eq!(out, None);
        assert!(t.elapsed() < Duration::from_secs(3));
    }

    #[test]
    fn same_exe_ignores_case_and_canonicalizes() {
        let exe = std::env::current_exe().unwrap();
        let upper = std::path::PathBuf::from(exe.to_string_lossy().to_uppercase());
        assert!(same_exe(&exe, &upper));
        assert!(!same_exe(&exe, std::path::Path::new(r"C:\Windows\notepad.exe")));
    }

    fn serve(name: &str, reply: Option<&'static str>) -> std::thread::JoinHandle<String> {
        let mut server = Server::create_first_named(name).unwrap();
        std::thread::spawn(move || {
            let f = server.accept().unwrap();
            let mut line = String::new();
            BufReader::new(&f).read_line(&mut line).unwrap();
            if let Some(r) = reply {
                (&f).write_all(r.as_bytes()).unwrap();
            }
            line
        })
    }

    #[test]
    fn permission_request_gets_allow_from_server() {
        let name = format!(r"\\.\pipe\ai-test-allow-{}", std::process::id());
        let t = serve(&name, Some("{\"decision\":\"allow\"}\n"));
        let input = r#"{"hook_event_name":"PermissionRequest","tool_name":"Bash"}"#;
        assert_eq!(run_with(&name, false, input).as_deref(), Some(ALLOW));
        let sent: serde_json::Value = serde_json::from_str(&t.join().unwrap()).unwrap();
        assert_eq!(sent["event"]["tool_name"], "Bash");
        assert!(sent["chain"].is_array());
    }

    #[test]
    fn ask_user_question_permission_does_not_wait() {
        let name = format!(r"\\.\pipe\ai-test-ask-{}", std::process::id());
        let t = serve(&name, None);
        let input = r#"{"hook_event_name":"PermissionRequest","tool_name":"AskUserQuestion"}"#;
        let start = Instant::now();
        assert_eq!(run_with(&name, false, input), None);
        assert!(start.elapsed() < Duration::from_secs(3));
        t.join().unwrap();
    }
}
