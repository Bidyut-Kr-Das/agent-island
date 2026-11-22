use crate::server::Msg;
use pulldown_cmark::{CodeBlockKind, Event, Options, Parser, Tag, TagEnd};
use serde_json::{Value, json};
use std::cmp::Reverse;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::Duration;

pub enum Out {
    /// `entities` are Telegram formatting spans over `text`, from `render` and `place`.
    Send { session_id: Option<String>, text: String, buttons: Vec<(String, String)>, tag: Option<String>, entities: Vec<Value> },
    Answer { callback_id: String, text: String },
    /// Rewrites a sent message without its buttons; `entities` are those the message already had.
    Edit { message_id: i64, text: String, entities: Vec<Value> },
}

/// The button tap and the message it was on.
pub struct Tap {
    pub callback_id: String,
    pub message_id: i64,
    /// Empty when Telegram sent none.
    pub text: String,
    pub entities: Vec<Value>,
}

pub enum Incoming {
    /// `date` is the message's unix time in seconds.
    Text { chat: i64, text: String, reply_to: Option<i64>, date: i64 },
    /// `date` is that of the message carrying the button.
    Button { chat: i64, data: String, date: i64, tap: Tap },
    BotName(String),
    /// Why the link is failing; None once it works again.
    LinkProblem(Option<&'static str>),
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
        data: q["data"].as_str()?.to_owned(),
        date: q["message"]["date"].as_i64().unwrap_or(0),
        tap: Tap {
            callback_id: q["id"].as_str()?.to_owned(),
            message_id: q["message"]["message_id"].as_i64().unwrap_or(0),
            text: q["message"]["text"].as_str().unwrap_or_default().to_owned(),
            entities: q["message"]["entities"].as_array().cloned().unwrap_or_default(),
        },
    })
}

/// Telegram caps a message at 4096 chars; stay well under.
pub const MAX_CHARS: usize = 3500;
/// Telegram formats at most this many entities per message.
pub const MAX_ENTITIES: usize = 100;
/// Telegram's hard cap on a message, in UTF-16 units.
pub const MAX_UNITS: usize = 4096;
/// Length of the suffix `cut_to` appends.
pub const CUT_SUFFIX: usize = 33;

pub fn cut(text: &str) -> String {
    cut_to(text, MAX_CHARS)
}

/// Keeps at most `max` UTF-16 units of `text`, then the "full answer" suffix.
pub fn cut_to(text: &str, max: usize) -> String {
    let mut units = 0;
    for (i, c) in text.char_indices() {
        units += c.len_utf16();
        if units > max {
            return format!("{}... (full answer in the terminal)", &text[..i]);
        }
    }
    text.to_owned()
}

/// Plain text being built, with entity offsets in UTF-16 units as Telegram counts them.
#[derive(Default)]
struct Doc {
    text: String,
    units: usize,
    /// Just wrote a list marker: the item's first block stays on its line.
    fresh: bool,
    /// Start offset of each open span; None for a span with no entity.
    open: Vec<(usize, Option<Value>)>,
    done: Vec<Value>,
}

impl Doc {
    fn push(&mut self, s: &str) {
        self.text.push_str(s);
        self.units += s.encode_utf16().count();
        self.fresh = false;
    }

    fn gap(&mut self) {
        if !self.fresh && !self.text.is_empty() {
            while !self.text.ends_with("\n\n") {
                self.push("\n");
            }
        }
    }

    fn line(&mut self) {
        if !self.text.is_empty() && !self.text.ends_with('\n') {
            self.push("\n");
        }
    }

    fn trim_newlines(&mut self) {
        while self.text.ends_with('\n') {
            self.text.pop();
            self.units -= 1;
        }
    }

    /// Telegram does not allow `code` and `pre` inside other entities.
    fn plain(&self) -> bool {
        self.open.iter().all(|(_, e)| e.is_none())
    }

    fn start(&mut self, entity: Option<Value>) {
        self.open.push((self.units, entity));
    }

    fn close(&mut self) {
        if let Some((offset, Some(mut e))) = self.open.pop()
            && self.units > offset
        {
            e["offset"] = json!(offset);
            e["length"] = json!(self.units - offset);
            self.done.push(e);
        }
    }

    fn code(&mut self, kind: &str, s: &str) {
        self.start(self.plain().then(|| json!({"type": kind})));
        self.push(s);
        self.close();
    }
}

/// Claude's Markdown as plain text plus Telegram entities. Entities need no escaping, so odd text cannot make Telegram reject the message.
// shortcut: Markdown escapes apply, so a Windows path outside backticks loses the backslash before punctuation (`\.claude`). Upgrade: restore it from the source range if this shows up in real answers.
pub fn render(md: &str) -> (String, Vec<Value>) {
    let kind = |k: &str| Some(json!({"type": k}));
    let mut d = Doc::default();
    let mut lists: Vec<Option<u64>> = vec![];
    let mut in_table = false;
    for (ev, range) in Parser::new_ext(md, Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH).into_offset_iter() {
        if in_table {
            in_table = ev != Event::End(TagEnd::Table);
            continue;
        }
        match ev {
            Event::Start(Tag::Paragraph | Tag::HtmlBlock) => d.gap(),
            Event::Start(Tag::Heading { .. }) => {
                d.gap();
                d.start(kind("bold"));
            }
            Event::Start(Tag::BlockQuote(_)) => {
                d.gap();
                // Telegram does not nest blockquotes.
                let nested = d.open.iter().any(|(_, e)| e.as_ref().is_some_and(|e| e["type"] == "blockquote"));
                d.start(if nested { None } else { kind("blockquote") });
            }
            Event::Start(Tag::CodeBlock(k)) => {
                d.gap();
                let mut e = d.plain().then(|| json!({"type": "pre"}));
                if let (Some(e), CodeBlockKind::Fenced(info)) = (&mut e, &k)
                    && let Some(lang) = info.split_whitespace().next()
                {
                    e["language"] = json!(lang);
                }
                d.start(e);
            }
            Event::Start(Tag::Strong) => d.start(kind("bold")),
            Event::Start(Tag::Emphasis) => d.start(kind("italic")),
            Event::Start(Tag::Strikethrough) => d.start(kind("strikethrough")),
            // Claude often links local files; those mean nothing on the phone, so only the label stays.
            Event::Start(Tag::Link { dest_url, .. }) => {
                let web = dest_url.starts_with("https://") || dest_url.starts_with("http://");
                d.start(web.then(|| json!({"type": "text_link", "url": &*dest_url})));
            }
            Event::Start(Tag::List(first)) => {
                if lists.is_empty() {
                    d.gap();
                }
                lists.push(first);
            }
            Event::Start(Tag::Item) => {
                d.line();
                d.push(&"  ".repeat(lists.len().saturating_sub(1)));
                let marker = match lists.last_mut() {
                    Some(Some(n)) => {
                        *n += 1;
                        format!("{}. ", *n - 1)
                    }
                    _ => "• ".to_owned(),
                };
                d.push(&marker);
                d.fresh = true;
            }
            // Telegram has no tables: keep the source aligned in a code block.
            Event::Start(Tag::Table(_)) => {
                d.gap();
                d.code("pre", md[range].trim_end());
                in_table = true;
            }
            Event::End(TagEnd::CodeBlock) => {
                d.trim_newlines();
                d.close();
            }
            Event::End(TagEnd::Heading(_) | TagEnd::BlockQuote(_) | TagEnd::Strong | TagEnd::Emphasis | TagEnd::Strikethrough | TagEnd::Link) => d.close(),
            Event::End(TagEnd::List(_)) => {
                lists.pop();
            }
            Event::Text(t) | Event::Html(t) | Event::InlineHtml(t) => d.push(&t),
            Event::Code(t) => d.code("code", &t),
            Event::SoftBreak | Event::HardBreak => d.push("\n"),
            _ => {}
        }
    }
    (d.text, d.done)
}

/// Fits `render` entities to the sent message `head` + `cut(body)`, where the rendered text starts `body`.
#[cfg(test)]
pub fn place(entities: Vec<Value>, head: &str, body: &str) -> Vec<Value> {
    place_to(entities, head, body, MAX_CHARS)
}

/// `place` for a body cut with `cut_to(body, max)`.
pub fn place_to(entities: Vec<Value>, head: &str, body: &str, max: usize) -> Vec<Value> {
    let shift = head.encode_utf16().count();
    let mut kept = 0;
    for c in body.chars() {
        if kept + c.len_utf16() > max {
            break;
        }
        kept += c.len_utf16();
    }
    let mut placed: Vec<Value> = entities
        .into_iter()
        .filter_map(|mut e| {
            let start = e["offset"].as_u64()? as usize;
            let end = (start + e["length"].as_u64()? as usize).min(kept);
            (start < end).then(|| {
                e["offset"] = json!(start + shift);
                e["length"] = json!(end - start);
                e
            })
        })
        .collect();
    placed.sort_by_key(|e| (e["offset"].as_u64(), Reverse(e["length"].as_u64())));
    placed.truncate(MAX_ENTITIES);
    placed
}

/// Collapses everything after the first paragraph (or third line) of `text` into an expandable quote, never splitting a code block.
pub fn fold(text: &str, mut entities: Vec<Value>) -> Vec<Value> {
    let units: Vec<u16> = text.encode_utf16().collect();
    let mut newlines = 0;
    let Some(mut split) = units.iter().enumerate().find_map(|(i, &u)| {
        newlines += (u == 10) as usize;
        (u == 10 && (newlines == 3 || i > 0 && units[i - 1] == 10)).then_some(i + 1)
    }) else { return entities };
    if String::from_utf16_lossy(&units[split..]).trim().is_empty() {
        return entities;
    }
    let span = |e: &Value| {
        let o = e["offset"].as_u64().unwrap_or(0) as usize;
        (o, o + e["length"].as_u64().unwrap_or(0) as usize)
    };
    if let Some(start) = entities.iter().filter(|e| matches!(e["type"].as_str(), Some("pre" | "code"))).map(span).find(|&(o, end)| o < split && split < end).map(|(o, _)| o) {
        if start == 0 {
            return entities;
        }
        split = start;
    }
    let mut folded = vec![];
    for e in entities.drain(..) {
        let (o, end) = span(&e);
        // Telegram forbids a quote inside the expandable quote.
        let quote = e["type"] == "blockquote";
        if quote && o >= split {
            continue;
        }
        if o < split && split < end {
            for (a, b) in [(o, split), (split, end)] {
                if quote && a == split {
                    continue;
                }
                folded.push(Value::Object(e.as_object().cloned().unwrap_or_default().into_iter().chain([("offset".into(), json!(a)), ("length".into(), json!(b - a))]).collect()));
            }
        } else {
            folded.push(e);
        }
    }
    folded.push(json!({"type": "expandable_blockquote", "offset": split, "length": units.len() - split}));
    folded
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

const UNREACHABLE: &str = "can't reach Telegram";

/// Short enough for the settings card. Telegram answers 401 or 404 to a bad token, 409 when a webhook or another poller has the bot.
fn problem(e: &ureq::Error) -> &'static str {
    match e {
        ureq::Error::StatusCode(401 | 404) => "bot token rejected",
        ureq::Error::StatusCode(409) => "bot used by another app",
        _ => UNREACHABLE,
    }
}

/// POSTs to a bot method. Errors are fixed strings: the URL contains the token.
fn call(agent: &ureq::Agent, token: &str, method: &str, body: &Value) -> Result<Value, &'static str> {
    let url = format!("https://api.telegram.org/bot{token}/{method}");
    let v = agent.post(&url).send_json(body).map_err(|e| problem(&e))?.body_mut().read_json::<Value>().map_err(|_| UNREACHABLE)?;
    if v["ok"].as_bool() == Some(true) { Ok(v["result"].clone()) } else { Err(UNREACHABLE) }
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
    let mut shown: Option<&'static str> = None;
    while !stop.load(Ordering::Relaxed) {
        let got = (|| {
            if !named {
                let me = call(agent, token, "getMe", &json!({}))?;
                named = true;
                // Replaces the / menu a previous app (e.g. openclaw) left on the bot. Private-chat scope wins over default, so set both.
                for scope in ["default", "all_private_chats"] {
                    let _ = call(agent, token, "setMyCommands", &json!({"commands": menu(), "scope": {"type": scope}}));
                }
                deliver(tx, Msg::Phone(Incoming::BotName(me["username"].as_str().unwrap_or_default().to_owned())));
            }
            // No long poll while failing, so a recovery clears the problem at once.
            let timeout = if shown.is_some() { 0 } else { 50 };
            call(agent, token, "getUpdates", &json!({"offset": offset, "timeout": timeout, "allowed_updates": ["message", "callback_query"]}))
        })();
        if stop.load(Ordering::Relaxed) {
            return;
        }
        let now = got.as_ref().err().copied();
        if now != shown {
            shown = now;
            deliver(tx, Msg::Phone(Incoming::LinkProblem(now)));
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
            Err(_) => {
                std::thread::sleep(Duration::from_secs(wait));
                wait = (wait * 2).min(60);
            }
        }
    }
}

/// Bot menu entries from the `/help` text, so the two never disagree.
fn menu() -> Vec<Value> {
    crate::command::HELP.lines().filter_map(|l| {
        let (usage, about) = l.split_once(" - ")?;
        let name = usage.strip_prefix('/')?.split(' ').next()?;
        Some(json!({"command": name, "description": about}))
    }).collect()
}

/// Allow/Deny share one row; longer button lists get one button per row so labels stay readable.
fn keyboard(buttons: &[(String, String)]) -> Vec<Vec<Value>> {
    let all: Vec<Value> = buttons.iter().map(|(l, d)| json!({"text": l, "callback_data": d})).collect();
    if all.len() > 2 { all.into_iter().map(|b| vec![b]).collect() } else { vec![all] }
}

fn send_loop(agent: &ureq::Agent, token: &str, chat: &AtomicI64, tx: &Sender<Msg>, rx: Receiver<Out>) {
    // Ends when the Link (the only Sender) is dropped.
    for o in rx {
        let (method, mut body, session, tag) = match o {
            Out::Send { session_id, text, buttons, tag, entities } => {
                let id = chat.load(Ordering::Relaxed);
                if id == 0 {
                    continue;
                }
                // Text with entities is already cut; cutting again would move the end they were fitted to.
                let mut b = if entities.is_empty() {
                    json!({"chat_id": id, "text": cut(&text)})
                } else {
                    json!({"chat_id": id, "text": text, "entities": entities})
                };
                if !buttons.is_empty() {
                    b["reply_markup"] = json!({"inline_keyboard": keyboard(&buttons)});
                }
                ("sendMessage", b, session_id, tag)
            }
            Out::Answer { callback_id, text } => ("answerCallbackQuery", json!({"callback_query_id": callback_id, "text": text}), None, None),
            Out::Edit { message_id, text, entities } => {
                let id = chat.load(Ordering::Relaxed);
                if id == 0 {
                    continue;
                }
                ("editMessageText", json!({"chat_id": id, "message_id": message_id, "text": text, "entities": entities}), None, None)
            }
        };
        for attempt in 0..3 {
            // A rejected format must not lose the answer: the last try goes out plain.
            if attempt == 2
                && let Some(b) = body.as_object_mut()
            {
                b.remove("entities");
            }
            if let Ok(r) = call(agent, token, method, &body) {
                if let (Some(session_id), Some(message_id)) = (session, r["message_id"].as_i64()) {
                    deliver(tx, Msg::Sent { message_id, session_id, tag });
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
    fn http_status_names_the_problem() {
        assert_eq!(problem(&ureq::Error::StatusCode(401)), "bot token rejected");
        assert_eq!(problem(&ureq::Error::StatusCode(404)), "bot token rejected");
        assert_eq!(problem(&ureq::Error::StatusCode(409)), "bot used by another app");
        assert_eq!(problem(&ureq::Error::StatusCode(502)), UNREACHABLE);
    }

    #[test]
    fn menu_lists_every_help_command() {
        let names: Vec<_> = menu().iter().map(|c| c["command"].as_str().unwrap().to_owned()).collect();
        assert_eq!(names, ["open", "claude", "away", "help"]);
        assert!(menu().iter().all(|c| !c["description"].as_str().unwrap().is_empty()));
    }

    #[test]
    fn keyboard_rows() {
        let b = |n: usize| (0..n).map(|i| (i.to_string(), format!("d{i}"))).collect::<Vec<_>>();
        assert_eq!(keyboard(&b(2)).len(), 1);
        assert_eq!(keyboard(&b(3)).iter().map(Vec::len).collect::<Vec<_>>(), vec![1, 1, 1]);
    }

    fn quote(o: usize, l: usize) -> Value {
        json!({"type": "expandable_blockquote", "offset": o, "length": l})
    }

    #[test]
    fn fold_short_reply_has_no_quote() {
        assert!(fold("a\nb", vec![]).is_empty());
    }

    #[test]
    fn fold_quotes_after_first_paragraph() {
        assert!(fold("r1\nr2\n\nmore\nmore", vec![]).contains(&quote(7, 9)));
    }

    #[test]
    fn fold_caps_visible_at_three_lines() {
        assert_eq!(fold("1\n2\n3\n4\n5", vec![]), vec![quote(6, 3)]);
    }

    #[test]
    fn fold_never_splits_code() {
        let (text, ents) = render("intro\n\n```\nl1\nl2\nl3\nl4\n```\n\nend");
        let pre = ents.iter().find(|e| e["type"] == "pre").unwrap().clone();
        let (po, pl) = (pre["offset"].as_u64().unwrap(), pre["length"].as_u64().unwrap());
        let out = fold(&text, ents);
        let q = out.iter().find(|e| e["type"] == "expandable_blockquote").unwrap();
        assert!(q["offset"].as_u64().unwrap() <= po);
        assert!(out.iter().filter(|e| e["type"] != "expandable_blockquote").all(|e| e == &pre || !(po < e["offset"].as_u64().unwrap() && e["offset"].as_u64().unwrap() < po + pl)));
    }

    #[test]
    fn parse_button() {
        let u = json!({"update_id": 2, "callback_query": {"id": "q", "data": "a:4", "message": {"message_id": 9, "chat": {"id": 5}, "date": 1800, "text": "hi", "entities": [{"type":"bold","offset":0,"length":2}]}}});
        let Some(Incoming::Button { chat: 5, date: 1800, tap, .. }) = parse_update(&u) else { panic!("no button") };
        assert_eq!((tap.callback_id.as_str(), tap.message_id, tap.text.as_str(), tap.entities.len()), ("q", 9, "hi", 1));
        let u = json!({"update_id": 2, "callback_query": {"id": "q", "data": "a:4", "message": {"message_id": 9, "chat": {"id": 5}, "date": 1800}}});
        let Some(Incoming::Button { tap, .. }) = parse_update(&u) else { panic!("no button") };
        assert_eq!(tap.text, "");
    }

    #[test]
    fn parse_ignores_non_text() {
        assert!(parse_update(&json!({"update_id": 3, "message": {"message_id": 1, "chat": {"id": 5}, "voice": {}}})).is_none());
    }

    /// Rendered text, and each entity's type with the text it covers (read through UTF-16 offsets, as Telegram does).
    fn spans(md: &str) -> (String, Vec<(String, String)>) {
        let (text, entities) = render(md);
        let units: Vec<u16> = text.encode_utf16().collect();
        let mut found: Vec<(String, String)> = place(entities, "", &text).iter().map(|e| {
            let (o, l) = (e["offset"].as_u64().unwrap() as usize, e["length"].as_u64().unwrap() as usize);
            (e["type"].as_str().unwrap().to_owned(), String::from_utf16_lossy(&units[o..o + l]))
        }).collect();
        found.sort();
        (text, found)
    }

    fn span(kind: &str, text: &str) -> (String, String) {
        (kind.to_owned(), text.to_owned())
    }

    #[test]
    fn render_inline_styles() {
        let (t, e) = spans("**b** *i* ~~s~~ `c`");
        assert_eq!(t, "b i s c");
        assert_eq!(e, vec![span("bold", "b"), span("code", "c"), span("italic", "i"), span("strikethrough", "s")]);
    }

    #[test]
    fn render_leaves_ordinary_symbols_alone() {
        assert_eq!(spans("snake_case_name takes ~5 min, ~10 at most, 2 * 3 * 4"), ("snake_case_name takes ~5 min, ~10 at most, 2 * 3 * 4".to_owned(), vec![]));
    }

    #[test]
    fn render_applies_markdown_escapes_to_bare_paths() {
        assert_eq!(render(r"C:\Users\me\.claude").0, r"C:\Users\me.claude");
        assert_eq!(render(r"`C:\Users\me\.claude`").0, r"C:\Users\me\.claude");
    }

    #[test]
    fn render_counts_emoji_as_two_units() {
        let (t, e) = render("🟠 **b**");
        assert_eq!(t, "🟠 b");
        assert_eq!((e[0]["offset"].as_u64(), e[0]["length"].as_u64()), (Some(3), Some(1)));
        // The "🟠 X\n" header adds 5 units.
        assert_eq!(place(e, "🟠 X\n", &t)[0]["offset"], 8);
    }

    #[test]
    fn render_blocks_and_lists() {
        let (t, e) = spans("# Title\n\ntext\n\n- a\n  - b\n\n1. c\n2. d\n\n> quote");
        assert_eq!(t, "Title\n\ntext\n\n• a\n  • b\n\n1. c\n2. d\n\nquote");
        assert_eq!(e, vec![span("blockquote", "quote"), span("bold", "Title")]);
    }

    #[test]
    fn render_code_is_never_nested() {
        assert_eq!(spans("**a `b`**"), ("a b".to_owned(), vec![span("bold", "a b")]));
        assert_eq!(spans("> ```\n> x\n> ```"), ("x".to_owned(), vec![span("blockquote", "x")]));
    }

    #[test]
    fn render_unclosed_fence_keeps_language() {
        let (t, e) = render("```rust\nfn a() {}");
        assert_eq!(t, "fn a() {}");
        assert_eq!((e[0]["type"].as_str(), e[0]["language"].as_str(), e[0]["length"].as_u64()), (Some("pre"), Some("rust"), Some(9)));
    }

    #[test]
    fn render_links_only_for_the_web() {
        let (t, e) = render("[phone.rs](src/phone.rs) [site](https://example.com)");
        assert_eq!(t, "phone.rs site");
        assert_eq!(e.len(), 1);
        assert_eq!((&e[0]["type"], &e[0]["url"], &e[0]["offset"]), (&json!("text_link"), &json!("https://example.com"), &json!(9)));
    }

    #[test]
    fn render_table_stays_as_source() {
        let table = "| a | b |\n|---|---|\n| 1 | 2 |";
        let (t, e) = spans(&format!("{table}\n\nafter"));
        assert_eq!(t, format!("{table}\n\nafter"));
        assert_eq!(e, vec![span("pre", table)]);
    }

    #[test]
    fn place_fits_entities_to_the_cut() {
        let body = "x".repeat(MAX_CHARS + 10);
        let e = |offset: usize, length: usize| json!({"type": "bold", "offset": offset, "length": length});
        let placed = place(vec![e(MAX_CHARS - 1, 5), e(MAX_CHARS + 1, 2)], "", &body);
        assert_eq!(placed, vec![e(MAX_CHARS - 1, 1)]);
        assert_eq!(place((0..150).map(|i| e(i, 1)).collect(), "", &body).len(), MAX_ENTITIES);
    }

    #[test]
    fn fold_drops_blockquote_inside_the_expandable_quote() {
        let (text, ents) = render("intro\n\nmore\n\n> quoted\n\nend");
        assert!(ents.iter().any(|e| e["type"] == "blockquote"));
        let out = fold(&text, ents);
        let q = out.iter().find(|e| e["type"] == "expandable_blockquote").unwrap();
        let split = q["offset"].as_u64().unwrap();
        assert!(out.iter().filter(|e| e["type"] == "blockquote").all(|e| e["offset"].as_u64().unwrap() + e["length"].as_u64().unwrap() <= split));
    }

    #[test]
    fn cut_to_counts_utf16_units() {
        assert_eq!(cut_to("ab😀cd", 4), "ab😀... (full answer in the terminal)");
        assert_eq!(cut_to("ab😀cd", 3), "ab... (full answer in the terminal)");
        assert_eq!("... (full answer in the terminal)".len(), CUT_SUFFIX);
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
