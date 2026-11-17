use crate::{hook::Reply, pipe::Server};
use serde_json::Value;
use std::io::{BufRead, BufReader, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{RecvTimeoutError, Sender, channel};
use std::time::Duration;

pub enum Msg {
    /// `reply` is Some only for a PermissionRequest that needs an answer; Core then sends the
    /// assigned req_id back on the second sender so the connection thread can report a timeout.
    Event {
        raw: Value,
        chain: Vec<(u32, String)>,
        reply: Option<(Sender<Reply>, Sender<u64>)>,
        /// Telegram bot token from the hook's plugin option; Some("") means off.
        token: Option<String>,
    },
    /// A `listen` helper: Core keeps `pipe` open and writes phone prompts to it.
    Listen { session_id: String, cwd: String, chain: Vec<(u32, String)>, pipe: std::fs::File },
    Timeout(u64),
    /// Todos rebuilt from a session's transcript by a worker thread.
    Replay { session_id: String, todos: Vec<crate::state::Todo> },
    /// From the Telegram poller.
    Phone(crate::telegram::Incoming),
    /// A session's message was sent to Telegram as `message_id`.
    Sent { message_id: i64, session_id: String },
}

/// Island wait for a card: normal, and while away.
const WAIT: (Duration, Duration) = (Duration::from_secs(30), Duration::from_secs(600));

/// Fails if another island already owns the pipe. `away` makes cards wait 600 s and holds questions.
pub fn spawn(tx: Sender<Msg>, wake: impl Fn() + Send + Clone + 'static, away: Arc<AtomicBool>) -> std::io::Result<()> {
    let mut server = Server::create_first()?;
    std::thread::spawn(move || {
        loop {
            match server.accept() {
                Ok(f) => {
                    let (tx, wake, away) = (tx.clone(), wake.clone(), away.clone());
                    std::thread::spawn(move || {
                        let _ = serve(f, &tx, &wake, &away, WAIT);
                    });
                }
                Err(_) => std::thread::sleep(Duration::from_millis(50)),
            }
        }
    });
    Ok(())
}

/// Any failure (bad JSON, broken pipe, dead receiver) just ends this connection thread.
/// `wait` is (normal, away) so tests can shrink the windows.
fn serve(f: std::fs::File, tx: &Sender<Msg>, wake: &impl Fn(), away: &AtomicBool, wait: (Duration, Duration)) -> Option<()> {
    let mut line = String::new();
    BufReader::new(&f).read_line(&mut line).ok()?;
    let mut v: Value = serde_json::from_str(&line).ok()?;
    let chain = serde_json::from_value(v["chain"].take()).unwrap_or_default();
    if let Some(session_id) = v["listen"].as_str() {
        if session_id.is_empty() {
            return None;
        }
        let cwd = v["cwd"].as_str().unwrap_or_default().to_owned();
        tx.send(Msg::Listen { session_id: session_id.to_owned(), cwd, chain, pipe: f }).ok()?;
        wake();
        return Some(());
    }
    let token = v["token"].as_str().map(str::to_owned);
    let mut raw = v["event"].take();
    // The hook ran the command gate (spec 9a); only its envelope flag counts, never a field in the event.
    let gated = v["gated"] == true;
    let is_away = away.load(Ordering::Relaxed);
    if let Some(o) = raw.as_object_mut() {
        o.insert("gated".into(), gated.into());
    }
    // A question is held only while away; otherwise the hook stays silent and the dialog opens at once.
    let ask = v["ask"] == true;
    let held_ask = ask && is_away;
    if ask && !is_away {
        (&f).write_all(b"{\"decision\":\"skip\"}\n").ok()?;
    }
    let wants = gated || held_ask || raw["hook_event_name"] == "PermissionRequest" && raw["tool_name"] != "AskUserQuestion";
    if !wants {
        tx.send(Msg::Event { raw, chain, reply: None, token }).ok()?;
        wake();
        return Some(());
    }
    let (rtx, rrx) = channel();
    let (qtx, qrx) = channel();
    tx.send(Msg::Event { raw, chain, reply: Some((rtx, qtx)), token }).ok()?;
    wake();
    // A held question the core does not take (it drops the senders): release the hook so the dialog opens.
    let Ok(id) = qrx.recv() else {
        if held_ask {
            (&f).write_all(b"{\"decision\":\"skip\"}\n").ok()?;
        }
        return None;
    };
    let mut window = if is_away { wait.1 } else { wait.0 };
    loop {
        let line = match rrx.recv_timeout(window) {
            // Away turned off: a waiting card gets a fresh normal window.
            Ok(Reply::Rearm) if !held_ask => {
                window = wait.0;
                continue;
            }
            Ok(Reply::Answer(answers)) if held_ask => serde_json::json!({"decision": "answer", "answers": answers}).to_string(),
            // Anything else to a held question just lets the dialog open.
            Ok(_) if held_ask => r#"{"decision":"skip"}"#.to_owned(),
            Ok(Reply::Allow) => r#"{"decision":"allow"}"#.to_owned(),
            Ok(_) => r#"{"decision":"deny"}"#.to_owned(),
            // The card was dropped (the turn ended): a gated command is denied, a permission request gets nothing.
            Err(RecvTimeoutError::Disconnected) if held_ask => r#"{"decision":"skip"}"#.to_owned(),
            Err(RecvTimeoutError::Disconnected) if gated => r#"{"decision":"deny"}"#.to_owned(),
            Err(RecvTimeoutError::Disconnected) => return Some(()),
            Err(RecvTimeoutError::Timeout) => {
                // A gated hook must tell a timeout from a dead island; a permission request or question gets nothing.
                if gated {
                    (&f).write_all(b"{\"decision\":\"timeout\"}\n").ok()?;
                }
                tx.send(Msg::Timeout(id)).ok()?;
                wake();
                return Some(());
            }
        };
        (&f).write_all(format!("{line}\n").as_bytes()).ok()?;
        return Some(());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::Receiver;

    /// Sends one envelope line through a real pipe to `serve` and returns what it hands the core.
    fn send(tag: &str, envelope: Value) -> (Msg, std::fs::File) {
        send_with(tag, envelope, false, WAIT)
    }

    fn send_with(tag: &str, envelope: Value, away: bool, wait: (Duration, Duration)) -> (Msg, std::fs::File) {
        let name = format!(r"\\.\pipe\ai-test-srv-{tag}-{}", std::process::id());
        let mut server = Server::create_first_named(&name).unwrap();
        let (tx, rx): (Sender<Msg>, Receiver<Msg>) = channel();
        std::thread::spawn(move || {
            let f = server.accept().unwrap();
            let _ = serve(f, &tx, &|| {}, &AtomicBool::new(away), wait);
        });
        let (mut c, _) = crate::pipe::connect_to(&name).unwrap();
        c.write_all(format!("{envelope}\n").as_bytes()).unwrap();
        (rx.recv_timeout(Duration::from_secs(3)).unwrap(), c)
    }

    #[test]
    fn listen_envelope_hands_pipe_to_core() {
        let (msg, _c) = send("listen", serde_json::json!({"listen": "s", "cwd": "x", "chain": []}));
        let Msg::Listen { session_id, cwd, .. } = msg else { panic!() };
        assert_eq!((session_id.as_str(), cwd.as_str()), ("s", "x"));
    }

    #[test]
    fn listen_envelope_with_empty_id_is_ignored() {
        let name = format!(r"\\.\pipe\ai-test-srv-emptyid-{}", std::process::id());
        let mut server = Server::create_first_named(&name).unwrap();
        let (tx, rx): (Sender<Msg>, Receiver<Msg>) = channel();
        std::thread::spawn(move || {
            let f = server.accept().unwrap();
            let _ = serve(f, &tx, &|| {}, &AtomicBool::new(false), WAIT);
        });
        let (mut c, _) = crate::pipe::connect_to(&name).unwrap();
        c.write_all(b"{\"listen\": \"\", \"cwd\": \"x\", \"chain\": []}\n").unwrap();
        assert!(rx.recv_timeout(Duration::from_millis(500)).is_err());
    }

    #[test]
    fn token_is_passed_to_core() {
        let ev = serde_json::json!({"hook_event_name": "PostToolUse"});
        let (msg, _c) = send("tok", serde_json::json!({"event": ev, "chain": [], "token": "t:1"}));
        let Msg::Event { token, .. } = msg else { panic!() };
        assert_eq!(token.as_deref(), Some("t:1"));
        let (msg, _c) = send("notok", serde_json::json!({"event": ev, "chain": []}));
        let Msg::Event { token, .. } = msg else { panic!() };
        assert_eq!(token, None);
    }

    #[test]
    fn gated_envelope_waits_for_reply_and_marks_event() {
        let ev = serde_json::json!({"hook_event_name": "PreToolUse", "tool_name": "PowerShell"});
        let (msg, c) = send("gated", serde_json::json!({"event": ev, "chain": [], "gated": true}));
        let Msg::Event { raw, reply, .. } = msg else { panic!() };
        assert_eq!(raw["gated"], true);
        let (rtx, qtx) = reply.expect("gated request must wait for a reply");
        qtx.send(1).unwrap();
        rtx.send(Reply::Deny).unwrap();
        let mut line = String::new();
        BufReader::new(&c).read_line(&mut line).unwrap();
        assert_eq!(line, "{\"decision\":\"deny\"}\n");
    }

    fn read_all(c: std::fs::File) -> String {
        let mut s = String::new();
        let _ = std::io::Read::read_to_string(&mut &c, &mut s);
        s
    }

    #[test]
    fn dropped_reply_denies_gated_at_once() {
        let ev = serde_json::json!({"hook_event_name": "PreToolUse", "tool_name": "PowerShell"});
        let (msg, c) = send("gdrop", serde_json::json!({"event": ev, "chain": [], "gated": true}));
        let Msg::Event { reply, .. } = msg else { panic!() };
        let (rtx, qtx) = reply.unwrap();
        qtx.send(1).unwrap();
        let start = std::time::Instant::now();
        drop(rtx);
        assert_eq!(read_all(c), "{\"decision\":\"deny\"}\n");
        assert!(start.elapsed() < Duration::from_secs(3));
    }

    #[test]
    fn dropped_reply_ends_permission_request_with_nothing() {
        let ev = serde_json::json!({"hook_event_name": "PermissionRequest", "tool_name": "Bash"});
        let (msg, c) = send("pdrop", serde_json::json!({"event": ev, "chain": []}));
        let Msg::Event { reply, .. } = msg else { panic!() };
        let (rtx, qtx) = reply.unwrap();
        qtx.send(1).unwrap();
        drop(rtx);
        assert_eq!(read_all(c), "");
    }

    #[test]
    fn ungated_pretooluse_does_not_wait() {
        let ev = serde_json::json!({"hook_event_name": "PreToolUse", "tool_name": "PowerShell", "gated": true});
        let (msg, _c) = send("ungated", serde_json::json!({"event": ev, "chain": []}));
        let Msg::Event { raw, reply, .. } = msg else { panic!() };
        assert!(reply.is_none());
        assert_ne!(raw["gated"], true, "only the hook's envelope flag may gate");
    }

    #[test]
    fn gated_never_skips_when_away_off() {
        let ev = serde_json::json!({"hook_event_name": "PreToolUse", "tool_name": "PowerShell"});
        let (msg, _c) = send("gnoskip", serde_json::json!({"event": ev, "chain": [], "gated": true}));
        let Msg::Event { raw, reply, .. } = msg else { panic!() };
        assert!(reply.is_some());
        assert_eq!(raw["gated"], true);
    }

    fn ask_envelope() -> Value {
        serde_json::json!({"event": {"hook_event_name": "PreToolUse", "tool_name": "AskUserQuestion"}, "chain": [], "ask": true})
    }

    #[test]
    fn ask_not_away_skips_and_does_not_wait() {
        let (msg, c) = send_with("asknot", ask_envelope(), false, WAIT);
        let Msg::Event { reply, .. } = msg else { panic!() };
        assert!(reply.is_none());
        assert_eq!(read_all(c), "{\"decision\":\"skip\"}\n");
    }

    #[test]
    fn ask_away_waits_and_writes_answers() {
        let (msg, c) = send_with("askaway", ask_envelope(), true, WAIT);
        let Msg::Event { reply, .. } = msg else { panic!() };
        let (rtx, qtx) = reply.expect("a held question waits");
        qtx.send(1).unwrap();
        rtx.send(Reply::Answer(serde_json::json!({"Q?": "B"}))).unwrap();
        let got: Value = serde_json::from_str(&read_all(c)).unwrap();
        assert_eq!(got, serde_json::json!({"decision": "answer", "answers": {"Q?": "B"}}));
    }

    #[test]
    fn untaken_held_question_writes_skip() {
        let (msg, c) = send_with("askdrop", ask_envelope(), true, WAIT);
        let Msg::Event { reply, .. } = msg else { panic!() };
        drop(reply); // the core never takes it
        assert_eq!(read_all(c), "{\"decision\":\"skip\"}\n");
    }

    #[test]
    fn rearm_restarts_with_30s() {
        // Normal window 1.5 s, away window 60 s: after Rearm the card times out on the short one.
        let wait = (Duration::from_millis(1500), Duration::from_secs(60));
        let ev = serde_json::json!({"hook_event_name": "PreToolUse", "tool_name": "PowerShell"});
        let (msg, c) = send_with("rearm", serde_json::json!({"event": ev, "chain": [], "gated": true}), true, wait);
        let Msg::Event { reply, .. } = msg else { panic!() };
        let (rtx, qtx) = reply.unwrap();
        qtx.send(1).unwrap();
        rtx.send(Reply::Rearm).unwrap();
        let h = std::thread::spawn(move || read_all(c));
        std::thread::sleep(Duration::from_secs(1));
        assert!(!h.is_finished(), "no reply within 1 s of the rearm");
        assert_eq!(h.join().unwrap(), "{\"decision\":\"timeout\"}\n");
    }
}
