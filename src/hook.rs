use crate::{detail, gate, pipe, procs};
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
pub(crate) const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;

pub enum Reply {
    Allow,
    Deny,
    /// Away turned off: the waiting card gets a fresh 30 s window. Never printed by a hook.
    Rearm,
    /// A held AskUserQuestion's answers map, keyed by question text.
    Answer(Value),
}

/// The island was verified by `connect_island` before any reply reaches the output functions.
pub fn decision_output(reply: Option<Reply>) -> Option<String> {
    let decision = match reply? {
        Reply::Allow => r#"{"behavior":"allow"}"#,
        Reply::Deny | Reply::Rearm | Reply::Answer(_) => r#"{"behavior":"deny","message":"Denied from Agent Islands"}"#,
    };
    Some(format!(
        r#"{{"hookSpecificOutput":{{"hookEventName":"PermissionRequest","decision":{decision}}}}}"#
    ))
}

/// Gated PreToolUse (spec 9a): always prints a decision; allow only with a verified island and a click.
pub fn pretool_output(reply: Option<Reply>, timed_out: bool) -> String {
    let deny = |why: &str| format!(r#""deny","permissionDecisionReason":"{why}""#);
    let decision = match reply {
        Some(Reply::Allow) => r#""allow""#.to_owned(),
        Some(Reply::Deny | Reply::Rearm | Reply::Answer(_)) => deny("Denied from Agent Islands"),
        None if timed_out => deny("No answer in Agent Islands in time"),
        None => deny("Agent Islands is not running"),
    };
    format!(r#"{{"hookSpecificOutput":{{"hookEventName":"PreToolUse","permissionDecision":{decision}}}}}"#)
}

/// Allow on a gated command that the card showed only in part: Claude Code's own prompt shows all of it.
const PRE_ASK: &str = r#"{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"ask","permissionDecisionReason":"Agent Islands showed only part of this command"}}"#;

/// The card's detail line is the whole command (one line, not truncated).
fn shown_whole(event: &Value) -> bool {
    let (tool, input) = (event["tool_name"].as_str().unwrap_or_default(), &event["tool_input"]);
    detail::detail_line(tool, input, None).1 == detail::full_line(tool, input, None).1
}

fn gated_output(d: &Option<Value>, event: &Value) -> String {
    match to_reply(d) {
        Some(Reply::Allow) if !shown_whole(event) => PRE_ASK.to_owned(),
        r => pretool_output(r, decision(d) == Some("timeout")),
    }
}

/// A held AskUserQuestion: the island's answers skip the dialog; anything else prints nothing.
fn ask_output(reply: Option<Reply>, event: &Value) -> Option<String> {
    let Some(Reply::Answer(answers)) = reply else { return None };
    let out = json!({"hookSpecificOutput": {"hookEventName": "PreToolUse", "permissionDecision": "allow",
        "updatedInput": {"questions": event["tool_input"]["questions"], "answers": answers}}});
    Some(out.to_string())
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

/// What a hook may do with the island that owns the pipe.
#[derive(Debug, PartialEq)]
pub(crate) enum Island {
    /// This build, or a newer install: talk to it.
    Usable,
    /// An older install: a newer hook replaces it.
    Older,
    /// Anything else: never talk to it, never touch it.
    Foreign,
}

/// `<root>\<x.y.z>\bin\agent-islands.exe` as (lowercased root, version). Path only: cleanup may have deleted the folder.
pub(crate) fn install(exe: &Path) -> Option<(String, [u32; 3])> {
    let bin = exe.parent()?;
    let version_dir = bin.parent()?;
    if !exe.file_name()?.eq_ignore_ascii_case("agent-islands.exe") || !bin.file_name()?.eq_ignore_ascii_case("bin") {
        return None;
    }
    let parts: Vec<u32> = version_dir.file_name()?.to_str()?.split('.').map(|n| n.parse().ok()).collect::<Option<_>>()?;
    Some((version_dir.parent()?.to_string_lossy().to_lowercase(), parts.try_into().ok()?))
}

pub(crate) fn classify(server: &Path, me: &Path) -> Island {
    if same_exe(server, me) {
        return Island::Usable;
    }
    match (install(server), install(me)) {
        (Some((a, v)), Some((b, mine))) if a == b && v > mine => Island::Usable,
        (Some((a, v)), Some((b, mine))) if a == b && v < mine => Island::Older,
        _ => Island::Foreign,
    }
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

/// A connection to an island this exe may talk to, and whether this call started that island.
/// With `replace`, an older install's island is stopped and this exe's started instead. Two rounds,
/// because an old `listen` helper can restart the old island in the gap; at most 4 s in all.
pub(crate) fn connect_island(name: &str, spawn: bool, replace: bool) -> Option<(std::fs::File, bool)> {
    let me = std::env::current_exe().ok()?;
    let start = Instant::now();
    let mut replaced = 0;
    loop {
        let (f, pid, spawned) = connect_retry(name, spawn)?;
        let server = procs::exe_path(pid)?;
        match classify(&server, &me) {
            Island::Usable => return Some((f, spawned || replaced > 0)),
            Island::Older if replace && replaced < 2 && start.elapsed() < Duration::from_millis(1500) => {
                drop(f);
                procs::terminate_exact(pid, &server);
                replaced += 1;
            }
            _ => return None,
        }
    }
}

/// The island's reply line: `decision` is "allow", "deny", "answer", or "timeout" / "skip".
fn read_reply(f: std::fs::File, wait: Duration) -> Option<Value> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut line = String::new();
        let _ = BufReader::new(f).read_line(&mut line);
        let _ = tx.send(line);
    });
    let line = rx.recv_timeout(wait).ok()?;
    serde_json::from_str(&line).ok()
}

const STE: &str = "The user reads this reply on a phone. Write in ASD-STE100 Simplified Technical English. First line: the result. Then: what you need from the user, if anything. Then details. One idea per sentence. Max 20 words per sentence. Active voice. No filler, no pleasantries. Keep code, paths, commands and error text exact.";

/// UserPromptSubmit stdout: the STE instruction, only when the island says Away is on.
pub fn prompt_output(reply: &Option<Value>) -> Option<String> {
    if reply.as_ref()?["away"] != true {
        return None;
    }
    Some(json!({"hookSpecificOutput": {"hookEventName": "UserPromptSubmit", "additionalContext": STE}}).to_string())
}

fn decision(v: &Option<Value>) -> Option<&str> {
    v.as_ref()?["decision"].as_str()
}

fn to_reply(v: &Option<Value>) -> Option<Reply> {
    match decision(v)? {
        "allow" => Some(Reply::Allow),
        "deny" => Some(Reply::Deny),
        "answer" => Some(Reply::Answer(v.as_ref()?["answers"].clone())),
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
    let mut event: Value = serde_json::from_str(input).ok()?;
    let gated = event["hook_event_name"] == "PreToolUse"
        && gate::is_gated(event["tool_name"].as_str().unwrap_or_default(), event["tool_input"]["command"].as_str().unwrap_or_default());
    trim(&mut event, gated);
    let token = std::env::var("CLAUDE_PLUGIN_OPTION_TELEGRAM_BOT_TOKEN").ok();
    let out = forward_with_token(name, spawn, event, gated, token.as_deref());
    if gated { Some(out.unwrap_or_else(|| pretool_output(None, false))) } else { out }
}

/// The island reads `tool_response` only for TaskCreate and `tool_input` only for todos, questions
/// and cards, so a big Read result or Write content does not cross the pipe.
fn trim(event: &mut Value, gated: bool) {
    let tool = event["tool_name"].as_str().unwrap_or_default();
    let keep_response = tool == "TaskCreate";
    let keep_input = gated
        || matches!(tool, "TaskCreate" | "TaskUpdate" | "TodoWrite" | "AskUserQuestion")
        || event["hook_event_name"] == "PermissionRequest";
    let Some(o) = event.as_object_mut() else { return };
    if !keep_response {
        o.remove("tool_response");
    }
    if !keep_input {
        o.remove("tool_input");
    }
}

/// `token`: the Telegram bot token option; Some("") means the user turned the phone link off.
fn forward_with_token(name: &str, spawn: bool, event: Value, gated: bool, token: Option<&str>) -> Option<String> {
    let (mut f, spawned) = connect_island(name, spawn, spawn)?;
    let ask = event["hook_event_name"] == "PreToolUse" && event["tool_name"] == "AskUserQuestion";
    let wants_reply = event["hook_event_name"] == "PermissionRequest"
        && event["tool_name"] != "AskUserQuestion";
    let chain = if gated || needs_chain(&event, spawned) { procs::ancestors(std::process::id()) } else { Vec::new() };
    let mut msg = json!({"event": event, "chain": chain});
    if gated {
        msg["gated"] = json!(true);
    }
    if ask {
        msg["ask"] = json!(true);
    }
    if let Some(t) = token {
        msg["token"] = json!(t);
    }
    let mut line = msg.to_string();
    line.push('\n');
    f.write_all(line.as_bytes()).ok()?;
    if event["hook_event_name"] == "UserPromptSubmit" {
        // A prompt must never be blocked: short wait, silent on any failure.
        return prompt_output(&read_reply(f, Duration::from_millis(300)));
    }
    let wait = Duration::from_secs(605);
    if gated {
        return Some(gated_output(&read_reply(f, wait), &event));
    }
    if ask {
        return ask_output(to_reply(&read_reply(f, wait)), &event);
    }
    if !wants_reply {
        return None;
    }
    decision_output(to_reply(&read_reply(f, wait)))
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
    fn trim_keeps_only_fields_the_island_reads() {
        let t = |name: &str, tool: &str, gated: bool| {
            let mut e = json!({"hook_event_name": name, "tool_name": tool, "tool_input": {"a": 1}, "tool_response": {"b": 2}});
            trim(&mut e, gated);
            (e.get("tool_input").is_some(), e.get("tool_response").is_some())
        };
        assert_eq!(t("PostToolUse", "Read", false), (false, false));
        assert_eq!(t("PreToolUse", "Write", false), (false, false));
        assert_eq!(t("PostToolUse", "TaskCreate", false), (true, true));
        assert_eq!(t("PostToolUse", "TaskUpdate", false), (true, false));
        assert_eq!(t("PostToolUse", "TodoWrite", false), (true, false));
        assert_eq!(t("PreToolUse", "AskUserQuestion", false), (true, false));
        assert_eq!(t("PermissionRequest", "Write", false), (true, false));
        assert_eq!(t("PreToolUse", "Bash", true), (true, false));
    }

    #[test]
    fn allow_verified_prints_allow_json() {
        assert_eq!(decision_output(Some(Reply::Allow)).as_deref(), Some(ALLOW));
    }

    #[test]
    fn deny_verified_prints_deny_with_message() {
        let want = r#"{"hookSpecificOutput":{"hookEventName":"PermissionRequest","decision":{"behavior":"deny","message":"Denied from Agent Islands"}}}"#;
        assert_eq!(decision_output(Some(Reply::Deny)).as_deref(), Some(want));
    }

    #[test]
    fn no_reply_prints_nothing() {
        assert_eq!(decision_output(None), None);
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
    fn token_rides_in_envelope_when_set() {
        let ev = json!({"hook_event_name":"PostToolUse","tool_name":"Bash"});
        for (tag, token) in [("set", Some("")), ("unset", None)] {
            let name = format!(r"\\.\pipe\ai-test-token-{tag}-{}", std::process::id());
            let t = serve(&name, Some("{\"decision\":\"deny\"}\n"));
            // gated: the hook waits for the reply, so the pipe stays open until the server read the line
            assert!(forward_with_token(&name, false, ev.clone(), true, token).is_some());
            let sent: serde_json::Value = serde_json::from_str(&t.join().unwrap()).unwrap();
            assert_eq!(sent.get("token").and_then(|v| v.as_str()), token);
        }
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
    fn prompt_output_only_when_away() {
        let out = prompt_output(&Some(json!({"away": true}))).unwrap();
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["hookSpecificOutput"]["hookEventName"], "UserPromptSubmit");
        assert_eq!(v["hookSpecificOutput"]["additionalContext"], STE);
        for r in [Some(json!({"away": false})), None, Some(json!({}))] {
            assert_eq!(prompt_output(&r), None);
        }
    }

    #[test]
    fn prompt_reply_wait_is_short() {
        let name = format!(r"\\.\pipe\ai-test-promptwait-{}", std::process::id());
        let t = serve(&name, None);
        let (f, _) = crate::pipe::connect_to(&name).unwrap();
        let start = Instant::now();
        assert_eq!(read_reply(f, Duration::from_millis(300)), None);
        assert!(start.elapsed() < Duration::from_secs(1));
        drop(t);
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

    #[test]
    fn classify_compares_sibling_versions() {
        let p = |v: &str| std::path::PathBuf::from(format!(r"C:\c\agent-islands\{v}\bin\agent-islands.exe"));
        assert_eq!(classify(&p("0.1.9"), &p("0.1.10")), Island::Older);
        assert_eq!(classify(&p("0.1.10"), &p("0.1.9")), Island::Usable);
        assert_eq!(classify(&p("0.2.0"), &p("0.2.0")), Island::Foreign, "equal version, not provably the same file");
        assert_eq!(classify(&p("0.2.1-beta"), &p("0.2.2")), Island::Foreign);
        assert_eq!(classify(&p("0.2"), &p("0.2.2")), Island::Foreign);
        let upper = std::path::PathBuf::from(r"C:\C\AGENT-ISLANDS\0.1.9\BIN\AGENT-ISLANDS.EXE");
        assert_eq!(classify(&upper, &p("0.2.0")), Island::Older);
    }

    #[test]
    fn classify_accepts_the_same_exe_anywhere() {
        let exe = std::env::current_exe().unwrap();
        assert_eq!(classify(&exe, &exe), Island::Usable);
    }

    #[test]
    fn classify_rejects_anything_outside_the_install_folder() {
        let me = Path::new(r"C:\c\agent-islands\0.2.1\bin\agent-islands.exe");
        for other in [
            r"C:\Windows\notepad.exe",
            r"C:\c\agent-islands\0.1.0\bin\other.exe",
            r"C:\c\agent-islands\0.1.0\agent-islands.exe",
            r"C:\elsewhere\agent-islands\0.1.0\bin\agent-islands.exe",
        ] {
            assert_eq!(classify(Path::new(other), me), Island::Foreign, "{other}");
        }
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
        assert_eq!(pretool_output(Some(Reply::Allow), false), want);
        let name = format!(r"\\.\pipe\ai-test-gallow-{}", std::process::id());
        let t = serve(&name, Some("{\"decision\":\"allow\"}\n"));
        assert_eq!(run_with(&name, false, GATED).as_deref(), Some(want));
        let sent: serde_json::Value = serde_json::from_str(&t.join().unwrap()).unwrap();
        assert_eq!(sent["gated"], true);
    }

    #[test]
    fn gated_allow_of_partly_shown_command_asks() {
        let allow = Some(json!({"decision": "allow"}));
        let ev = |c: &str| json!({"tool_name": "Bash", "tool_input": {"command": c}});
        assert_eq!(gated_output(&allow, &ev("git push\nrm -rf ~")), PRE_ASK);
        assert_eq!(gated_output(&allow, &ev(&format!("git push {}", "x".repeat(80)))), PRE_ASK);
        assert!(gated_output(&allow, &ev("git push")).contains(r#""allow""#));
        assert!(gated_output(&Some(json!({"decision": "deny"})), &ev("git push\nx")).contains(r#""deny""#));
    }

    #[test]
    fn gated_deny_prints_reason() {
        assert_eq!(pretool_output(Some(Reply::Deny), false), pre_deny("Denied from Agent Islands"));
        let name = format!(r"\\.\pipe\ai-test-gdeny-{}", std::process::id());
        let t = serve(&name, Some("{\"decision\":\"deny\"}\n"));
        assert_eq!(run_with(&name, false, GATED), Some(pre_deny("Denied from Agent Islands")));
        t.join().unwrap();
    }

    #[test]
    fn gated_timeout_reason_is_in_time() {
        assert!(pretool_output(None, true).contains("No answer in Agent Islands in time"));
        let want = pre_deny("No answer in Agent Islands in time");
        assert_eq!(pretool_output(None, true), want);
        let name = format!(r"\\.\pipe\ai-test-gtimeout-{}", std::process::id());
        let t = serve(&name, Some("{\"decision\":\"timeout\"}\n"));
        assert_eq!(run_with(&name, false, GATED), Some(want));
        t.join().unwrap();
    }

    const ASK: &str = r#"{"hook_event_name":"PreToolUse","tool_name":"AskUserQuestion","tool_input":{"questions":[{"question":"Q?","options":[{"label":"A"},{"label":"B"}]}]}}"#;

    #[test]
    fn ask_answer_prints_updated_input() {
        let name = format!(r"\\.\pipe\ai-test-askans-{}", std::process::id());
        let t = serve(&name, Some("{\"decision\":\"answer\",\"answers\":{\"Q?\":\"B\"}}\n"));
        let out = run_with(&name, false, ASK).unwrap();
        let input: Value = serde_json::from_str(ASK).unwrap();
        let want = json!({"hookSpecificOutput": {"hookEventName": "PreToolUse", "permissionDecision": "allow",
            "updatedInput": {"questions": input["tool_input"]["questions"], "answers": {"Q?": "B"}}}});
        assert_eq!(serde_json::from_str::<Value>(&out).unwrap(), want);
        let sent: Value = serde_json::from_str(&t.join().unwrap()).unwrap();
        assert_eq!(sent["ask"], true);
    }

    #[test]
    fn ask_skip_prints_nothing() {
        let name = format!(r"\\.\pipe\ai-test-askskip-{}", std::process::id());
        let t = serve(&name, Some("{\"decision\":\"skip\"}\n"));
        assert_eq!(run_with(&name, false, ASK), None);
        t.join().unwrap();
    }

    #[test]
    fn gated_unverified_or_unreachable_prints_deny_not_running() {
        let want = pre_deny("Agent Islands is not running");
        assert_eq!(pretool_output(None, false), want, "connection broke without a reply");
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
