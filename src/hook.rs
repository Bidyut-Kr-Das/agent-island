use crate::{gate, pipe, procs};
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Read, Write};
use std::os::windows::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};
use windows::Win32::Foundation::{HANDLE_FLAG_INHERIT, HANDLE_FLAGS, SetHandleInformation};
use windows::Win32::System::Console::{GetStdHandle, STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE};

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

/// Gated PreToolUse (spec 9a): always prints a decision; allow only with a verified island and a click.
pub fn pretool_output(reply: Option<Reply>, verified: bool, timed_out: bool) -> String {
    let deny = |why: &str| format!(r#""deny","permissionDecisionReason":"{why}""#);
    let decision = match (verified, reply) {
        (true, Some(Reply::Allow)) => r#""allow""#.to_owned(),
        (true, Some(Reply::Deny)) => deny("Denied from Agent Islands"),
        (true, None) if timed_out => deny("No answer in Agent Islands within 30 seconds"),
        _ => deny("Agent Islands is not running"),
    };
    format!(r#"{{"hookSpecificOutput":{{"hookEventName":"PreToolUse","permissionDecision":{decision}}}}}"#)
}

/// Same file content. Two installs of one build must match; any IO error means not the same.
pub fn same_exe(a: &Path, b: &Path) -> bool {
    let (Ok(ca), Ok(cb)) = (a.canonicalize(), b.canonicalize()) else { return false };
    if ca.to_string_lossy().to_lowercase() == cb.to_string_lossy().to_lowercase() {
        return true;
    }
    let (Ok(ma), Ok(mb)) = (std::fs::metadata(&ca), std::fs::metadata(&cb)) else { return false };
    if ma.len() != mb.len() {
        return false;
    }
    matches!((std::fs::read(&ca), std::fs::read(&cb)), (Ok(x), Ok(y)) if x == y)
}

/// The island must not inherit Claude Code's stdio pipes, or the hook call never completes.
fn no_inherit_std_handles() {
    for id in [STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, STD_ERROR_HANDLE] {
        if let Ok(h) = unsafe { GetStdHandle(id) } {
            let _ = unsafe { SetHandleInformation(h, HANDLE_FLAG_INHERIT.0, HANDLE_FLAGS(0)) };
        }
    }
}

fn island_command(exe: &Path) -> Command {
    let mut cmd = Command::new(exe);
    cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    // Never the project folder: the island would lock it and add it to the DLL search path.
    cmd.current_dir(exe.parent().unwrap_or(Path::new(r"C:\")));
    cmd
}

fn spawn_island() {
    no_inherit_std_handles();
    let Ok(exe) = std::env::current_exe() else { return };
    let go = |flags: u32| island_command(&exe).creation_flags(flags).spawn();
    let base = DETACHED_PROCESS | CREATE_NO_WINDOW;
    if go(base | CREATE_BREAKAWAY_FROM_JOB).is_err() {
        let _ = go(base);
    }
}

/// The pipe, the server pid, and whether this call started the island.
fn connect_retry(name: &str, spawn: bool) -> Option<(std::fs::File, u32, bool)> {
    let mut spawned = false;
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        match pipe::connect_to(name) {
            Ok((f, pid)) => return Some((f, pid, spawned)),
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

/// The island's decision: "allow", "deny", or "timeout" / "skip" (gated requests only).
fn read_reply(f: std::fs::File) -> Option<String> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut line = String::new();
        let _ = BufReader::new(f).read_line(&mut line);
        let _ = tx.send(line);
    });
    let line = rx.recv_timeout(Duration::from_secs(35)).ok()?;
    Some(serde_json::from_str::<Value>(&line).ok()?["decision"].as_str()?.to_owned())
}

fn to_reply(d: Option<&str>) -> Option<Reply> {
    match d? {
        "allow" => Some(Reply::Allow),
        "deny" => Some(Reply::Deny),
        _ => None,
    }
}

/// Only events that create or locate a session's processes need the (slow) process chain, plus every
/// event that started the island (it knows no pids yet). Stop ends every turn, so any session seen
/// mid-session gets its pids within one turn.
fn needs_chain(event: &Value, spawned: bool) -> bool {
    spawned || match event["hook_event_name"].as_str() {
        Some("SessionStart" | "UserPromptSubmit" | "PermissionRequest" | "Notification" | "Stop" | "StopFailure") => true,
        Some("PreToolUse") => event["tool_name"] == "AskUserQuestion",
        _ => false,
    }
}

/// Forwards the hook event; returns the stdout text to print, if any.
fn run_with(name: &str, spawn: bool, input: &str) -> Option<String> {
    let event: Value = serde_json::from_str(input).ok()?;
    let gated = event["hook_event_name"] == "PreToolUse"
        && gate::is_gated(event["tool_name"].as_str().unwrap_or_default(), event["tool_input"]["command"].as_str().unwrap_or_default());
    let out = forward(name, spawn, event, gated);
    if gated { Some(out.unwrap_or_else(|| pretool_output(None, false, false))) } else { out }
}

fn forward(name: &str, spawn: bool, event: Value, gated: bool) -> Option<String> {
    let (mut f, server_pid, spawned) = connect_retry(name, spawn)?;
    let verified = match (procs::exe_path(server_pid), std::env::current_exe()) {
        (Some(a), Ok(b)) => same_exe(&a, &b),
        _ => false,
    };
    if !verified {
        return None;
    }
    let wants_reply = event["hook_event_name"] == "PermissionRequest"
        && event["tool_name"] != "AskUserQuestion";
    let chain = if gated || needs_chain(&event, spawned) { procs::ancestors(std::process::id()) } else { Vec::new() };
    let mut msg = json!({"event": event, "chain": chain});
    if gated {
        msg["gated"] = json!(true);
    }
    let mut line = msg.to_string();
    line.push('\n');
    f.write_all(line.as_bytes()).ok()?;
    if gated {
        let d = read_reply(f);
        // Gating paused in the island: print nothing, Claude Code's own permission rules decide.
        if d.as_deref() == Some("skip") {
            return Some(String::new());
        }
        return Some(pretool_output(to_reply(d.as_deref()), verified, d.as_deref() == Some("timeout")));
    }
    if !wants_reply {
        return None;
    }
    decision_output(to_reply(read_reply(f).as_deref()), verified)
}

pub fn run() -> i32 {
    let mut input = String::new();
    if std::io::stdin().read_to_string(&mut input).is_ok()
        && let Some(out) = run_with(&pipe::pipe_name(), true, &input)
    {
        let mut so = std::io::stdout();
        let _ = so.write_all(out.as_bytes());
        let _ = so.flush();
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
    fn chain_only_for_events_that_find_processes() {
        let ev = |name: &str, tool: &str| json!({"hook_event_name": name, "tool_name": tool});
        for name in ["SessionStart", "UserPromptSubmit", "PermissionRequest", "Notification", "Stop", "StopFailure"] {
            assert!(needs_chain(&ev(name, "Bash"), false), "{name}");
        }
        assert!(needs_chain(&ev("PreToolUse", "AskUserQuestion"), false));
        for name in ["PreToolUse", "PostToolUse", "SessionEnd", "TaskCompleted"] {
            assert!(!needs_chain(&ev(name, "Bash"), false), "{name}");
        }
    }

    #[test]
    fn freshly_spawned_island_always_gets_the_chain() {
        assert!(needs_chain(&json!({"hook_event_name": "PostToolUse", "tool_name": "Bash"}), true));
    }

    #[test]
    fn post_tool_use_sends_empty_chain() {
        let name = format!(r"\\.\pipe\ai-test-nochain-{}", std::process::id());
        let t = serve(&name, None);
        assert_eq!(run_with(&name, false, r#"{"hook_event_name":"PostToolUse","tool_name":"Bash"}"#), None);
        let sent: serde_json::Value = serde_json::from_str(&t.join().unwrap()).unwrap();
        assert_eq!(sent["chain"], json!([]));
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

    #[test]
    fn same_exe_true_for_byte_identical_copy() {
        let exe = std::env::current_exe().unwrap();
        let copy = std::env::temp_dir().join(format!("ai-same-exe-{}.exe", std::process::id()));
        std::fs::copy(&exe, &copy).unwrap();
        let r = same_exe(&exe, &copy);
        let _ = std::fs::remove_file(&copy);
        assert!(r);
    }

    #[test]
    fn same_exe_false_for_different_content() {
        let exe = std::env::current_exe().unwrap();
        assert!(!same_exe(&exe, Path::new(r"C:\Windows\notepad.exe")));
    }

    #[test]
    fn same_exe_false_when_unreadable() {
        let exe = std::env::current_exe().unwrap();
        assert!(!same_exe(&exe, Path::new(r"C:\no\such\agent-islands.exe")));
        assert!(!same_exe(Path::new(r"C:\no\such\a.exe"), Path::new(r"C:\no\such\a.exe")));
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

    const GATED: &str = r#"{"hook_event_name":"PreToolUse","tool_name":"PowerShell","tool_input":{"command":"git push"}}"#;
    fn pre_deny(reason: &str) -> String {
        format!(r#"{{"hookSpecificOutput":{{"hookEventName":"PreToolUse","permissionDecision":"deny","permissionDecisionReason":"{reason}"}}}}"#)
    }

    #[test]
    fn gated_allow_verified_prints_pretool_allow() {
        let want = r#"{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"allow"}}"#;
        assert_eq!(pretool_output(Some(Reply::Allow), true, false), want);
        let name = format!(r"\\.\pipe\ai-test-gallow-{}", std::process::id());
        let t = serve(&name, Some("{\"decision\":\"allow\"}\n"));
        assert_eq!(run_with(&name, false, GATED).as_deref(), Some(want));
        let sent: serde_json::Value = serde_json::from_str(&t.join().unwrap()).unwrap();
        assert_eq!(sent["gated"], true);
    }

    #[test]
    fn gated_deny_prints_reason() {
        assert_eq!(pretool_output(Some(Reply::Deny), true, false), pre_deny("Denied from Agent Islands"));
        let name = format!(r"\\.\pipe\ai-test-gdeny-{}", std::process::id());
        let t = serve(&name, Some("{\"decision\":\"deny\"}\n"));
        assert_eq!(run_with(&name, false, GATED), Some(pre_deny("Denied from Agent Islands")));
        t.join().unwrap();
    }

    #[test]
    fn gated_timeout_prints_deny_with_timeout_reason() {
        let want = pre_deny("No answer in Agent Islands within 30 seconds");
        assert_eq!(pretool_output(None, true, true), want);
        let name = format!(r"\\.\pipe\ai-test-gtimeout-{}", std::process::id());
        let t = serve(&name, Some("{\"decision\":\"timeout\"}\n"));
        assert_eq!(run_with(&name, false, GATED), Some(want));
        t.join().unwrap();
    }

    #[test]
    fn gated_skip_prints_nothing() {
        let name = format!(r"\\.\pipe\ai-test-gskip-{}", std::process::id());
        let t = serve(&name, Some("{\"decision\":\"skip\"}\n"));
        assert_eq!(run_with(&name, false, GATED).as_deref(), Some(""));
        t.join().unwrap();
    }

    #[test]
    fn gated_unverified_or_unreachable_prints_deny_not_running() {
        let want = pre_deny("Agent Islands is not running");
        assert_eq!(pretool_output(Some(Reply::Allow), false, false), want);
        assert_eq!(pretool_output(Some(Reply::Deny), false, false), want);
        assert_eq!(pretool_output(None, false, true), want);
        assert_eq!(pretool_output(None, true, false), want, "connection broke without a reply");
        let name = format!(r"\\.\pipe\ai-test-gnobody-{}", std::process::id());
        assert_eq!(run_with(&name, false, GATED), Some(want.clone()));
        let name = format!(r"\\.\pipe\ai-test-gbroke-{}", std::process::id());
        let t = serve(&name, None);
        assert_eq!(run_with(&name, false, GATED), Some(want));
        t.join().unwrap();
    }

    #[test]
    fn ungated_pretool_prints_nothing_and_does_not_wait() {
        let name = format!(r"\\.\pipe\ai-test-ungated-{}", std::process::id());
        let mut server = Server::create_first_named(&name).unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        // Holds the connection open after reading, so a waiting hook would block.
        let t = std::thread::spawn(move || {
            let f = server.accept().unwrap();
            let mut line = String::new();
            BufReader::new(&f).read_line(&mut line).unwrap();
            let _ = rx.recv_timeout(Duration::from_secs(5));
            line
        });
        let start = Instant::now();
        let input = r#"{"hook_event_name":"PreToolUse","tool_name":"PowerShell","tool_input":{"command":"git status"}}"#;
        assert_eq!(run_with(&name, false, input), None);
        assert!(start.elapsed() < Duration::from_secs(2));
        tx.send(()).unwrap();
        let sent: serde_json::Value = serde_json::from_str(&t.join().unwrap()).unwrap();
        assert_eq!(sent.get("gated"), None);
    }

    #[test]
    fn island_runs_from_its_own_folder() {
        let exe = Path::new(r"C:\plugins\agent-islands\agent-islands.exe");
        let cmd = island_command(exe);
        assert_eq!(cmd.get_current_dir(), Some(Path::new(r"C:\plugins\agent-islands")));
        assert_eq!(cmd.get_program(), exe.as_os_str());
    }

    #[test]
    fn std_handles_are_made_non_inheritable() {
        use windows::Win32::Foundation::{GetHandleInformation, HANDLE_FLAG_INHERIT, SetHandleInformation};
        use windows::Win32::System::Console::{GetStdHandle, STD_ERROR_HANDLE};
        let h = unsafe { GetStdHandle(STD_ERROR_HANDLE) }.unwrap();
        unsafe { SetHandleInformation(h, HANDLE_FLAG_INHERIT.0, HANDLE_FLAG_INHERIT) }.unwrap();
        no_inherit_std_handles();
        let mut flags = 0u32;
        unsafe { GetHandleInformation(h, &mut flags) }.unwrap();
        assert_eq!(flags & HANDLE_FLAG_INHERIT.0, 0);
    }
}
