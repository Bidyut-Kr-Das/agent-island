use crate::server::Msg;
use serde_json::{Value, json};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::Duration;

pub enum Out {
    Send { session_id: Option<String>, text: String, buttons: Vec<(String, String)> },
    Answer { callback_id: String, text: String },
}

pub enum Incoming {
    /// `date` is the message's unix time in seconds.
    Text { chat: i64, text: String, reply_to: Option<i64>, date: i64 },
    /// `date` is that of the message carrying the button.
    Button { chat: i64, callback_id: String, data: String, date: i64 },
    BotName(String),
}

pub fn parse_update(u: &Value) -> Option<Incoming> {
    if let Some(m) = u.get("message") {
        return Some(Incoming::Text {
            chat: m["chat"]["id"].as_i64()?,
            text: m["text"].as_str()?.to_owned(),
            reply_to: m["reply_to_message"]["message_id"].as_i64(),
            date: m["date"].as_i64().unwrap_or(0),
        });
    }
    let q = u.get("callback_query")?;
    Some(Incoming::Button {
        chat: q["message"]["chat"]["id"].as_i64()?,
        callback_id: q["id"].as_str()?.to_owned(),
        data: q["data"].as_str()?.to_owned(),
        date: q["message"]["date"].as_i64().unwrap_or(0),
    })
}

/// Telegram caps a message at 4096 chars; stay well under.
pub fn cut(text: &str) -> String {
    match text.char_indices().nth(3500) {
        Some((i, _)) => format!("{}... (full answer in the terminal)", &text[..i]),
        None => text.to_owned(),
    }
}

/// Dropping the Link stops both threads (the poller after its current getUpdates, at most ~60 s).
pub struct Link {
    pub out: Sender<Out>,
    stop: Arc<AtomicBool>,
}

impl Drop for Link {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

/// POSTs to a bot method. Errors carry no detail: the URL contains the token.
fn call(agent: &ureq::Agent, token: &str, method: &str, body: &Value) -> Result<Value, ()> {
    let url = format!("https://api.telegram.org/bot{token}/{method}");
    let v = agent.post(&url).send_json(body).map_err(|_| ())?.body_mut().read_json::<Value>().map_err(|_| ())?;
    if v["ok"].as_bool() == Some(true) { Ok(v["result"].clone()) } else { Err(()) }
}

pub fn start(token: String, chat: Arc<AtomicI64>, tx: Sender<Msg>) -> Link {
    let stop = Arc::new(AtomicBool::new(false));
    let agent: ureq::Agent = ureq::Agent::config_builder().timeout_global(Some(Duration::from_secs(60))).build().into();
    let (out, rx) = channel();
    {
        let (agent, token, tx, stop) = (agent.clone(), token.clone(), tx.clone(), stop.clone());
        std::thread::spawn(move || poll(&agent, &token, &tx, &stop));
    }
    std::thread::spawn(move || send_loop(&agent, &token, &chat, &tx, rx));
    Link { out, stop }
}

fn deliver(tx: &Sender<Msg>, m: Msg) -> bool {
    let ok = tx.send(m).is_ok();
    crate::ui::wake();
    ok
}

fn poll(agent: &ureq::Agent, token: &str, tx: &Sender<Msg>, stop: &AtomicBool) {
    let (mut offset, mut named, mut wait) = (0i64, false, 1u64);
    while !stop.load(Ordering::Relaxed) {
        let got = (|| {
            if !named {
                let me = call(agent, token, "getMe", &json!({}))?;
                named = true;
                deliver(tx, Msg::Phone(Incoming::BotName(me["username"].as_str().unwrap_or_default().to_owned())));
            }
            call(agent, token, "getUpdates", &json!({"offset": offset, "timeout": 50, "allowed_updates": ["message", "callback_query"]}))
        })();
        if stop.load(Ordering::Relaxed) {
            return;
        }
        match got {
            Ok(v) => {
                wait = 1;
                for u in v.as_array().into_iter().flatten() {
                    offset = offset.max(u["update_id"].as_i64().unwrap_or(0) + 1);
                    if let Some(i) = parse_update(u)
                        && !deliver(tx, Msg::Phone(i))
                    {
                        return;
                    }
                }
            }
            Err(()) => {
                std::thread::sleep(Duration::from_secs(wait));
                wait = (wait * 2).min(60);
            }
        }
    }
}

fn send_loop(agent: &ureq::Agent, token: &str, chat: &AtomicI64, tx: &Sender<Msg>, rx: Receiver<Out>) {
    // Ends when the Link (the only Sender) is dropped.
    for o in rx {
        let (method, body, session) = match o {
            Out::Send { session_id, text, buttons } => {
                let id = chat.load(Ordering::Relaxed);
                if id == 0 {
                    continue;
                }
                let mut b = json!({"chat_id": id, "text": cut(&text)});
                if !buttons.is_empty() {
                    let row: Vec<Value> = buttons.iter().map(|(l, d)| json!({"text": l, "callback_data": d})).collect();
                    b["reply_markup"] = json!({"inline_keyboard": [row]});
                }
                ("sendMessage", b, session_id)
            }
            Out::Answer { callback_id, text } => ("answerCallbackQuery", json!({"callback_query_id": callback_id, "text": text}), None),
        };
        for _ in 0..3 {
            if let Ok(r) = call(agent, token, method, &body) {
                if let (Some(session_id), Some(message_id)) = (session, r["message_id"].as_i64()) {
                    deliver(tx, Msg::Sent { message_id, session_id });
                }
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_reply_text() {
        let u = json!({"update_id": 1, "message": {"message_id": 9, "chat": {"id": 5}, "text": "go", "date": 1700, "reply_to_message": {"message_id": 3}}});
        assert!(matches!(parse_update(&u), Some(Incoming::Text { chat: 5, ref text, reply_to: Some(3), date: 1700 }) if text == "go"));
    }

    #[test]
    fn parse_button() {
        let u = json!({"update_id": 2, "callback_query": {"id": "q", "data": "a:4", "message": {"chat": {"id": 5}, "date": 1800}}});
        assert!(matches!(parse_update(&u), Some(Incoming::Button { chat: 5, date: 1800, .. })));
    }

    #[test]
    fn parse_ignores_non_text() {
        assert!(parse_update(&json!({"update_id": 3, "message": {"message_id": 1, "chat": {"id": 5}, "voice": {}}})).is_none());
    }

    #[test]
    fn cut_long_multibyte_text() {
        let s = "é".repeat(5000);
        let c = cut(&s);
        assert!(c.ends_with("... (full answer in the terminal)"));
        assert!(c.chars().count() <= 3500 + 40);
        assert_eq!(cut("short"), "short");
    }
}
