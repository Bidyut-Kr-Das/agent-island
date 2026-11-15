use crate::server::Msg;
use crate::{event, hook::Reply, procs, state::State};
use std::collections::HashMap;
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};

pub struct Core {
    pub state: State,
    pub replies: HashMap<u64, Sender<Reply>>,
    empty_since: Option<Instant>,
    last_prune: Option<Instant>,
}

const PRUNE_EVERY: Duration = Duration::from_secs(3);
const EXIT_AFTER_EMPTY: Duration = Duration::from_secs(3);

impl Core {
    pub fn new() -> Core {
        Core { state: State::default(), replies: HashMap::new(), empty_since: None, last_prune: None }
    }

    pub fn handle(&mut self, msg: Msg, now: u64) {
        match msg {
            Msg::Event { raw, chain, reply } => {
                let Some(ev) = event::parse(&raw) else { return };
                let id = self.state.apply(&ev, procs::find_pids(&chain), now);
                if let (Some(id), Some((reply, req))) = (id, reply) {
                    self.replies.insert(id, reply);
                    let _ = req.send(id);
                }
            }
            Msg::Timeout(id) => {
                self.replies.remove(&id);
                self.state.timeout(id);
            }
        }
    }

    pub fn answer(&mut self, req_id: u64, r: Reply) {
        if let Some(tx) = self.replies.remove(&req_id) {
            let _ = tx.send(r);
        }
        self.state.resolve(req_id);
    }

    /// True when the process should exit.
    pub fn tick(&mut self, now: Instant) -> bool {
        if self.last_prune.is_none_or(|t| now.duration_since(t) >= PRUNE_EVERY) {
            self.last_prune = Some(now);
            self.state.prune(procs::is_alive);
        }
        if !self.state.is_empty() {
            self.empty_since = None;
            return false;
        }
        now.duration_since(*self.empty_since.get_or_insert(now)) >= EXIT_AFTER_EMPTY
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};
    use std::sync::mpsc::channel;

    fn raw(name: &str, sid: &str) -> Value {
        json!({"hook_event_name": name, "session_id": sid, "cwd": r"D:\p\x", "transcript_path": "t.jsonl", "tool_name": "Bash", "tool_input": {"command": "ls"}})
    }

    fn event(core: &mut Core, name: &str, sid: &str, wants: bool) -> Option<(std::sync::mpsc::Receiver<Reply>, u64)> {
        let (rtx, rrx) = channel();
        let (qtx, qrx) = channel();
        let reply = wants.then_some((rtx, qtx));
        core.handle(Msg::Event { raw: raw(name, sid), chain: vec![], reply }, 1);
        wants.then(|| (rrx, qrx.try_recv().unwrap()))
    }

    #[test]
    fn answer_sends_reply_and_resolves() {
        let mut c = Core::new();
        let (rx, id) = event(&mut c, "PermissionRequest", "s", true).unwrap();
        assert_eq!(c.state.permissions.len(), 1);
        c.answer(id, Reply::Allow);
        assert!(matches!(rx.try_recv(), Ok(Reply::Allow)));
        assert!(c.state.permissions.is_empty());
        assert!(c.replies.is_empty());
    }

    #[test]
    fn timeout_msg_marks_timed_out() {
        let mut c = Core::new();
        let (_rx, id) = event(&mut c, "PermissionRequest", "s", true).unwrap();
        c.handle(Msg::Timeout(id), 2);
        assert!(c.state.permissions[0].timed_out);
        assert!(c.replies.is_empty());
    }

    #[test]
    fn tick_exits_after_3s_empty_not_before() {
        let mut c = Core::new();
        let t = Instant::now();
        assert!(!c.tick(t));
        assert!(!c.tick(t + Duration::from_millis(2900)));
        assert!(c.tick(t + Duration::from_secs(3)));
    }

    #[test]
    fn tick_does_not_exit_while_session_exists() {
        let mut c = Core::new();
        event(&mut c, "SessionStart", "s", false);
        let t = Instant::now();
        assert!(!c.tick(t));
        assert!(!c.tick(t + Duration::from_secs(10)));
    }
}
