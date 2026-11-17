use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::hash::{BuildHasher, RandomState};
use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI64};
use std::sync::atomic::Ordering;
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};
use serde_json::Value;
use crate::hook::Reply;
use crate::state::{Permission, Session, Status, TodoStatus, WaitKind};
use crate::{server::Msg, telegram::{self, Incoming, Link, Out}};

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
            // Not `compact`: auto-compaction is not a new session.
            "SessionStart" if matches!(raw["source"].as_str(), Some("startup" | "clear" | "resume")) => "Started".to_owned(),
            "SessionEnd" => "Ended".to_owned(),
            "UserPromptSubmit" => format!("You (PC): {}", raw["prompt"].as_str()?),
            "Stop" => {
                let mut lines: Vec<String> = raw["last_assistant_message"].as_str().filter(|m| !m.is_empty()).map(str::to_owned).into_iter().collect();
                lines.extend(s.todos.iter().map(|t| format!("{}{}", if t.status == TodoStatus::Completed { "[x] " } else { "[ ] " }, t.subject)));
                if lines.is_empty() {
                    return None;
                }
                lines.join("\n")
            }
            "StopFailure" => format!("Error: {}", ["error", "error_details"].iter().find_map(|k| raw[k].as_str()).unwrap_or("turn failed")),
            // Same cases State::apply turns into Waiting(Other).
            "Notification"
                if s.status == Status::Waiting(WaitKind::Other)
                    && !matches!(raw["notification_type"].as_str(), Some("permission_prompt" | "idle_prompt" | "agent_completed")) =>
            {
                match raw["message"].as_str().filter(|m| !m.is_empty()) {
                    Some(m) => format!("Waiting for you: {m}"),
                    None => "Waiting for you".to_owned(),
                }
            }
            _ => return None,
        }
    };
    Some((format!("{} {}\n{}", circle(s.color), s.project, telegram::cut(&body)), buttons))
}

/// A held AskUserQuestion waiting for phone answers.
#[allow(dead_code)] // answers and picks are filled by the answer flow (Task 5)
pub struct Ask {
    session_id: String,
    questions: Vec<Value>,
    answers: serde_json::Map<String, Value>,
    picks: Vec<String>,
    current: usize,
}

pub enum Action {
    None,
    /// Allow/Deny tap (request id, reply, callback id): Core decides if it still applies and answers the tap.
    Answer(u64, Reply, String),
}

pub enum PhoneStatus {
    Off,
    Pairing { code: String, bot: Option<String> },
    Paired,
}

const CODE_TTL: Duration = Duration::from_secs(600);

fn new_code() -> String {
    format!("{:06}", RandomState::new().hash_one(0u8) % 1_000_000)
}

/// Phone link state: one open `listen` pipe per session, each line written is one phone prompt.
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
    /// Away mode, shared with the pipe server: while unset, `on_event` sends nothing.
    pub away: Arc<AtomicBool>,
    /// Telegram message id to the session it was sent for.
    pub sent: HashMap<i64, String>,
    /// Last text delivered per session, so its echo as a PC prompt is not sent back.
    delivered: HashMap<String, String>,
    /// Pairing code the chat must send, its age and the wrong guesses since.
    pub code: String,
    code_at: Instant,
    wrong: u32,
    pub bot: Option<String>,
    /// Text waiting for a session pick.
    held: Option<String>,
    /// Permission ids whose hook gave up waiting.
    pub timed_out: HashSet<u64>,
    /// Held questions by request id, and the Telegram message of each question to (id, question index).
    pub asks: HashMap<u64, Ask>,
    pub ask_msgs: HashMap<i64, (u64, usize)>,
    /// Settings file for `phone_chat`; None in tests.
    pub settings: Option<PathBuf>,
    /// Unix seconds at island start: Telegram messages and button taps dated before it are ignored.
    started: i64,
    /// The "Not delivered" notice goes out once per island run.
    told_stale: bool,
}

fn unix_now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

impl Default for Phone {
    fn default() -> Phone {
        Phone {
            listeners: HashMap::new(), token: String::new(), link: None, out: None, tx: None, chat: Arc::default(), away: Arc::default(), sent: HashMap::new(),
            delivered: HashMap::new(), code: new_code(), code_at: Instant::now(), wrong: 0, bot: None, held: None, timed_out: HashSet::new(), asks: HashMap::new(), ask_msgs: HashMap::new(), settings: None,
            started: unix_now(), told_stale: false,
        }
    }
}

impl Phone {
    pub fn new(tx: Sender<Msg>) -> Phone {
        Phone { tx: Some(tx), ..Default::default() }
    }

    #[cfg(test)]
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
        self.renew(Instant::now());
        if !self.token.is_empty()
            && let Some(tx) = self.tx.clone()
        {
            self.link = Some(telegram::start(self.token.clone(), self.chat.clone(), tx));
        }
    }

    fn renew(&mut self, now: Instant) {
        self.code = new_code();
        self.code_at = now;
        self.wrong = 0;
    }

    fn expire(&mut self, now: Instant) {
        if now.saturating_duration_since(self.code_at) > CODE_TTL {
            self.renew(now);
        }
    }

    fn save_chat(&self, v: Value) {
        if let Some(p) = &self.settings {
            let _ = crate::settings::set(p, "phone_chat", v);
        }
    }

    pub fn disconnect(&mut self) {
        self.chat.store(0, Ordering::Relaxed);
        self.renew(Instant::now());
        self.save_chat(Value::Null);
    }

    pub fn status(&mut self, now: Instant) -> PhoneStatus {
        self.expire(now);
        if self.chat.load(Ordering::Relaxed) != 0 {
            PhoneStatus::Paired
        } else if self.token.is_empty() {
            PhoneStatus::Off
        } else {
            PhoneStatus::Pairing { code: self.code.clone(), bot: self.bot.clone() }
        }
    }

    fn say(&self, text: impl Into<String>, buttons: Vec<(String, String)>) {
        if let Some(out) = self.out() {
            let _ = out.send(Out::Send { session_id: None, text: text.into(), buttons, tag: None });
        }
    }

    pub fn ack(&self, callback_id: String, text: &str) {
        if let Some(out) = self.out() {
            let _ = out.send(Out::Answer { callback_id, text: text.to_owned() });
        }
    }

    fn send_to(&mut self, sid: &str, text: &str, sessions: &[Session]) {
        let s = sessions.iter().find(|s| s.id == sid);
        let name = s.map_or(sid, |s| s.project.as_str()).to_owned();
        if !self.deliver(sid, text) {
            self.say(format!("{name} can't receive messages right now"), vec![]);
        } else if s.is_some_and(|s| s.status != Status::Idle) {
            self.say(format!("Queued: {name} is busy"), vec![]);
        }
    }

    /// Routes one Telegram update; Allow/Deny taps are returned for Core, which owns the pending replies.
    pub fn incoming(&mut self, m: Incoming, sessions: &[Session], now: Instant) -> Action {
        let paired = self.chat.load(Ordering::Relaxed);
        // Updates from before this run (the island is closed while no session is open) must not act now.
        match m {
            Incoming::Text { chat, date, .. } if date < self.started => {
                if paired != 0 && chat == paired && !self.told_stale {
                    self.told_stale = true;
                    self.say("Not delivered: sent while no session was open", vec![]);
                }
                return Action::None;
            }
            Incoming::Button { chat, callback_id, date, .. } if date < self.started => {
                if paired != 0 && chat == paired {
                    self.ack(callback_id, "Too late, answered in the terminal");
                }
                return Action::None;
            }
            _ => {}
        }
        match m {
            Incoming::BotName(n) => self.bot = Some(n),
            Incoming::Text { chat, text, .. } if paired == 0 => {
                self.expire(now);
                if text.trim() == self.code {
                    self.chat.store(chat, Ordering::Relaxed);
                    self.save_chat(chat.into());
                    self.say("Paired", vec![]);
                } else {
                    self.wrong += 1;
                    if self.wrong >= 5 {
                        self.renew(now);
                    }
                }
            }
            Incoming::Text { chat, text, reply_to, .. } if chat == paired => {
                if let Some(sid) = reply_to.and_then(|r| self.sent.get(&r)).cloned() {
                    self.send_to(&sid, &text, sessions);
                } else {
                    match sessions {
                        [] => self.say("No open sessions", vec![]),
                        [s] => self.send_to(&s.id, &text, sessions),
                        _ => {
                            self.held = Some(text);
                            let buttons = sessions.iter().map(|s| (format!("{} {}", circle(s.color), s.project), format!("s:{}", s.id))).collect();
                            self.say("Which session?", buttons);
                        }
                    }
                }
            }
            Incoming::Button { chat, callback_id, data, .. } if paired != 0 && chat == paired => {
                let tap = data.split_once(':').and_then(|(k, id)| Some((match k { "a" => Reply::Allow, "d" => Reply::Deny, _ => return None }, id.parse().ok()?)));
                if let Some((reply, id)) = tap {
                    return Action::Answer(id, reply, callback_id);
                }
                self.ack(callback_id, "");
                if let Some(sid) = data.strip_prefix("s:")
                    && let Some(text) = self.held.take()
                {
                    self.send_to(sid, &text, sessions);
                }
            }
            _ => {}
        }
        Action::None
    }

    pub fn forget(&mut self, session_id: &str) {
        self.listeners.remove(session_id);
        self.delivered.remove(session_id);
        self.sent.retain(|_, s| s != session_id);
        self.asks.retain(|_, a| a.session_id != session_id);
    }

    /// Holds a question set and sends question 0 with one button per option.
    pub fn ask(&mut self, id: u64, session: &Session, questions: Vec<Value>) {
        self.asks.insert(id, Ask { session_id: session.id.clone(), questions, answers: Default::default(), picks: vec![], current: 0 });
        self.send_question(id, session);
    }

    fn send_question(&self, id: u64, s: &Session) {
        let Some(a) = self.asks.get(&id) else { return };
        let q = &a.questions[a.current];
        let text = format!("{} {}
{}
{}", circle(s.color), s.project, q["header"].as_str().unwrap_or_default(), q["question"].as_str().unwrap_or_default());
        let mut buttons: Vec<(String, String)> = q["options"].as_array().into_iter().flatten().enumerate()
            .map(|(oi, o)| (o["label"].as_str().unwrap_or_default().to_owned(), format!("q:{id}:{}:{oi}", a.current))).collect();
        if q["multiSelect"] == true {
            buttons.push(("Done".into(), format!("q:{id}:{}:done", a.current)));
        }
        if let Some(out) = self.out() {
            let _ = out.send(Out::Send { session_id: Some(s.id.clone()), text: telegram::cut(&text), buttons, tag: Some(format!("q:{id}:{}", a.current)) });
        }
    }

    /// Remembers which held question a sent message (tag `q:<id>:<qi>`) carries.
    pub fn note_sent(&mut self, message_id: i64, tag: Option<&str>) {
        let mut it = tag.and_then(|t| t.strip_prefix("q:")).unwrap_or_default().split(':');
        if let (Some(Ok(id)), Some(Ok(qi))) = (it.next().map(str::parse), it.next().map(str::parse))
            && self.asks.contains_key(&id)
        {
            self.ask_msgs.insert(message_id, (id, qi));
        }
    }

    /// Forgets a held question that was released or timed out; late taps for it get "Too late".
    pub fn drop_ask(&mut self, id: u64) {
        self.timed_out.insert(id);
        self.asks.remove(&id);
        self.ask_msgs.retain(|_, (i, _)| *i != id);
    }

    pub fn on_event(&mut self, raw: &Value, s: &Session, card: Option<&Permission>) {
        if self.chat.load(Ordering::Relaxed) == 0 || !self.away.load(Ordering::Relaxed) {
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
            let _ = out.send(Out::Send { session_id: Some(s.id.clone()), text, buttons, tag: None });
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
    fn started_only_for_startup_clear_resume() {
        let mut r = fx("SessionStart");
        for (src, sent) in [("startup", true), ("clear", true), ("resume", true), ("compact", false), ("other", false)] {
            r["source"] = json!(src);
            assert_eq!(msg(&r).is_some(), sent, "{src}");
        }
        r["source"] = json!(null);
        assert!(msg(&r).is_none());
    }

    #[test]
    fn empty_stop_and_bare_waiting() {
        let mut r = fx("Stop");
        r["last_assistant_message"] = json!("");
        assert!(msg(&r).is_none(), "no text and no todos: nothing to send");
        let mut n = fx("Notification-permission_prompt");
        n["notification_type"] = json!("needs_input");
        let s = sess(Status::Waiting(WaitKind::Other), vec![]);
        n["message"] = json!(null);
        assert_eq!(message(&n, &s, None).unwrap().0, "🟠 X\nWaiting for you");
        n["message"] = json!("");
        assert_eq!(message(&n, &s, None).unwrap().0, "🟠 X\nWaiting for you");
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
        p.away.store(true, Ordering::Relaxed);
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

    #[test]
    fn away_off_sends_nothing() {
        let (mut p, rx) = paired();
        p.on_event(&fx("SessionStart"), &sess(Status::Idle, vec![]), None);
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn away_on_sends() {
        let (mut p, rx) = paired();
        p.away.store(true, Ordering::Relaxed);
        p.on_event(&fx("SessionStart"), &sess(Status::Idle, vec![]), None);
        assert!(matches!(rx.try_recv(), Ok(Out::Send { text, .. }) if text == "🟠 X\nStarted"));
    }

    fn two() -> Vec<Session> {
        let mut b = sess(Status::Idle, vec![]);
        b.id = "t".into();
        b.project = "Y".into();
        b.color = 1;
        vec![sess(Status::Idle, vec![]), b]
    }

    fn text(chat: i64, t: &str, reply_to: Option<i64>) -> Incoming {
        Incoming::Text { chat, text: t.into(), reply_to, date: i64::MAX }
    }

    fn unpaired() -> (Phone, std::sync::mpsc::Receiver<Out>) {
        let (p, rx) = paired();
        p.chat.store(0, Ordering::Relaxed);
        (p, rx)
    }

    #[test]
    fn exact_code_pairs() {
        let (mut p, rx) = unpaired();
        let now = Instant::now();
        let code = p.code.clone();
        p.incoming(text(5, "nope", None), &[], now);
        assert_eq!(p.chat.load(Ordering::Relaxed), 0);
        p.incoming(text(5, &format!(" {code}
"), None), &[], now);
        assert_eq!(p.chat.load(Ordering::Relaxed), 5);
        assert!(matches!(rx.try_recv(), Ok(Out::Send { text, .. }) if text == "Paired"));
    }

    #[test]
    fn stranger_is_ignored() {
        let (mut p, rx) = paired();
        p.chat.store(5, Ordering::Relaxed);
        p.listen("s".into(), File::create("NUL").unwrap());
        p.incoming(text(6, "hi", None), &[sess(Status::Idle, vec![])], Instant::now());
        assert!(rx.try_recv().is_err());
        assert!(p.delivered.is_empty());
    }

    #[test]
    fn five_wrong_codes_renew() {
        let (mut p, _rx) = unpaired();
        let now = Instant::now();
        let old = p.code.clone();
        for _ in 0..5 {
            p.incoming(text(5, "x", None), &[], now);
        }
        assert_ne!(p.code, old);
        assert_eq!(p.wrong, 0);
        p.incoming(text(5, &old, None), &[], now);
        assert_eq!(p.chat.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn token_change_renews_code() {
        let (mut p, _rx) = unpaired();
        let old = p.code.clone();
        p.wrong = 3;
        p.set_token("t".into());
        assert_ne!(p.code, old);
        assert_eq!(p.wrong, 0);
    }

    #[test]
    fn status_renews_expired_code() {
        let (mut p, _rx) = unpaired();
        p.set_token("t".into());
        let old = p.code.clone();
        let later = Instant::now() + std::time::Duration::from_secs(601);
        let PhoneStatus::Pairing { code, .. } = p.status(later) else { panic!("not pairing") };
        assert_ne!(code, old);
        p.incoming(text(5, &code, None), &[], later);
        assert_eq!(p.chat.load(Ordering::Relaxed), 5);
    }

    #[test]
    fn code_expires_after_10_min() {
        let (mut p, _rx) = unpaired();
        let old = p.code.clone();
        p.incoming(text(5, &old, None), &[], Instant::now() + std::time::Duration::from_secs(601));
        assert_eq!(p.chat.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn reply_routes_to_session() {
        let (mut p, _rx) = paired();
        p.listen("s".into(), File::create("NUL").unwrap());
        p.listen("t".into(), File::create("NUL").unwrap());
        p.sent.insert(3, "s".into());
        p.incoming(text(7, "go", Some(3)), &two(), Instant::now());
        assert_eq!(p.delivered.get("s").map(String::as_str), Some("go"));
        assert!(!p.delivered.contains_key("t"));
    }

    #[test]
    fn single_session_takes_plain_text() {
        let (mut p, rx) = paired();
        p.listen("s".into(), File::create("NUL").unwrap());
        p.incoming(text(7, "go", None), &[sess(Status::Idle, vec![])], Instant::now());
        assert_eq!(p.delivered.get("s").map(String::as_str), Some("go"));
        assert!(rx.try_recv().is_err());
        p.incoming(text(7, "go", None), &[], Instant::now());
        assert!(matches!(rx.try_recv(), Ok(Out::Send { text, .. }) if text == "No open sessions"));
    }

    #[test]
    fn several_sessions_ask_which() {
        let (mut p, rx) = paired();
        p.listen("t".into(), File::create("NUL").unwrap());
        let now = Instant::now();
        p.incoming(text(7, "go", None), &two(), now);
        let Ok(Out::Send { text: t, buttons, .. }) = rx.try_recv() else { panic!("no question") };
        assert_eq!(t, "Which session?");
        assert_eq!(buttons, vec![("🟠 X".to_owned(), "s:s".to_owned()), ("🔵 Y".to_owned(), "s:t".to_owned())]);
        let b = Incoming::Button { chat: 7, callback_id: "c".into(), data: "s:t".into(), date: i64::MAX };
        assert!(matches!(p.incoming(b, &two(), now), Action::None));
        assert!(matches!(rx.try_recv(), Ok(Out::Answer { callback_id, text }) if callback_id == "c" && text.is_empty()));
        assert_eq!(p.delivered.get("t").map(String::as_str), Some("go"));
    }

    #[test]
    fn stale_text_is_dropped_with_one_notice_per_run() {
        let (mut p, rx) = paired();
        p.listen("s".into(), File::create("NUL").unwrap());
        let old = |chat, t: &str| Incoming::Text { chat, text: t.into(), reply_to: None, date: 1 };
        let one = [sess(Status::Idle, vec![])];
        p.incoming(old(7, "old"), &one, Instant::now());
        p.incoming(old(7, "older"), &one, Instant::now());
        assert!(p.delivered.is_empty(), "nothing typed into the session");
        assert!(matches!(rx.try_recv(), Ok(Out::Send { text, .. }) if text == "Not delivered: sent while no session was open"));
        assert!(rx.try_recv().is_err(), "notice only once");
        p.incoming(old(6, "stranger"), &one, Instant::now());
        assert!(rx.try_recv().is_err(), "strangers get no reply");
        p.incoming(text(7, "now", None), &one, Instant::now());
        assert_eq!(p.delivered.get("s").map(String::as_str), Some("now"));
    }

    #[test]
    fn stale_button_is_answered_too_late() {
        let (mut p, rx) = paired();
        let b = |chat, data: &str, date| Incoming::Button { chat, callback_id: "c".into(), data: data.into(), date };
        assert!(matches!(p.incoming(b(7, "a:1", 1), &[], Instant::now()), Action::None));
        assert!(matches!(rx.try_recv(), Ok(Out::Answer { text, .. }) if text == "Too late, answered in the terminal"));
        assert!(matches!(p.incoming(b(6, "a:1", 1), &[], Instant::now()), Action::None));
        assert!(rx.try_recv().is_err(), "strangers are ignored");
        assert!(matches!(p.incoming(b(7, "a:1", i64::MAX), &[], Instant::now()), Action::Answer(1, _, _)));
    }

    #[test]
    fn no_listener_says_cant_receive() {
        let (mut p, rx) = paired();
        p.incoming(text(7, "go", None), &[sess(Status::Idle, vec![])], Instant::now());
        assert!(matches!(rx.try_recv(), Ok(Out::Send { text, .. }) if text == "X can't receive messages right now"));
    }

    #[test]
    fn busy_session_says_queued() {
        let (mut p, rx) = paired();
        p.listen("s".into(), File::create("NUL").unwrap());
        p.incoming(text(7, "go", None), &[sess(Status::Working, vec![])], Instant::now());
        assert!(matches!(rx.try_recv(), Ok(Out::Send { text, .. }) if text == "Queued: X is busy"));
    }
}
