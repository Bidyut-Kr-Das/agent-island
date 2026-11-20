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

type PhoneMessage = (String, Vec<(String, String)>, Vec<Value>);

/// The phone message for a hook event (`s` is the session after the event was applied), with its inline buttons and formatting entities.
pub fn message(raw: &Value, s: &Session, card: Option<&Permission>) -> Option<PhoneMessage> {
    let name = raw["hook_event_name"].as_str()?;
    let mut buttons = vec![];
    let mut entities = vec![];
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
            // Background task results arrive as prompts; they are not the user's.
            "UserPromptSubmit" => format!("You (PC): {}", raw["prompt"].as_str().filter(|p| !p.starts_with("<task-notification>"))?),
            "Stop" => {
                let (answer, spans) = telegram::render(raw["last_assistant_message"].as_str().unwrap_or_default());
                entities = spans;
                let mut lines: Vec<String> = Some(answer).filter(|a| !a.is_empty()).into_iter().collect();
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
    let head = format!("{} {}\n", circle(s.color), s.project);
    let entities = telegram::place(entities, &head, &body);
    Some((format!("{head}{}", telegram::cut(&body)), buttons, entities))
}

/// A held AskUserQuestion waiting for phone answers.
pub struct Ask {
    session_id: String,
    questions: Vec<Value>,
    answers: serde_json::Map<String, Value>,
    picks: Vec<String>,
    current: usize,
}

pub enum Action {
    None,
    /// An answer (request id, reply, callback id of the tap; None for typed text): Core decides if it still applies and acks the tap.
    Answer(u64, Reply, Option<String>),
    /// `/away on|off` from the paired chat: Core flips the switch, then confirms.
    Away(bool),
}

pub enum PhoneStatus {
    Off,
    Pairing { code: String, bot: Option<String>, problem: Option<&'static str> },
    Paired,
}

impl PhoneStatus {
    /// What the user must do to pair, or why they can't right now.
    pub fn pairing_line(&self) -> Option<String> {
        let PhoneStatus::Pairing { code, bot, problem } = self else { return None };
        Some(match problem {
            Some(p) => format!("Phone: {p}"),
            None => format!("Phone: send {code} to {}", bot.as_ref().map_or("your bot".to_string(), |b| format!("@{b}"))),
        })
    }
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
    /// Why the Telegram link is failing right now.
    problem: Option<&'static str>,
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
    /// Test seam: runs a phone command (default: real zoxide + wt).
    pub run_cmd: fn(crate::command::Cmd, Sender<Out>),
    /// Test seam: stores or clears the token and the login entry (default: `settings::autostart`).
    pub save_token: fn(&std::path::Path, Option<&str>),
}

fn unix_now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

impl Default for Phone {
    fn default() -> Phone {
        Phone {
            listeners: HashMap::new(), token: String::new(), link: None, out: None, tx: None, chat: Arc::default(), away: Arc::default(), sent: HashMap::new(),
            delivered: HashMap::new(), code: new_code(), code_at: Instant::now(), wrong: 0, bot: None, problem: None, held: None, timed_out: HashSet::new(), asks: HashMap::new(), ask_msgs: HashMap::new(), settings: None,
            started: unix_now(), told_stale: false, run_cmd: crate::command::spawn,
            // Tests never touch the real token or login entry, even with `settings` set.
            save_token: if cfg!(test) { |_, _| {} } else { crate::settings::autostart },
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
        self.problem = None;
        self.renew(Instant::now());
        if !self.token.is_empty()
            && let Some(tx) = self.tx.clone()
        {
            self.link = Some(telegram::start(self.token.clone(), self.chat.clone(), tx));
        }
    }

    pub fn token(&self) -> &str {
        &self.token
    }

    /// Sets the token without starting a Telegram link.
    #[cfg(test)]
    pub fn set_token_for_test(&mut self, t: &str) {
        self.token = t.to_owned();
    }

    /// `on`: keep the token (encrypted) and start at login; otherwise clear both. No-op without a settings file.
    pub fn autostart(&self, on: bool) {
        if let Some(p) = &self.settings {
            (self.save_token)(p, on.then_some(self.token.as_str()));
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
        self.autostart(false);
    }

    pub fn status(&mut self, now: Instant) -> PhoneStatus {
        self.expire(now);
        if self.chat.load(Ordering::Relaxed) != 0 {
            PhoneStatus::Paired
        } else if self.token.is_empty() {
            PhoneStatus::Off
        } else {
            PhoneStatus::Pairing { code: self.code.clone(), bot: self.bot.clone(), problem: self.problem }
        }
    }

    pub(crate) fn say(&self, text: impl Into<String>, buttons: Vec<(String, String)>) {
        if let Some(out) = self.out() {
            let _ = out.send(Out::Send { session_id: None, text: text.into(), buttons, tag: None, entities: vec![] });
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
            Incoming::LinkProblem(p) => self.problem = p,
            Incoming::Text { chat, text, .. } if paired == 0 => {
                self.expire(now);
                if text.trim() == self.code {
                    self.chat.store(chat, Ordering::Relaxed);
                    self.save_chat(chat.into());
                    self.autostart(true);
                    self.say("Paired", vec![]);
                } else {
                    self.wrong += 1;
                    if self.wrong >= 5 {
                        self.renew(now);
                    }
                }
            }
            Incoming::Text { chat, text, reply_to, .. } if chat == paired => {
                use crate::command::{self, Cmd};
                // `/open@OtherBot` is meant for another bot in the chat: plain text here.
                let other_bot = command::addressee(&text).zip(self.bot.as_deref()).is_some_and(|(a, b)| !a.eq_ignore_ascii_case(b));
                match if other_bot { None } else { command::parse(&text) } {
                    None => {}
                    Some(Err(usage)) => { self.say(usage, vec![]); return Action::None; }
                    Some(Ok(Cmd::Away(on))) => return Action::Away(on),
                    Some(Ok(Cmd::Help)) => { self.say(command::HELP, vec![]); return Action::None; }
                    Some(Ok(c)) => {
                        if !self.away.load(Ordering::Relaxed) {
                            self.say("Turn Away on to use commands", vec![]);
                        } else if let Some(o) = self.out() {
                            (self.run_cmd)(c, o.clone());
                        }
                        return Action::None;
                    }
                }
                if let Some(&(id, qi)) = reply_to.and_then(|r| self.ask_msgs.get(&r)) {
                    if self.asks.get(&id).is_some_and(|a| a.current == qi) {
                        return self.record(id, text, sessions).map_or(Action::None, |r| Action::Answer(id, r, None));
                    }
                    self.say(self.gone(id), vec![]);
                } else if let Some(sid) = reply_to.and_then(|r| self.sent.get(&r)).cloned() {
                    self.send_to(&sid, &text, sessions);
                } else if let Some(waiting) = self.waiting(sessions) {
                    if let [(id, _)] = waiting[..] {
                        return self.record(id, text, sessions).map_or(Action::None, |r| Action::Answer(id, r, None));
                    }
                    let buttons = waiting.iter().filter_map(|&(id, qi)| {
                        let a = &self.asks[&id];
                        let s = sessions.iter().find(|s| s.id == a.session_id)?;
                        Some((format!("{} {}: {}", circle(s.color), s.project, a.questions[qi]["header"].as_str().unwrap_or_default()), format!("w:{id}:{qi}")))
                    }).collect();
                    self.held = Some(text);
                    self.say("Which question?", buttons);
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
                    return Action::Answer(id, reply, Some(callback_id));
                }
                if let Some(rest) = data.strip_prefix("q:") {
                    return self.tap(rest, callback_id, sessions);
                }
                if let Some(rest) = data.strip_prefix("w:") {
                    return self.pick_question(rest, callback_id, sessions);
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

    /// Why a tap or reply for a question that is no longer held does nothing.
    fn gone(&self, id: u64) -> String {
        if self.timed_out.contains(&id) { "Too late, answered in the terminal" } else { "Already answered" }.to_owned()
    }

    /// The (ask id, question index) each held ask is showing now, oldest first; None when nothing waits.
    fn waiting(&self, sessions: &[Session]) -> Option<Vec<(u64, usize)>> {
        let mut w: Vec<(u64, usize)> = self.asks.iter().filter(|(_, a)| sessions.iter().any(|s| s.id == a.session_id)).map(|(&id, a)| (id, a.current)).collect();
        w.sort();
        (!w.is_empty()).then_some(w)
    }

    /// Stores `text` as the answer to ask `id`'s current question, then sends the next one; the finished reply when none is left.
    fn record(&mut self, id: u64, text: String, sessions: &[Session]) -> Option<Reply> {
        let a = self.asks.get_mut(&id)?;
        let key = a.questions[a.current]["question"].as_str().unwrap_or_default().to_owned();
        a.answers.insert(key, Value::String(text));
        a.picks.clear();
        a.current += 1;
        if a.current < a.questions.len() {
            if let Some(s) = sessions.iter().find(|s| s.id == a.session_id) {
                self.send_question(id, s);
            }
            return None;
        }
        // ask_msgs stays so a late reply to its messages is not typed into the session.
        let a = self.asks.remove(&id)?;
        Some(Reply::Answer(Value::Object(a.answers)))
    }

    /// A `w:<id>:<qi>` tap: the held text answers that question.
    fn pick_question(&mut self, rest: &str, callback_id: String, sessions: &[Session]) -> Action {
        let mut it = rest.split(':');
        if let (Some(Ok(id)), Some(Ok(qi)), Some(text)) = (it.next().map(str::parse::<u64>), it.next().map(str::parse::<usize>), self.held.take()) {
            if !self.asks.get(&id).is_some_and(|a| a.current == qi) {
                self.ack(callback_id, &self.gone(id));
                return Action::None;
            }
            if let Some(r) = self.record(id, text, sessions) {
                return Action::Answer(id, r, Some(callback_id));
            }
        }
        self.ack(callback_id, "");
        Action::None
    }

    /// A `q:<id>:<qi>:<oi|done>` tap (the part after `q:`).
    fn tap(&mut self, rest: &str, callback_id: String, sessions: &[Session]) -> Action {
        let mut it = rest.split(':');
        let (Some(Ok(id)), Some(Ok(qi)), Some(which)) = (it.next().map(str::parse::<u64>), it.next().map(str::parse::<usize>), it.next()) else {
            self.ack(callback_id, "");
            return Action::None;
        };
        let Some(a) = self.asks.get_mut(&id).filter(|a| a.current == qi) else {
            self.ack(callback_id, &self.gone(id));
            return Action::None;
        };
        let q = &a.questions[qi];
        let labels: Vec<String> = q["options"].as_array().into_iter().flatten().map(|o| o["label"].as_str().unwrap_or_default().to_owned()).collect();
        let multi = q["multiSelect"] == true;
        let oi = which.parse::<usize>().ok().filter(|&i| i < labels.len());
        if multi && let Some(oi) = oi {
            match a.picks.iter().position(|p| *p == labels[oi]) {
                Some(i) => { a.picks.remove(i); }
                None => a.picks.push(labels[oi].clone()),
            }
        }
        // Picks are shown and joined in option order.
        let picked = labels.iter().filter(|l| a.picks.contains(l)).cloned().collect::<Vec<_>>().join(", ");
        let answer = match (multi, which, oi) {
            (true, "done", _) if !picked.is_empty() => picked,
            (false, _, Some(oi)) => labels[oi].clone(),
            _ => {
                let text = if multi { format!("Picked: {picked}") } else { String::new() };
                self.ack(callback_id, &text);
                return Action::None;
            }
        };
        match self.record(id, answer, sessions) {
            Some(r) => Action::Answer(id, r, Some(callback_id)),
            None => {
                self.ack(callback_id, "");
                Action::None
            }
        }
    }

    pub fn forget(&mut self, session_id: &str) {
        self.listeners.remove(session_id);
        self.delivered.remove(session_id);
        self.sent.retain(|_, s| s != session_id);
        let ids: Vec<u64> = self.asks.iter().filter(|(_, a)| a.session_id == session_id).map(|(id, _)| *id).collect();
        self.ask_msgs.retain(|_, (id, _)| !ids.contains(id));
        self.asks.retain(|_, a| a.session_id != session_id);
    }

    /// Forgets sessions that left without SessionEnd (pruned for a dead pid).
    pub fn forget_gone(&mut self, sessions: &[Session]) {
        let gone: HashSet<String> = self.listeners.keys().chain(self.delivered.keys()).chain(self.sent.values()).chain(self.asks.values().map(|a| &a.session_id))
            .filter(|id| !sessions.iter().any(|s| &s.id == *id)).cloned().collect();
        for id in gone {
            self.forget(&id);
        }
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
            let _ = out.send(Out::Send { session_id: Some(s.id.clone()), text: telegram::cut(&text), buttons, tag: Some(format!("q:{id}:{}", a.current)), entities: vec![] });
        }
    }

    /// Remembers which held question a sent message (tag `q:<id>:<qi>`) carries.
    pub fn note_sent(&mut self, message_id: i64, tag: Option<&str>) {
        let mut it = tag.and_then(|t| t.strip_prefix("q:")).unwrap_or_default().split(':');
        if let (Some(Ok(id)), Some(Ok(qi))) = (it.next().map(str::parse), it.next().map(str::parse))
            && self.asks.contains_key(&id)
        {
            self.ask_msgs.insert(message_id, (id, qi));
            // Oldest message ids go first; keeps a days-long island bounded.
            while self.ask_msgs.len() > 256 {
                let Some(&k) = self.ask_msgs.keys().min() else { break };
                self.ask_msgs.remove(&k);
            }
        }
    }

    /// Forgets a held question that was released or timed out; late taps for it get "Too late".
    pub fn drop_ask(&mut self, id: u64) {
        self.timed_out.insert(id);
        // Request ids only grow, so the minimum is the oldest.
        while self.timed_out.len() > 256 {
            let Some(&k) = self.timed_out.iter().min() else { break };
            self.timed_out.remove(&k);
        }
        self.asks.remove(&id);
    }

    /// A session's turn ended, so its hook is gone: drop its held questions.
    pub fn drop_session_asks(&mut self, session_id: &str) {
        let ids: Vec<u64> = self.asks.iter().filter(|(_, a)| a.session_id == session_id).map(|(id, _)| *id).collect();
        for id in ids {
            self.drop_ask(id);
        }
    }

    pub fn on_event(&mut self, raw: &Value, s: &Session, card: Option<&Permission>) {
        if self.chat.load(Ordering::Relaxed) == 0 {
            return;
        }
        // The phone's own prompt starting its turn. Acknowledged even with Away off: the user is on the phone.
        // shortcut: only the last delivered text per session is remembered, so of several sent before the first starts only the last is acknowledged. Upgrade: keep a queue per session.
        if raw["hook_event_name"] == "UserPromptSubmit"
            && self.delivered.get(&s.id).is_some_and(|d| Some(d.trim()) == raw["prompt"].as_str().map(str::trim))
        {
            self.delivered.remove(&s.id);
            if let Some(out) = self.out() {
                let text = format!("{} {}\nWorking on it", circle(s.color), s.project);
                let _ = out.send(Out::Send { session_id: Some(s.id.clone()), text, buttons: vec![], tag: None, entities: vec![] });
            }
            return;
        }
        if !self.away.load(Ordering::Relaxed) {
            return;
        }
        if let Some((text, buttons, entities)) = message(raw, s, card)
            && let Some(out) = self.out()
        {
            let _ = out.send(Out::Send { session_id: Some(s.id.clone()), text, buttons, tag: None, entities });
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

    fn test_ask(session_id: &str) -> Ask {
        Ask { session_id: session_id.into(), questions: vec![json!({"header": "h", "question": "q", "options": []})], answers: Default::default(), picks: vec![], current: 0 }
    }

    #[test]
    fn timed_out_and_ask_msgs_are_capped() {
        let mut p = Phone::default();
        for id in 0..300u64 { p.asks.insert(id, test_ask("s")); p.note_sent(id as i64, Some(&format!("q:{id}:0"))); p.drop_ask(id); }
        assert_eq!(p.timed_out.len(), 256);
        assert!(!p.timed_out.contains(&0) && p.timed_out.contains(&299));
        assert!(p.ask_msgs.len() <= 256 && !p.ask_msgs.contains_key(&0));
    }

    #[test]
    fn forget_drops_session_ask_msgs() {
        let mut p = Phone::default();
        p.asks.insert(1, test_ask("s")); p.note_sent(10, Some("q:1:0"));
        p.forget("s");
        assert!(p.ask_msgs.is_empty());
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
        r["prompt"] = json!("<task-notification>\n<task-id>b</task-id>");
        assert!(msg(&r).is_none());
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
        let (t, b, _) = message(&fx("PermissionRequest"), &sess(Status::Working, vec![]), Some(&c)).unwrap();
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
        let (t, _, _) = message(&fx("Stop"), &sess(Status::Idle, todos), None).unwrap();
        assert_eq!(t, "🟠 X\ndone\n[x] A\n[ ] B");
    }

    #[test]
    fn stop_formats_the_answer_but_not_todos() {
        let todos = vec![Todo { id: "1".into(), subject: "**B**".into(), status: TodoStatus::InProgress }];
        let mut r = fx("Stop");
        r["last_assistant_message"] = json!("**hi**");
        let (t, _, e) = message(&r, &sess(Status::Idle, todos), None).unwrap();
        assert_eq!(t, "🟠 X\nhi\n[ ] **B**");
        assert_eq!(e, vec![json!({"type": "bold", "offset": 5, "length": 2})]);
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
        let (t, _, _) = message(&n, &sess(Status::Waiting(WaitKind::Other), vec![]), None).unwrap();
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
    fn phone_prompt_acknowledged_not_echoed() {
        let (mut p, rx) = paired();
        p.away.store(true, Ordering::Relaxed);
        p.listen("s".into(), File::create("NUL").unwrap());
        assert!(p.deliver("s", "go"));
        let mut r = fx("UserPromptSubmit");
        r["prompt"] = json!(" go\n");
        let s = sess(Status::Working, vec![]);
        p.on_event(&r, &s, None);
        assert!(matches!(rx.try_recv(), Ok(Out::Send { session_id: Some(id), text, .. }) if id == "s" && text == "🟠 X\nWorking on it"));
        p.on_event(&r, &s, None);
        assert!(matches!(rx.try_recv(), Ok(Out::Send { text, .. }) if text == "🟠 X\nYou (PC):  go\n"));
    }

    #[test]
    fn phone_prompt_acknowledged_with_away_off() {
        let (mut p, rx) = paired();
        p.listen("s".into(), File::create("NUL").unwrap());
        assert!(p.deliver("s", "go"));
        let mut r = fx("UserPromptSubmit");
        r["prompt"] = json!("go");
        let s = sess(Status::Working, vec![]);
        p.on_event(&r, &s, None);
        assert!(matches!(rx.try_recv(), Ok(Out::Send { text, .. }) if text == "🟠 X\nWorking on it"));
        p.on_event(&r, &s, None);
        assert!(rx.try_recv().is_err(), "PC prompts stay silent with Away off");
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

    fn sent_texts(rx: &std::sync::mpsc::Receiver<Out>) -> Vec<String> {
        rx.try_iter().filter_map(|o| if let Out::Send { text, .. } = o { Some(text) } else { None }).collect()
    }

    fn fake_run(c: crate::command::Cmd, out: Sender<Out>) {
        let _ = out.send(Out::Send { session_id: None, text: format!("ran {c:?}"), buttons: vec![], tag: None, entities: vec![] });
    }

    #[test]
    fn command_runs_while_away() {
        let (mut p, rx) = paired(); p.away.store(true, Ordering::Relaxed); p.run_cmd = fake_run;
        p.incoming(text(7, "/open agent", None), &two(), Instant::now());
        assert_eq!(sent_texts(&rx), vec!["ran Open([\"agent\"])"]);
    }
    #[test]
    fn command_refused_while_away_off() {
        let (mut p, rx) = paired(); p.run_cmd = fake_run;
        p.incoming(text(7, "/claude agent", None), &two(), Instant::now());
        assert_eq!(sent_texts(&rx), vec!["Turn Away on to use commands"]);
    }
    #[test]
    fn usage_reply_runs_nothing() {
        let (mut p, rx) = paired(); p.away.store(true, Ordering::Relaxed); p.run_cmd = fake_run;
        p.incoming(text(7, "/open", None), &two(), Instant::now());
        assert_eq!(sent_texts(&rx), vec!["Usage: /open <name>"]);
    }
    #[test]
    fn other_slash_text_goes_to_session() {
        let (mut p, rx) = paired(); p.away.store(true, Ordering::Relaxed); p.run_cmd = fake_run;
        p.incoming(text(7, "/clear", None), &two(), Instant::now());
        assert_eq!(sent_texts(&rx), vec!["Which session?"]);
    }
    #[test]
    fn command_from_other_chat_ignored() {
        let (mut p, rx) = paired(); p.away.store(true, Ordering::Relaxed); p.run_cmd = fake_run;
        p.incoming(text(8, "/open agent", None), &two(), Instant::now());
        let (mut u, urx) = unpaired(); u.away.store(true, Ordering::Relaxed); u.run_cmd = fake_run;
        u.incoming(text(7, "/open agent", None), &two(), Instant::now());
        assert!(sent_texts(&rx).is_empty() && sent_texts(&urx).is_empty());
    }
    #[test]
    fn away_command_works_while_away_off() {
        let (mut p, rx) = paired(); p.run_cmd = fake_run;
        assert!(matches!(p.incoming(text(7, "/away on", None), &two(), Instant::now()), Action::Away(true)));
        assert!(sent_texts(&rx).is_empty());
    }
    #[test]
    fn command_for_other_bot_is_plain_text() {
        let (mut p, rx) = paired(); p.away.store(true, Ordering::Relaxed); p.run_cmd = fake_run;
        p.bot = Some("MyBot".into());
        p.incoming(text(7, "/open@OtherBot agent", None), &two(), Instant::now());
        assert_eq!(sent_texts(&rx), vec!["Which session?"]);
        p.incoming(text(7, "/open@mybot agent", None), &two(), Instant::now());
        assert_eq!(sent_texts(&rx), vec!["ran Open([\"agent\"])"]);
    }
    #[test]
    fn forget_gone_drops_pruned_sessions() {
        let mut p = Phone::default();
        p.listen("s".into(), File::create("NUL").unwrap());
        p.listen("t".into(), File::create("NUL").unwrap());
        assert!(p.deliver("s", "a") && p.deliver("t", "b"));
        p.sent.insert(1, "s".into());
        p.sent.insert(2, "t".into());
        p.asks.insert(3, test_ask("s"));
        p.forget_gone(&[sess(Status::Idle, vec![])]);
        assert_eq!((p.listeners.len(), p.delivered.len(), p.sent.len(), p.asks.len()), (1, 1, 1, 1));
        assert!(p.listeners.contains_key("s") && !p.delivered.contains_key("t") && p.sent.get(&1).is_some());
    }
    #[test]
    fn help_works_while_away_off() {
        let (mut p, rx) = paired(); p.run_cmd = fake_run;
        p.incoming(text(7, "/help", None), &two(), Instant::now());
        assert_eq!(sent_texts(&rx), vec![crate::command::HELP]);
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

    fn fake_save(p: &std::path::Path, t: Option<&str>) {
        crate::settings::set(p, "phone_token", t.map_or(Value::Null, |t| json!(t))).unwrap();
    }

    #[test]
    fn pairing_stores_token_and_disconnect_clears_it() {
        let (mut p, _rx) = unpaired();
        let path = std::env::temp_dir().join(format!("ai-pairtok-{}.json", std::process::id()));
        p.settings = Some(path.clone());
        p.save_token = fake_save;
        p.set_token_for_test("t:1");
        let code = p.code.clone();
        p.incoming(text(5, &code, None), &[], Instant::now());
        let paired = crate::settings::load_chat(&path);
        let saved = std::fs::read_to_string(&path).unwrap();
        p.disconnect();
        let after = std::fs::read_to_string(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        assert_eq!(paired, Some(5));
        assert!(saved.contains("\"phone_token\":\"t:1\""), "{saved}");
        assert!(after.contains("\"phone_token\":null"), "{after}");
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
    fn link_problem_replaces_code_until_cleared() {
        let (mut p, _rx) = unpaired();
        p.set_token("t".into());
        let now = Instant::now();
        p.incoming(Incoming::LinkProblem(Some("bot token rejected")), &[], now);
        assert_eq!(p.status(now).pairing_line().unwrap(), "Phone: bot token rejected");
        p.incoming(Incoming::LinkProblem(None), &[], now);
        assert_eq!(p.status(now).pairing_line().unwrap(), format!("Phone: send {} to your bot", p.code));
        p.incoming(Incoming::LinkProblem(Some("bot token rejected")), &[], now);
        p.set_token("new".into());
        assert_eq!(p.status(now).pairing_line().unwrap(), format!("Phone: send {} to your bot", p.code));
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

    fn q(question: &str, header: &str, labels: &[&str], multi: bool) -> Value {
        json!({"question": question, "header": header, "multiSelect": multi, "options": labels.iter().map(|l| json!({"label": l})).collect::<Vec<_>>()})
    }

    fn tap(p: &mut Phone, data: &str) -> Action {
        p.incoming(Incoming::Button { chat: 7, callback_id: "c".into(), data: data.into(), date: i64::MAX }, &two(), Instant::now())
    }

    /// Holds a question set for session `s` and registers each sent message as id 100 + question index.
    fn held(p: &mut Phone, rx: &std::sync::mpsc::Receiver<Out>, id: u64, qs: Vec<Value>) {
        p.away.store(true, Ordering::Relaxed);
        p.ask(id, &sess(Status::Idle, vec![]), qs);
        note(p, rx);
    }

    fn note(p: &mut Phone, rx: &std::sync::mpsc::Receiver<Out>) {
        while let Ok(Out::Send { tag, .. }) = rx.try_recv() {
            p.note_sent(100 + tag.as_deref().and_then(|t| t.rsplit(':').next()).and_then(|n| n.parse::<i64>().ok()).unwrap(), tag.as_deref());
        }
    }

    fn ack_text(rx: &std::sync::mpsc::Receiver<Out>) -> String {
        match rx.try_recv() { Ok(Out::Answer { text, .. }) => text, _ => panic!("no ack") }
    }

    #[test]
    fn question_message_text_and_buttons() {
        let (mut p, rx) = paired();
        p.away.store(true, Ordering::Relaxed);
        p.ask(4, &sess(Status::Idle, vec![]), vec![q("Which?", "Pick", &["A", "B"], true)]);
        let Ok(Out::Send { text, buttons, tag, .. }) = rx.try_recv() else { panic!("no question") };
        assert_eq!(text, "🟠 X
Pick
Which?");
        assert_eq!(buttons, vec![("A".to_owned(), "q:4:0:0".to_owned()), ("B".to_owned(), "q:4:0:1".to_owned()), ("Done".to_owned(), "q:4:0:done".to_owned())]);
        assert_eq!(tag.as_deref(), Some("q:4:0"));
    }

    #[test]
    fn single_select_tap_answers() {
        let (mut p, rx) = paired();
        held(&mut p, &rx, 4, vec![q("Which?", "Pick", &["A", "B"], false)]);
        let Action::Answer(4, Reply::Answer(a), Some(cb)) = tap(&mut p, "q:4:0:1") else { panic!("no answer") };
        assert_eq!((a, cb), (json!({"Which?": "B"}), "c".to_owned()));
        assert!(p.asks.is_empty());
        assert!(matches!(tap(&mut p, "q:4:0:1"), Action::None));
        assert_eq!(ack_text(&rx), "Already answered");
    }

    #[test]
    fn multi_select_toggles_and_done() {
        let (mut p, rx) = paired();
        held(&mut p, &rx, 4, vec![q("Which?", "Pick", &["A", "B", "C"], true)]);
        assert!(matches!(tap(&mut p, "q:4:0:0"), Action::None));
        assert_eq!(ack_text(&rx), "Picked: A");
        tap(&mut p, "q:4:0:2");
        assert_eq!(ack_text(&rx), "Picked: A, C");
        tap(&mut p, "q:4:0:1");
        tap(&mut p, "q:4:0:1");
        assert_eq!((ack_text(&rx), ack_text(&rx)), ("Picked: A, B, C".to_owned(), "Picked: A, C".to_owned()));
        let Action::Answer(4, Reply::Answer(a), Some(_)) = tap(&mut p, "q:4:0:done") else { panic!("no answer") };
        assert_eq!(a, json!({"Which?": "A, C"}));
    }

    #[test]
    fn done_with_nothing_picked_waits() {
        let (mut p, rx) = paired();
        held(&mut p, &rx, 4, vec![q("Which?", "Pick", &["A", "B"], true)]);
        assert!(matches!(tap(&mut p, "q:4:0:done"), Action::None));
        assert_eq!(ack_text(&rx), "Picked: ");
        assert!(p.asks.contains_key(&4));
    }

    #[test]
    fn reply_text_is_other_answer() {
        let (mut p, rx) = paired();
        held(&mut p, &rx, 4, vec![q("Which?", "Pick", &["A", "B"], false)]);
        let a = p.incoming(text(7, "my own", Some(100)), &two(), Instant::now());
        let Action::Answer(4, Reply::Answer(a), None) = a else { panic!("no answer") };
        assert_eq!(a, json!({"Which?": "my own"}));
        // A late reply to the answered question is not typed into the session.
        p.incoming(text(7, "again", Some(100)), &two(), Instant::now());
        assert!(matches!(rx.try_recv(), Ok(Out::Send { text, .. }) if text == "Already answered"));
    }

    #[test]
    fn plain_text_answers_only_waiting_question() {
        let (mut p, rx) = paired();
        held(&mut p, &rx, 4, vec![q("Which?", "Pick", &["A", "B"], false)]);
        let a = p.incoming(text(7, "free", None), &two(), Instant::now());
        assert!(matches!(a, Action::Answer(4, Reply::Answer(ref v), None) if *v == json!({"Which?": "free"})));
    }

    #[test]
    fn two_waiting_questions_ask_which() {
        let (mut p, rx) = paired();
        held(&mut p, &rx, 4, vec![q("Q1?", "H1", &["A"], false)]);
        let mut t = sess(Status::Idle, vec![]);
        t.id = "t".into();
        t.project = "Y".into();
        t.color = 1;
        p.ask(5, &t, vec![q("Q2?", "H2", &["B"], false)]);
        note(&mut p, &rx);
        assert!(matches!(p.incoming(text(7, "free", None), &two(), Instant::now()), Action::None));
        let Ok(Out::Send { text: t, buttons, .. }) = rx.try_recv() else { panic!("no prompt") };
        assert_eq!(t, "Which question?");
        assert_eq!(buttons, vec![("🟠 X: H1".to_owned(), "w:4:0".to_owned()), ("🔵 Y: H2".to_owned(), "w:5:0".to_owned())]);
        let a = tap(&mut p, "w:5:0");
        assert!(matches!(a, Action::Answer(5, Reply::Answer(ref v), Some(_)) if *v == json!({"Q2?": "free"})));
        assert!(p.asks.contains_key(&4));
    }

    #[test]
    fn two_questions_answer_in_order() {
        let (mut p, rx) = paired();
        held(&mut p, &rx, 4, vec![q("Q1?", "H1", &["A", "B"], true), q("Q2?", "H2", &["C", "D"], false)]);
        assert!(rx.try_recv().is_err(), "second question waits for the first answer");
        tap(&mut p, "q:4:0:1");
        assert_eq!(ack_text(&rx), "Picked: B");
        assert!(matches!(tap(&mut p, "q:4:0:done"), Action::None));
        let Ok(Out::Send { text, tag, .. }) = rx.try_recv() else { panic!("no second question") };
        assert_eq!((text.as_str(), tag.as_deref()), ("🟠 X
H2
Q2?", Some("q:4:1")));
        assert_eq!(ack_text(&rx), "");
        let Action::Answer(4, Reply::Answer(a), Some(_)) = tap(&mut p, "q:4:1:0") else { panic!("no answer") };
        assert_eq!(a, json!({"Q1?": "B", "Q2?": "C"}));
    }
}
