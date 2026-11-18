use crate::hook::connect_island;
use crate::{pipe, procs};
use serde_json::json;
use std::io::{self, BufRead, BufReader, Write};
use std::time::Duration;

/// One connection: sends the envelope, copies each received line to `out`. Ok when the island is
/// unreachable, unverified or hangs up (the caller reconnects); Err only when `out` fails (Claude Code is gone).
pub(crate) fn serve_lines(name: &str, sid: &str, cwd: &str, out: &mut impl Write, spawn: bool) -> io::Result<()> {
    // Replacing is left to the session's hooks.
    let Some((mut f, _)) = connect_island(name, spawn, false) else { return Ok(()) };
    let chain = procs::ancestors(std::process::id());
    let mut line = json!({"listen": sid, "cwd": cwd, "chain": chain}).to_string();
    line.push('\n');
    if f.write_all(line.as_bytes()).is_err() {
        return Ok(());
    }
    let mut lines = BufReader::new(f);
    let mut buf = String::new();
    while matches!(lines.read_line(&mut buf), Ok(n) if n > 0) {
        out.write_all(buf.as_bytes())?;
        out.flush()?;
        buf.clear();
    }
    Ok(())
}

pub fn run(session_id: &str, cwd: &str) -> i32 {
    let parent = procs::ancestors(std::process::id()).get(1).map(|p| p.0);
    run_named(&pipe::pipe_name(), session_id, cwd, parent)
}

/// Stops when `out` fails or, before each (re)connect, when the parent Claude Code process is gone:
/// an orphan must not respawn the island forever.
fn run_named(name: &str, session_id: &str, cwd: &str, parent: Option<u32>) -> i32 {
    while parent.is_none_or(procs::is_alive) && serve_lines(name, session_id, cwd, &mut io::stdout(), true).is_ok() {
        std::thread::sleep(Duration::from_secs(1));
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipe::Server;

    #[test]
    fn dead_parent_ends_listen_without_connecting() {
        let mut c = std::process::Command::new("cmd").args(["/c", "exit"]).spawn().unwrap();
        let dead = c.id();
        c.wait().unwrap();
        // spawn=true would start an island if it connected; a dead parent returns first.
        assert_eq!(run_named(r"\\.\pipe\ai-test-orphan-none", "s", "x", Some(dead)), 0);
    }

    #[test]
    fn listen_prints_lines_and_reconnects() {
        let name = format!(r"\\.\pipe\ai-test-listen-{}", std::process::id());
        let mut server = Server::create_first_named(&name).unwrap();
        let t = std::thread::spawn(move || {
            let mut envelopes = Vec::new();
            for i in 0..2 {
                let f = server.accept().unwrap();
                let mut line = String::new();
                BufReader::new(&f).read_line(&mut line).unwrap();
                envelopes.push(line);
                if i == 0 {
                    (&f).write_all(b"\"a\\nb\"\n").unwrap();
                }
            }
            envelopes
        });
        let mut out = Vec::new();
        serve_lines(&name, "s", "x", &mut out, false).unwrap();
        assert_eq!(String::from_utf8(out.clone()).unwrap(), "\"a\\nb\"\n");
        serve_lines(&name, "s", "x", &mut out, false).unwrap();
        let envelopes = t.join().unwrap();
        for e in &envelopes {
            let v: serde_json::Value = serde_json::from_str(e).unwrap();
            assert_eq!((v["listen"].as_str(), v["cwd"].as_str()), (Some("s"), Some("x")));
            assert!(v["chain"].is_array());
        }
        assert_eq!(out.len(), 7, "second connection added nothing");
    }
}
