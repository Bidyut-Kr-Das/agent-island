# Telegram Phone Link Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Mirror every Claude Code session to one paired Telegram chat and let a reply there become that session's next real user prompt.

**Architecture:** The island grows a Telegram link (one long-poll thread, one send thread) and a `Phone` state machine in its core. A new exe mode `agent-islands.exe listen <session_id> <cwd>` holds a connection on the existing named pipe for each session; a plugin mod spawns it and turns each line it prints into `$.prompt.submit({ text, asUser: true })`. The token arrives with every hook envelope from `CLAUDE_PLUGIN_OPTION_TELEGRAM_BOT_TOKEN`.

**Tech Stack:** Rust 2024, egui/eframe, `windows` 0.62, serde_json, new dep `ureq` 3 (features `json`, default rustls). Claude Code mod in TypeScript (`hooks/phone.ts`).

**Spec:** `docs/superpowers/specs/2026-11-17-telegram-phone-design.md`

## Global Constraints

- Off by default: no network call of any kind until a non-empty bot token arrives.
- Only the paired chat id gets updates or can drive a session; other chats are ignored with no reply (except an exact pairing code while unpaired).
- One new Telegram message per event; never edit or delete a sent message.
- Plain text only. First line of every session message: color circle + session name (`state::display_parts(cwd).1`). No emoji other than the circle.
- Circles by palette index: 0 `🟠`, 1 `🔵`, 2 `🟣`, 3 `🔴`, 4 `🟢`, 5 `⚫`.
- Bodies cut to 3500 chars, then `... (full answer in the terminal)`.
- Phone prompts arrive as the user's own prompt (`asUser: true`). No launch flag.
- No polling on the PC side except Telegram's held `getUpdates` (timeout 50 s).
- Permission window stays 30 s (`server.rs` unchanged).
- Copy strings exactly: `Started`, `You (PC): `, `Permission: `, `Allow`, `Deny`, `Error: `, `Waiting for you: `, `Ended`, `Paired`, `Which session?`, `<name> can't receive messages right now`, `Queued: <name> is busy`, `Already answered`, `Too late, answered in the terminal`.
- Commit only on a feature branch `feat/telegram-phone`, never on `main`.

## Review Focus

1. A phone prompt with newlines, quotes, backslashes or emoji must reach the session byte-identical (lines are JSON strings). Tests: Task 3 `deliver_encodes_multiline_as_one_json_line`, Task 4 `listen_prints_lines_and_reconnects`.
2. A body over 3500 chars with multi-byte text must be cut on a char boundary, never panic. Test: Task 6 `cut_long_multibyte_text`.
3. Island restarted while sessions sit idle: helpers reconnect and the island must not exit-and-respawn in a loop. Test: Task 3 `listen_for_unknown_session_creates_it`, plus Task 10 manual step.
4. Strangers and guessing: messages from other chats are silent; 5 wrong codes renew the code. Tests: Task 8 `stranger_is_ignored`, `five_wrong_codes_renew`.
5. A button tap for a card already answered, timed out, or from before an island restart must not answer anything else. Test: Task 8 `stale_button_taps`.

---

### Task 1: Manifest check and plugin config

Settles the spec's "left to the builder" question before anything else: can `hooks/hooks.json` hold `modules` next to `hooks`, and does a module's `classic.SessionStart` hook see `session_id` and `cwd`.

**Files:**
- Modify: `plugin/.claude-plugin/plugin.json`
- Modify: `plugin/hooks/hooks.json`
- Create: `plugin/hooks/phone.ts` (stub)

**Interfaces:**
- Produces: `userConfig.telegram_bot_token` (type `string`, `sensitive: true`, not required, title `Telegram bot token`, description `Optional. Lets you follow and answer sessions from Telegram. Leave blank to keep it off.`); env var `CLAUDE_PLUGIN_OPTION_TELEGRAM_BOT_TOKEN` in hook processes; module path `./phone.ts`.

- [ ] **Step 1:** `git switch -c feat/telegram-phone` (also move `3929e33` here only if the user asks).
- [ ] **Step 2:** Add `"modules": ["./phone.ts"]` as a top-level key in `plugin/hooks/hooks.json` beside `"hooks"`. Stub `phone.ts`: `register` adds `on('classic.SessionStart', ...)` that calls `$.ui.log(JSON.stringify({ sid: e.session_id, cwd: e.cwd }), { to: 'debug' })` and returns `next(e)`.
- [ ] **Step 3:** Add the `userConfig` field to `plugin.json`.
- [ ] **Step 4:** Run `claude plugin validate plugin`. Expected: no errors, module listed. If `modules` beside `hooks` is refused, move the module to the shape validate accepts and record the shape in this task before continuing.
- [ ] **Step 5:** Run `claude --plugin-dir .\plugin --debug`, exit, and grep `~/.claude/debug/<session>.txt` for the logged `sid`. Expected: the real session id and cwd. Then set a token under `/plugin` and confirm a hook sees the env var (temporary `$env:...` dump is fine, remove after).
- [ ] **Step 6:** Commit `feat: plugin config for the phone link`.

### Task 2: Settings read-modify-write and paired chat

**Files:**
- Modify: `src/settings.rs`, `src/ui/mod.rs` (the `save_to` call site)

**Interfaces:**
- Produces: `settings::set(path: &Path, key: &str, v: serde_json::Value) -> io::Result<()>` (reads the object, replaces one key, writes; unreadable file starts from `{}`); `settings::load_chat(path: &Path) -> Option<i64>` (key `phone_chat`). `save_to(path, side)` becomes `set(path, "side", ...)`; `load_from` unchanged.

- [ ] **Step 1: Failing tests**

```rust
#[test]
fn set_keeps_other_keys() {
    let p = tmp("rmw");
    save_to(&p, Side::Right).unwrap();
    set(&p, "phone_chat", serde_json::json!(42)).unwrap();
    assert_eq!((load_from(&p), load_chat(&p)), (Side::Right, Some(42)));
    save_to(&p, Side::Left).unwrap();
    assert_eq!(load_chat(&p), Some(42));
    set(&p, "phone_chat", serde_json::Value::Null).unwrap();
    assert_eq!(load_chat(&p), None);
}

#[test]
fn set_over_garbage_starts_fresh() {
    let p = tmp("rmwg");
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(&p, "not json").unwrap();
    set(&p, "phone_chat", serde_json::json!(7)).unwrap();
    assert_eq!(load_chat(&p), Some(7));
}
```

- [ ] **Step 2:** `cargo test settings` — FAIL (`set` not found).
- [ ] **Step 3:** Implement `set` and `load_chat`; rewrite `save_to` on top of `set`.
- [ ] **Step 4:** `cargo test settings` — PASS (old tests too).
- [ ] **Step 5:** Commit `refactor: settings.json read-modify-write`.

### Task 3: `listen` envelope and per-session listeners in the island

**Files:**
- Create: `src/phone.rs` (listener part only in this task)
- Modify: `src/server.rs`, `src/ui/mod.rs` (`Core`), `src/main.rs` (`mod phone;`)

**Interfaces:**
- Consumes: `procs::find_pids(&chain)`, `State::apply`.
- Produces:
  - Envelope `{"listen": "<session_id>", "cwd": "<cwd>", "chain": [...]}`; `serve` sends `Msg::Listen { session_id: String, cwd: String, chain: Vec<(u32, String)>, pipe: std::fs::File }` and returns without closing the file.
  - `#[derive(Default)] pub struct Phone` in `src/phone.rs` with `pub fn listen(&mut self, session_id: String, pipe: File)` (newest wins) and `pub fn deliver(&mut self, session_id: &str, text: &str) -> bool` (writes `serde_json::to_string(text)` + `\n`; on write error drops the listener and returns false).
  - `Core.phone: Phone`. `Core::handle(Msg::Listen ..)`: if the session is unknown, apply a synthetic `SessionStart` (`session_id`, `cwd`, empty transcript) with `find_pids(&chain)` so the island keeps it; if known, only fill pids. Then `phone.listen(..)`. No Telegram message for this synthetic start (Task 7 sends only from `Msg::Event`).
  - `Phone` drops the listener on `SessionEnd` (`fn forget(&mut self, session_id: &str)`), called from `Core::handle`.

- [ ] **Step 1: Failing tests** (in `src/ui/mod.rs` tests; build the `File` with a real pipe pair like `server.rs` tests do, helper `pipe_pair(tag) -> (server_file, client_file)`)

```rust
#[test]
fn listen_for_unknown_session_creates_it() {
    let mut c = Core::new(channel().0);
    let (srv, _cli) = pipe_pair("lu");
    c.handle(Msg::Listen { session_id: "s".into(), cwd: r"D:\p\x".into(), chain: vec![], pipe: srv }, 1);
    assert_eq!(c.state.sessions.len(), 1);
    assert_eq!(c.state.sessions[0].project, "X");
}

#[test]
fn deliver_encodes_multiline_as_one_json_line() {
    let mut c = Core::new(channel().0);
    let (srv, cli) = pipe_pair("ml");
    c.handle(Msg::Listen { session_id: "s".into(), cwd: "x".into(), chain: vec![], pipe: srv }, 1);
    let text = "line 1\nsay \"hi\" \\ 🟠";
    assert!(c.phone.deliver("s", text));
    let mut line = String::new();
    BufReader::new(&cli).read_line(&mut line).unwrap();
    assert_eq!(serde_json::from_str::<String>(line.trim_end()).unwrap(), text);
}

#[test]
fn deliver_to_closed_listener_fails_and_forgets() {
    let mut c = Core::new(channel().0);
    let (srv, cli) = pipe_pair("cl");
    c.handle(Msg::Listen { session_id: "s".into(), cwd: "x".into(), chain: vec![], pipe: srv }, 1);
    drop(cli);
    assert!(!c.phone.deliver("s", "a"));
    assert!(!c.phone.deliver("s", "a"));
}
```

Plus in `server.rs` tests: `listen_envelope_hands_pipe_to_core` sends `{"listen":"s","cwd":"x","chain":[]}` and asserts `Msg::Listen { session_id: "s", .. }`.

- [ ] **Step 2:** `cargo test` — FAIL (no `Msg::Listen`).
- [ ] **Step 3:** Implement. In `serve`, branch on `v["listen"].as_str()` before the event path.
- [ ] **Step 4:** `cargo test` — PASS.
- [ ] **Step 5:** Commit `feat: island keeps a listener per session`.

### Task 4: `listen` exe mode and the mod

**Files:**
- Create: `src/listen.rs`
- Modify: `src/main.rs` (dispatch `listen`), `src/hook.rs` (make `connect_retry` `pub(crate)`), `plugin/hooks/phone.ts`
- Test: `tests/island.rs`

**Interfaces:**
- Consumes: `hook::connect_retry(name, spawn: true)`, `pipe::pipe_name()`, `procs::ancestors(std::process::id())`, envelope from Task 3.
- Produces: `listen::run(session_id: &str, cwd: &str) -> i32`. Loop: connect (spawning the island if absent), write the envelope, copy each received line to stdout and flush; on EOF or error sleep 1 s and reconnect. Exits only when stdout write fails (Claude Code gone). `pub(crate) fn serve_lines(name: &str, sid: &str, cwd: &str, out: &mut impl Write, spawn: bool) -> io::Result<()>` holds one connection, so the test can drive it.
- Mod `phone.ts`: on `classic.SessionStart` (fires for startup, resume and `/clear`, which changes the id) end the previous child's loop, then `$.process.spawn({ argv: [\`${$.plugin.root}/bin/agent-islands.exe\`, 'listen', e.session_id, e.cwd] })`; buffer stdout pieces, split on `\n`, `void $.prompt.submit({ text: JSON.parse(line), asUser: true })` per line; ignore lines that fail to parse. The loop runs detached as in the `$.process.spawn` doc example ("a child for the session's life").

- [ ] **Step 1: Failing test** in `src/listen.rs`: `listen_prints_lines_and_reconnects`, a `pipe::Server` on a test pipe name and `serve_lines(name, "s", "x", &mut Vec<u8>, false)` called twice. Assert: first line received by the server is the envelope with `listen == "s"`; after the server writes `"\"a\\nb\""\n`, the output buffer holds that exact line; after the server drops the connection and accepts again, a second envelope arrives.
- [ ] **Step 2:** `cargo test listen` — FAIL.
- [ ] **Step 3:** Implement `listen.rs` and the `main.rs` dispatch (`Some("listen")` with args 2 and 3).
- [ ] **Step 4:** `cargo test` — PASS.
- [ ] **Step 5:** Write the final `phone.ts`; `claude plugin validate plugin` and `tsc -p plugin` (once the engine laid its types) — clean.
- [ ] **Step 6:** Manual: add a debug-only test hook `AGENT_ISLANDS_ECHO=<text>` (`#[cfg(debug_assertions)]`, like `AGENT_ISLANDS_CLICK`) that delivers `<text>` to the first listener 2 s after it registers. Build debug, copy it to `plugin/bin`, run `claude --plugin-dir .\plugin` with the env var set. Expected: the text shows in the terminal as your prompt line and a turn starts. Then `.\build-plugin.ps1` to restore the release exe.
- [ ] **Step 7 (also):** `Get-Process agent-islands` during the session shows one extra `listen` process per session; it is gone after `/exit`.
- [ ] **Step 8:** Commit `feat: listen mode and phone mod`.

### Task 5: Token in the hook envelope

**Files:**
- Modify: `src/hook.rs` (`forward`), `src/server.rs` (`Msg::Event` gets `token: Option<String>`), `src/ui/mod.rs`

**Interfaces:**
- Produces: envelope key `"token"` present only when `CLAUDE_PLUGIN_OPTION_TELEGRAM_BOT_TOKEN` is set (empty string means "turned off"). `Msg::Event { raw, chain, reply, token }`. `Core::handle` calls `self.phone.set_token(token)` when `Some`. `Phone::set_token(&mut self, t: String)` stores it; Task 6 makes a change start or stop the link.

- [ ] **Step 1: Failing tests:** `hook.rs` `token_rides_in_envelope_when_set` (set the env var in-process via a `forward_with_token(name, event, token: Option<&str>)` seam rather than mutating process env); `server.rs` `token_is_passed_to_core`.
- [ ] **Step 2:** `cargo test` — FAIL.
- [ ] **Step 3:** Implement. Existing tests construct `Msg::Event` — add `token: None`.
- [ ] **Step 4:** `cargo test` — PASS.
- [ ] **Step 5:** Commit `feat: hook passes the bot token to the island`.

### Task 6: Telegram link

**Files:**
- Create: `src/telegram.rs`
- Modify: `Cargo.toml` (`ureq = { version = "3", features = ["json"] }`), `src/main.rs`, `src/phone.rs`, `src/ui/mod.rs` (`pub(crate) fn wake()` reading `CTX`)

**Interfaces:**
- Produces:
  - `pub enum Out { Send { session_id: Option<String>, text: String, buttons: Vec<(String, String)> /* (label, callback_data) */ }, Answer { callback_id: String, text: String } }`
  - `pub enum Incoming { Text { chat: i64, text: String, reply_to: Option<i64> }, Button { chat: i64, callback_id: String, data: String }, BotName(String) }`
  - `server::Msg::Phone(Incoming)` and `Msg::Sent { message_id: i64, session_id: String }`.
  - `pub fn parse_update(u: &Value) -> Option<Incoming>` (pure).
  - `pub fn cut(text: &str) -> String` (3500 chars, char-safe, suffix `... (full answer in the terminal)`).
  - `pub struct Link { pub out: Sender<Out> }` with `Drop` setting a stop flag. `pub fn start(token: String, chat: Arc<AtomicI64>, tx: Sender<Msg>) -> Link`: poller thread (`getMe` once, then `getUpdates` with `offset`, `timeout: 50`, `allowed_updates: ["message","callback_query"]`, agent `timeout_global` 60 s; errors back off 1, 2, 4 ... 60 s; exits when stopped) and sender thread (`sendMessage` with `chat_id` from `chat`, skipped while 0; `reply_markup.inline_keyboard` one row of buttons; posts `Msg::Sent`; `answerCallbackQuery`; 3 tries then drop). Both call `crate::ui::wake()` after each `tx.send`.
  - `Phone::set_token` now: empty or changed token drops the old `Link`; non-empty starts a new one. `Phone` sends through `self.link.as_ref().map(|l| &l.out)`; in tests `Phone::with_out(tx: Sender<Out>)` bypasses the network.

- [ ] **Step 1: Failing tests** (`src/telegram.rs`):

```rust
#[test]
fn parse_reply_text() {
    let u = json!({"update_id": 1, "message": {"message_id": 9, "chat": {"id": 5}, "text": "go", "reply_to_message": {"message_id": 3}}});
    assert!(matches!(parse_update(&u), Some(Incoming::Text { chat: 5, ref text, reply_to: Some(3) }) if text == "go"));
}

#[test]
fn parse_button() {
    let u = json!({"update_id": 2, "callback_query": {"id": "q", "data": "a:4", "message": {"chat": {"id": 5}}}});
    assert!(matches!(parse_update(&u), Some(Incoming::Button { chat: 5, .. })));
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
```

- [ ] **Step 2:** `cargo test telegram` — FAIL.
- [ ] **Step 3:** Implement. No test touches the network.
- [ ] **Step 4:** `cargo test` — PASS; `cargo build --release` still links.
- [ ] **Step 5:** Commit `feat: Telegram link`.

### Task 7: Outgoing messages

**Files:**
- Modify: `src/phone.rs`, `src/ui/mod.rs` (`Core::handle` calls `phone.on_event`)

**Interfaces:**
- Consumes: `Out::Send`, `telegram::cut`, `state::Session`, `state::Permission`.
- Produces:
  - `pub fn circle(color: usize) -> &'static str`.
  - `pub fn message(raw: &Value, s: &Session, card: Option<&Permission>) -> Option<(String, Vec<(String, String)>)>`: pure; header `"{circle} {s.project}\n"` + body per the spec's Outgoing table; buttons `("Allow", "a:<req>")`, `("Deny", "d:<req>")` for a card. Stop body = `last_assistant_message` then one line per todo, `[x] ` for completed else `[ ] `. StopFailure body = `Error: ` + first string of `raw["error"]`, `raw["error_details"]`, else `turn failed`. Waiting body for a `Notification` that sets `Waiting(Other)` and for AskUserQuestion (`tool_input.questions[0].question`). Body passes through `cut` before the header is added.
  - `Phone::on_event(&mut self, raw: &Value, s: &Session, card: Option<&Permission>)`: skips everything while unpaired; skips a `UserPromptSubmit` whose trimmed `prompt` equals the trimmed text last delivered to that session (then forgets it); otherwise sends `Out::Send { session_id: Some(s.id) .. }`.
  - `Core::handle` passes a clone of the session taken before `apply` for `SessionEnd` (apply removes it), the post-apply session otherwise, and the new card when `apply` returned a req id. `Msg::Listen` never calls `on_event`.
  - `Core::handle(Msg::Sent { message_id, session_id })` stores `phone.sent.insert(message_id, session_id)`; `forget` drops a session's entries.

- [ ] **Step 1: Failing tests**, one per table row, in `src/phone.rs`, using `tests/fixtures/*.json` where one exists:
  - `started_ended_and_pc_prompt` — SessionStart gives `"🟠 X\nStarted"`; `UserPromptSubmit` with prompt `hi` gives `"🟠 X\nYou (PC): hi"`; SessionEnd gives `"🟠 X\nEnded"`.
  - `card_has_allow_deny_buttons` — body `Permission: Wants to run a command npm i`, buttons `[("Allow","a:1"),("Deny","d:1")]`.
  - `stop_lists_todos` — `last_assistant_message: "done"`, todos completed `A`, in-progress `B` gives `"done\n[x] A\n[ ] B"`.
  - `stop_failure_and_waiting` — StopFailure and AskUserQuestion bodies.
  - `tool_calls_send_nothing` — PreToolUse (ungated), PostToolUse, permission_prompt Notification all `None`.
  - `indigo_is_black_circle` — `circle(5) == "⚫"`, `circle(3) == "🔴"`.
  - `phone_prompt_not_echoed` — after `deliver("s", "go")`, `on_event` for `UserPromptSubmit` `go` sends nothing; a second identical prompt is sent.
  - `nothing_sent_while_unpaired`.
- [ ] **Step 2:** `cargo test phone` — FAIL.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** `cargo test` — PASS.
- [ ] **Step 5:** Commit `feat: phone messages for session events`.

### Task 8: Incoming messages, pairing, buttons

**Files:**
- Modify: `src/phone.rs`, `src/ui/mod.rs` (`Core::handle(Msg::Phone ..)`, `Msg::Timeout` records the id), `src/ui/mod.rs` `Core::answer` unchanged

**Interfaces:**
- Consumes: `Core::answer(req_id, Reply)`, `Core.replies`, `settings::set`.
- Produces:
  - `Phone` fields: `chat: Option<i64>`, `code: String` (6 digits from `RandomState` hash, `{:06}`), `code_at: Instant`, `wrong: u32`, `bot: Option<String>`, `held: Option<String>`, `timed_out: HashSet<u64>`.
  - `pub enum Action { None, Answer(u64, Reply) }`; `Phone::incoming(&mut self, m: Incoming, sessions: &[Session], now: Instant) -> Action` holds all routing; `Core` performs `Answer` via `self.answer` only if `self.replies` has the id, else replies `Too late, answered in the terminal` when the id is in `timed_out`, else `Already answered`. Pairing writes `settings::set(path, "phone_chat", chat)`; `Core` gets the settings path as `Option<PathBuf>` field (None in tests).
  - Rules (spec Incoming section): unpaired + text equal to `code` pairs and sends `Paired`; unpaired wrong text increments `wrong`; `wrong >= 5` or `code_at` older than 10 min makes a new code and resets both. Paired + other chat: ignored. Reply with known `reply_to`: deliver to that session. Otherwise: zero sessions, `No open sessions`; one session, deliver there; several, hold the text and send `Which session?` with one button per session (`"{circle} {project}"`, `"s:<session_id>"`). Button `s:<id>` delivers the held text. Deliver failure: `<name> can't receive messages right now`; success while the session's status is not `Idle`: `Queued: <name> is busy`.
  - `Phone::disconnect(&mut self)`: `chat = None`, new code, settings key set to `null`.
  - `Phone::status(&self) -> PhoneStatus` with `pub enum PhoneStatus { Off, Pairing { code: String, bot: Option<String> }, Paired }` for the UI.

- [ ] **Step 1: Failing tests** in `src/phone.rs` (drive with `Phone::with_out`, read `Out` from the receiver):
  - `exact_code_pairs` — wrong chat text no, exact code yes, `Out::Send` text `Paired`, `chat == Some(5)`.
  - `stranger_is_ignored` — paired to 5; text from 6 sends nothing and delivers nothing.
  - `five_wrong_codes_renew` — after 5 wrong texts the code differs; the old code no longer pairs.
  - `code_expires_after_10_min` — `incoming` with `now + 601 s` and the old code does not pair.
  - `reply_routes_to_session` — `sent` maps 3 to `s`; reply_to 3 delivers to `s`'s listener.
  - `single_session_takes_plain_text` and `several_sessions_ask_which` — buttons carry `s:<id>`; tapping delivers the held text.
  - `no_listener_says_cant_receive` and `busy_session_says_queued`.
  - `stale_button_taps` (in `src/ui/mod.rs` tests) — `a:<id>` answers once and the hook gets `Allow`; the same tap again replies `Already answered`; a timed-out id replies `Too late, answered in the terminal`; `a:999` replies `Already answered` and answers nothing.
- [ ] **Step 2:** `cargo test` — FAIL.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** `cargo test` — PASS.
- [ ] **Step 5:** Commit `feat: phone replies, pairing and buttons`.

### Task 9: Island UI and wiring

**Files:**
- Modify: `src/ui/mod.rs` (`run`: load `phone_chat`, pass settings path to `Core`), `src/ui/views.rs` (`panel_rows`, `panel`, `settings_card`)

**Interfaces:**
- Consumes: `Phone::status`, `Phone::disconnect`.
- Produces:
  - `panel_rows(s: &Session, phone: &PhoneStatus) -> Vec<(RowIcon, String)>`: appends one `(RowIcon::None, "Phone: send <code> to @<bot>")` row while `Pairing` (`"Phone: send <code> to your bot"` until `getMe` answers). Nothing for `Off` or `Paired`. This is the spec's pairing line on the island panel.
  - `settings_card(ui, pill, side, paused, paired: bool) -> (Rect, Option<Side>, Option<bool>, bool /* disconnect clicked */)`. The settings card is the island's right-click menu. When `paired`, one extra row `Phone` with one `Disconnect` button in the first button slot; otherwise the card is unchanged.

- [ ] **Step 1: Failing tests** in `views.rs` tests:
  - `pairing_line_only_while_pairing` — `panel_rows(&idle, &PhoneStatus::Pairing { code: "482913".into(), bot: Some("mybot".into()) })` ends with `(RowIcon::None, "Phone: send 482913 to @mybot")`; with `bot: None` ends with `"Phone: send 482913 to your bot"`; `Off` and `Paired` add nothing.
  - `settings_card_height_grows_only_when_paired` — extract `fn settings_h(paired: bool) -> f32`; `settings_h(false) == 15.0 + 20.0 + 12.0 + 28.0 + 12.0 + 28.0 + 15.0`, `settings_h(true) == settings_h(false) + 12.0 + 28.0`.
- [ ] **Step 2:** `cargo test views` — FAIL.
- [ ] **Step 3:** Implement and wire `disconnect`. Update existing `panel_rows` tests to pass `&PhoneStatus::Off`.
- [ ] **Step 4:** `cargo test` — PASS.
- [ ] **Step 5:** Commit `feat: phone pairing line and disconnect`.

### Task 10: End to end with a real bot, docs

**Files:**
- Modify: `README.md` (setup section: create a bot with @BotFather, paste the token in `/plugin` > agent-islands > configure, right-click the island, send the code), `CHANGELOG.md`

- [ ] **Step 1:** `cargo test` and `cargo clippy` clean; `.\build-plugin.ps1`.
- [ ] **Step 2:** Manual run, one item per spec promise, each checked on the phone:
  1. No token: no Telegram traffic (Resource Monitor shows no connection from `agent-islands.exe`).
  2. Token set: the island panel shows `Phone: send <code> to @<bot>`; sending it gives `Paired`.
  3. Two sessions: `Started` from each with different circles.
  4. Prompt typed at the PC gives `You (PC): ...`; Stop gives the answer and todos.
  5. Reply to a Stop message in an idle session: the terminal shows it as your prompt line and a turn starts; no `You (PC)` echo arrives.
  6. Reply during a turn: `Queued: <name> is busy`, runs after the turn.
  7. Gated command: Allow from the phone runs it; tap again gives `Already answered`; a card left 30 s gives `Too late, answered in the terminal`.
  8. Non-reply with two sessions: `Which session?`, button delivers.
  9. Kill the island (`Stop-Process`): helpers restart it within a few seconds, sessions reappear, a reply still lands (plain text path), and the island does not restart in a loop (watch for 2 min).
  10. `/clear` in a session, then reply: lands in the cleared session.
  11. Close a session: `Ended`; `Get-Process agent-islands` shows no leftover `listen` process.
  12. Disconnect: no more messages; the code shows again.
  13. Clear the token: no more traffic after the next hook event.
- [ ] **Step 3:** Update README and CHANGELOG.
- [ ] **Step 4:** Commit `docs: phone link setup`.
