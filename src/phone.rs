use std::collections::HashMap;
use std::fs::File;
use std::io::Write;
use std::sync::Arc;
use std::sync::atomic::AtomicI64;
use std::sync::mpsc::Sender;
use crate::{server::Msg, telegram::{self, Link, Out}};

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

    #[allow(dead_code)] // used from Task 8
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
    }
}
