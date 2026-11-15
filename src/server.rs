use crate::{hook::Reply, pipe::Server};
use serde_json::Value;
use std::io::{BufRead, BufReader, Write};
use std::sync::mpsc::{RecvTimeoutError, Sender, channel};
use std::time::Duration;

pub enum Msg {
    /// `reply` is Some only for a PermissionRequest that needs an answer; Core then sends the
    /// assigned req_id back on the second sender so the connection thread can report a timeout.
    Event {
        raw: Value,
        chain: Vec<(u32, String)>,
        reply: Option<(Sender<Reply>, Sender<u64>)>,
    },
    Timeout(u64),
}

/// Fails if another island already owns the pipe.
pub fn spawn(tx: Sender<Msg>, wake: impl Fn() + Send + Clone + 'static) -> std::io::Result<()> {
    let mut server = Server::create_first()?;
    std::thread::spawn(move || {
        loop {
            match server.accept() {
                Ok(f) => {
                    let (tx, wake) = (tx.clone(), wake.clone());
                    std::thread::spawn(move || {
                        let _ = serve(f, &tx, &wake);
                    });
                }
                Err(_) => std::thread::sleep(Duration::from_millis(50)),
            }
        }
    });
    Ok(())
}

/// Any failure (bad JSON, broken pipe, dead receiver) just ends this connection thread.
fn serve(f: std::fs::File, tx: &Sender<Msg>, wake: &impl Fn()) -> Option<()> {
    let mut line = String::new();
    BufReader::new(&f).read_line(&mut line).ok()?;
    let mut v: Value = serde_json::from_str(&line).ok()?;
    let mut raw = v["event"].take();
    let chain = serde_json::from_value(v["chain"].take()).unwrap_or_default();
    // The hook ran the command gate (spec 9a); only its envelope flag counts, never a field in the event.
    let gated = v["gated"] == true;
    if let Some(o) = raw.as_object_mut() {
        o.insert("gated".into(), gated.into());
    }
    let wants = gated || raw["hook_event_name"] == "PermissionRequest" && raw["tool_name"] != "AskUserQuestion";
    if !wants {
        tx.send(Msg::Event { raw, chain, reply: None }).ok()?;
        wake();
        return Some(());
    }
    let (rtx, rrx) = channel();
    let (qtx, qrx) = channel();
    tx.send(Msg::Event { raw, chain, reply: Some((rtx, qtx)) }).ok()?;
    wake();
    let id = qrx.recv().ok()?;
    match rrx.recv_timeout(Duration::from_secs(30)) {
        Ok(r) => {
            let d = if matches!(r, Reply::Allow) { "allow" } else { "deny" };
            (&f).write_all(format!("{{\"decision\":\"{d}\"}}\n").as_bytes()).ok()?;
        }
        // The card was dropped (the turn ended): a gated command is denied, a permission request gets nothing.
        Err(RecvTimeoutError::Disconnected) => {
            if gated {
                (&f).write_all(b"{\"decision\":\"deny\"}\n").ok()?;
            }
        }
        Err(RecvTimeoutError::Timeout) => {
            // A gated hook must tell a timeout from a dead island; a permission request gets nothing.
            if gated {
                (&f).write_all(b"{\"decision\":\"timeout\"}\n").ok()?;
            }
            tx.send(Msg::Timeout(id)).ok()?;
            wake();
        }
    }
    Some(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::Receiver;

    /// Sends one envelope line through a real pipe to `serve` and returns what it hands the core.
    fn send(tag: &str, envelope: Value) -> (Msg, std::fs::File) {
        let name = format!(r"\\.\pipe\ai-test-srv-{tag}-{}", std::process::id());
        let mut server = Server::create_first_named(&name).unwrap();
        let (tx, rx): (Sender<Msg>, Receiver<Msg>) = channel();
        std::thread::spawn(move || {
            let f = server.accept().unwrap();
            let _ = serve(f, &tx, &|| {});
        });
        let (mut c, _) = crate::pipe::connect_to(&name).unwrap();
        c.write_all(format!("{envelope}\n").as_bytes()).unwrap();
        (rx.recv_timeout(Duration::from_secs(3)).unwrap(), c)
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
}
