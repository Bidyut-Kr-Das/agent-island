use crate::{hook::Reply, pipe::Server};
use serde_json::Value;
use std::io::{BufRead, BufReader, Write};
use std::sync::mpsc::{Sender, channel};
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
    let raw = v["event"].take();
    let chain = serde_json::from_value(v["chain"].take()).unwrap_or_default();
    let wants = raw["hook_event_name"] == "PermissionRequest" && raw["tool_name"] != "AskUserQuestion";
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
        Err(_) => {
            tx.send(Msg::Timeout(id)).ok()?;
            wake();
        }
    }
    Some(())
}
