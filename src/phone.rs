use std::collections::HashMap;
use std::fs::File;
use std::io::Write;
use std::sync::Arc;
use std::sync::atomic::AtomicI64;
use std::sync::atomic::Ordering;
use std::sync::mpsc::Sender;
use serde_json::Value;
use crate::state::{Permission, Session, Status, TodoStatus, WaitKind};
use crate::{server::Msg, telegram::{self, Link, Out}};

pub fn circle(color: usize) -> &'static str {
    ["🟠", "🔵", "🟣", "🔴", "🟢", "⚫"][color % 6]
}

/// The phone message for a hook event (`s` is the session after the event was applied), with its inline buttons.
pub fn message(raw: &Value, s: &Session, card: Option<&Permission>) -> Option<(String, Vec<(String, String)>)> {
    let name = raw["hook_event_name"].as_str()?;
    let mut buttons = vec![];
    let body = if raw["tool_name"] == "AskUserQuestion" {
        // Fires PreToolUse and PermissionRequest; only one phone message.
        if name != "PreToolUse" {
            return None;
        }
        format!("Waiting for you: {}", raw["tool_input"]["questions"][0]["question"].as_str()?)
    } else if let Some(c) = card {
        buttons = vec![("Allow".to_owned(), format!("a:{}", c.req_id)), ("Deny".to_owned(), format!("d:{}", c.req_id))];
        format!("Permission: {} {}", c.verb, c.detail)
    } else {
        match name {
            "SessionStart" => "Started".to_owned(),
            "SessionEnd" => "Ended".to_owned(),
            "UserPromptSubmit" => format!("You (PC): {}", raw["prompt"].as_str()?),
            "Stop" => {
                let mut lines: Vec<String> = raw["last_assistant_message"].as_str().filter(|m| !m.is_empty()).map(str::to_owned).into_iter().collect();
                lines.extend(s.todos.iter().map(|t| format!("{}{}", if t.status == TodoStatus::Completed { "[x] " } else { "[ ] " }, t.subject)));
                lines.join("\n")
            }
            "StopFailure" => format!("Error: {}", ["error", "error_details"].iter().find_map(|k| raw[k].as_str()).unwrap_or("turn failed")),
            // Same cases State::apply turns into Waiting(Other).
            "Notification"
                if s.status == Status::Waiting(WaitKind::Other)
                    && !matches!(raw["notification_type"].as_str(), Some("permission_prompt" | "idle_prompt" | "agent_completed")) =>
            {
                format!("Waiting for you: {}", raw["message"].as_str().unwrap_or(""))
            }
            _ => return None,
        }
    };
    Some((format!("{} {}\n{}", circle(s.color), s.project, telegram::cut(&body)), buttons))
}

/// Phone link state: one open `listen` pipe per session, each line written is one phone prompt.
#[derive(Default)]
pub struct Phone {
    listeners: HashMap<String, File>,
    /// Telegram bot token; empty means the link is off. Never log it.
    token: String,
    link: Option<Link>,
    /// Test seam: stands in for the link's sender.
    out: Option<Sender<Out>>,
    /// Where Link threads post; None (as in `default()`) means set_token never starts one.
    tx: Option<Sender<Msg>>,
    /// Paired Telegram chat; 0 = unpaired, sends are skipped. Shared with the link's sender thread.
    pub chat: Arc<AtomicI64>,
    /// Telegram message id to the session it was sent for.
    pub sent: HashMap<i64, String>,
    /// Last text delivered per session, so its echo as a PC prompt is not sent back.
    delivered: HashMap<String, String>,
}

impl Phone {
    pub fn new(tx: Sender<Msg>) -> Phone {
        Phone { tx: Some(tx), ..Default::default() }
    }

    #[cfg(test)]
    #[allow(dead_code)]
    pub fn with_out(out: Sender<Out>) -> Phone {
        Phone { out: Some(out), ..Default::default() }
    }

    pub fn out(&self) -> Option<&Sender<Out>> {
        self.link.as_ref().map(|l| &l.out).or(self.out.as_ref())
    }

    /// Newest listener wins; the older one is dropped (its helper sees EOF).
    pub fn listen(&mut self, session_id: String, pipe: File) {
        self.listeners.insert(session_id, pipe);
    }

    /// Writes `text` as one JSON string line. A dead listener is dropped and false returned.
    pub fn deliver(&mut self, session_id: &str, text: &str) -> bool {
        let Some(mut f) = self.listeners.remove(session_id) else { return false };
        // ponytail: blocking write on the UI thread; a hung helper with a full 4 KB pipe buffer stalls the island. Upgrade: write on a per-listener thread.
        let line = format!("{}\n", serde_json::to_string(text).unwrap_or_default());
        if f.write_all(line.as_bytes()).is_err() {
            return false;
        }
        self.listeners.insert(session_id.to_owned(), f);
        self.delivered.insert(session_id.to_owned(), text.to_owned());
        true
    }

    pub fn set_token(&mut self, t: String) {
        if t == self.token {
            return;
        }
        self.link = None;
        self.token = t;
        if !self.token.is_empty()
            && let Some(tx) = self.tx.clone()
        {
            self.link = Some(telegram::start(self.token.clone(), self.chat.clone(), tx));
        }
    }

    pub fn forget(&mut self, session_id: &str) {
        self.listeners.remove(session_id);
        self.delivered.remove(session_id);
        self.sent.retain(|_, s| s != session_id);
    }

    pub fn on_event(&mut self, raw: &Value, s: &Session, card: Option<&Permission>) {
        if self.chat.load(Ordering::Relaxed) == 0 {
            return;
        }
        if raw["hook_event_name"] == "UserPromptSubmit"
            && self.delivered.get(&s.id).is_some_and(|d| Some(d.trim()) == raw["prompt"].as_str().map(str::trim))
        {
            self.delivered.remove(&s.id);
            return;
        }
        if let Some((text, buttons)) = message(raw, s, card)
            && let Some(out) = self.out()
        {
            let _ = out.send(Out::Send { session_id: Some(s.id.clone()), text, buttons });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{Permission, Session, Status, Todo, TodoStatus, WaitKind};
    use serde_json::{Value, json};
    use std::sync::atomic::Ordering;
    use std::sync::mpsc::channel;

    fn fx(name: &str) -> Value {
        let p = format!("{}/tests/fixtures/{name}.json", env!("CARGO_MANIFEST_DIR"));
        serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap()
    }

    fn sess(status: Status, todos: Vec<Todo>) -> Session {
        Session {
            id: "s".into(), parent: String::new(), project: "X".into(), cwd: String::new(), transcript: String::new(),
            color: 0, status, todos, finished: None, claude_pid: None, terminal_pid: None, changed_at: 0,
        }
    }

    fn msg(raw: &Value) -> Option<String> {
        message(raw, &sess(Status::Working, vec![]), None).map(|m| m.0)
    }

    #[test]
    fn started_ended_and_pc_prompt() {
        assert_eq!(msg(&fx("SessionStart")).unwrap(), "🟠 X\nStarted");
        let mut r = fx("UserPromptSubmit");
        r["prompt"] = json!("hi");
        assert_eq!(msg(&r).unwrap(), "🟠 X\nYou (PC): hi");
        assert_eq!(msg(&fx("SessionEnd")).unwrap(), "🟠 X\nEnded");
    }

    #[test]
    fn card_has_allow_deny_buttons() {
        let c = Permission { req_id: 1, session_id: "s".into(), tool: "Bash".into(), verb: "Wants to run a command".into(), detail: "npm i".into(), full: String::new(), timed_out: false, gated: false };
        let (t, b) = message(&fx("PermissionRequest"), &sess(Status::Working, vec![]), Some(&c)).unwrap();
        assert_eq!(t, "🟠 X\nPermission: Wants to run a command npm i");
        assert_eq!(b, vec![("Allow".to_owned(), "a:1".to_owned()), ("Deny".to_owned(), "d:1".to_owned())]);
        // AskUserQuestion's PermissionRequest stays silent: its PreToolUse already sent one.
        assert!(message(&fx("PermissionRequest-AskUserQuestion"), &sess(Status::Working, vec![]), Some(&c)).is_none());
    }

    #[test]
    fn stop_lists_todos() {
        let todos = vec![
            Todo { id: "1".into(), subject: "A".into(), status: TodoStatus::Completed },
            Todo { id: "2".into(), subject: "B".into(), status: TodoStatus::InProgress },
        ];
        let (t, _) = message(&fx("Stop"), &sess(Status::Idle, todos), None).unwrap();
        assert_eq!(t, "🟠 X\ndone\n[x] A\n[ ] B");
    }

    #[test]
    fn stop_failure_and_waiting() {
        let mut r = fx("Stop");
        r["hook_event_name"] = json!("StopFailure");
        assert_eq!(msg(&r).unwrap(), "🟠 X\nError: turn failed");
        r["error"] = json!("rate_limit");
        assert_eq!(msg(&r).unwrap(), "🟠 X\nError: rate_limit");
        assert_eq!(msg(&fx("PreToolUse-AskUserQuestion")).unwrap(), "🟠 X\nWaiting for you: Pick option for probe test: X or Y?");
        let mut n = fx("Notification-permission_prompt");
        n["notification_type"] = json!("needs_input");
        n["message"] = json!("Claude is waiting");
        assert!(msg(&n).is_none(), "status is not Waiting(Other)");
        let (t, _) = message(&n, &sess(Status::Waiting(WaitKind::Other), vec![]), None).unwrap();
        assert_eq!(t, "🟠 X\nWaiting for you: Claude is waiting");
    }

    #[test]
    fn tool_calls_send_nothing() {
        assert!(msg(&fx("PreToolUse")).is_none());
        assert!(msg(&fx("PostToolUse")).is_none());
        let s = sess(Status::Waiting(WaitKind::Other), vec![]);
        assert!(message(&fx("Notification-permission_prompt"), &s, None).is_none());
    }

    #[test]
    fn indigo_is_black_circle() {
        assert_eq!(circle(5), "⚫");
        assert_eq!(circle(3), "🔴");
    }

    fn paired() -> (Phone, std::sync::mpsc::Receiver<Out>) {
        let (tx, rx) = channel();
        let p = Phone::with_out(tx);
        p.chat.store(7, Ordering::Relaxed);
        (p, rx)
    }

    #[test]
    fn phone_prompt_not_echoed() {
        let (mut p, rx) = paired();
        p.listen("s".into(), File::create("NUL").unwrap());
        assert!(p.deliver("s", "go"));
        let mut r = fx("UserPromptSubmit");
        r["prompt"] = json!(" go\n");
        let s = sess(Status::Working, vec![]);
        p.on_event(&r, &s, None);
        assert!(rx.try_recv().is_err());
        p.on_event(&r, &s, None);
        assert!(matches!(rx.try_recv(), Ok(Out::Send { session_id: Some(id), .. }) if id == "s"));
    }

    #[test]
    fn nothing_sent_while_unpaired() {
        let (mut p, rx) = paired();
        p.chat.store(0, Ordering::Relaxed);
        p.on_event(&fx("SessionStart"), &sess(Status::Idle, vec![]), None);
        assert!(rx.try_recv().is_err());
    }
}
