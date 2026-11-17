use std::collections::HashMap;
use std::fs::File;
use std::io::Write;

/// Phone link state: one open `listen` pipe per session, each line written is one phone prompt.
#[derive(Default)]
pub struct Phone {
    listeners: HashMap<String, File>,
}

impl Phone {
    /// Newest listener wins; the older one is dropped (its helper sees EOF).
    pub fn listen(&mut self, session_id: String, pipe: File) {
        self.listeners.insert(session_id, pipe);
    }

    /// Writes `text` as one JSON string line. A dead listener is dropped and false returned.
    pub fn deliver(&mut self, session_id: &str, text: &str) -> bool {
        let Some(mut f) = self.listeners.remove(session_id) else { return false };
        let line = format!("{}\n", serde_json::to_string(text).unwrap_or_default());
        if f.write_all(line.as_bytes()).is_err() {
            return false;
        }
        self.listeners.insert(session_id.to_owned(), f);
        true
    }

    pub fn forget(&mut self, session_id: &str) {
        self.listeners.remove(session_id);
    }
}
