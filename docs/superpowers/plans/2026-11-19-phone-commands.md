# Phone Commands While Away Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** (Revised 2026-11-20: see the spec's last section.) While Away is on, the paired Telegram chat can send `/open <name>`, `/claude <name>` and `/help`. The island opens a Windows Terminal tab in the zoxide match, and optionally starts `claude` there. The island keeps running without Claude: the token is DPAPI-encrypted and the island starts at login.

**Architecture:**
- New `src/command.rs`: parsing, folder validation, building the argv, and running the command.
- `Phone::incoming` routes the three verbs to a worker thread. The worker replies through the existing `Out` channel.
- `settings.rs` gains DPAPI token storage and the HKCU Run value. Both are tied to Away.
- `Core::tick` stops exiting while Away is on.

**Tech Stack:** Rust 2024, `windows` 0.62 (new features: `Win32_Security_Cryptography`, `Win32_System_Registry`), std `Command`.

**Spec:** `docs/superpowers/specs/2026-11-19-phone-commands-design.md`

## Global Constraints

- **Branch:** work on `feat/phone-commands`, created from `main`.
- **Commands:** only these verbs are commands: `open`, `claude`, `help`, `away` (`/away on`, `/away off`). They are case-insensitive, and an optional `@BotName` suffix is stripped. Any other `/` text goes to a session as today.
- **No shell:** Telegram text never reaches a shell parser. `<name>` words reach only `zoxide query --` as separate argv items. The only variable passed to `wt.exe` is the validated directory.
- **Gate:** commands are accepted only from the paired chat. `/open` and `/claude` also need Away on. `/away` and `/help` work with Away off.
- **Name limits:** 1 to 8 words, at most 100 characters after trimming, no control characters.
- **Accepted folder:** exit code 0, absolute, an existing directory, and containing no `;` or `"`. Trim one trailing `\` unless the path is a drive root.
- **zoxide:** times out after 5 s, then the process is killed. Spawn it with `CREATE_NO_WINDOW` and stdin null.
- **Launch argv:**
  - `/open`: `wt.exe -w 0 nt -d <dir>`
  - `/claude`: the same plus `powershell.exe -NoLogo -NoExit -Command claude`
  - Never a permission-mode flag.
- **Token at rest:**
  - Stored while a phone is paired, at `settings.json` key `phone_token`, as hex DPAPI (CurrentUser) with entropy `b"agent-islands phone token v1"`.
  - `CRYPTPROTECT_UI_FORBIDDEN`, and `LocalFree` on the output buffer.
- **Run value:** `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`, name `AgentIslands`, data `"<current exe>"`. It exists only while a phone is paired.
- **Never write to disk or logs:** the token or typed commands.
- **Reply texts (exact):**
  - `Turn Away on to use commands`
  - `Away on`
  - `Away off`
  - `Usage: /away on|off`
  - `Usage: /open <name>`
  - `Usage: /claude <name>`
  - `No folder matches <name>`
  - `zoxide is not installed`
  - `Could not look up <name>`
  - `Opened <dir>`
  - `Started claude in <dir>`
  - `Could not open Windows Terminal`
- **Memory caps:** `timed_out` and `ask_msgs` hold at most 256 entries each. Drop the smallest id, since ids increase over time.
- **Tests:** run `cargo test` from the repo root. Every task leaves the whole suite green.

## Review Focus

1. **A name containing shell characters** (`foo" & calc`, `$(x)`, `;`) reaches zoxide only as plain argv words, and nothing else runs. Covered by `run_passes_words_verbatim` (Task 3).
2. **zoxide returns a path containing `;`**, which `wt` would split into two commands, so it must be refused. Covered by `check_dir_rejects_wt_separators` (Task 2).
3. **A `/open` sent with Away off** runs nothing and gets the "Turn Away on" reply. Covered by `command_refused_while_away_off` (Task 3).
4. **Claude's own slash commands** (`/clear`, `/compact`) sent from the phone still reach the session. Covered by `other_slash_text_goes_to_session` (Task 3).
5. **A command from an unpaired chat**, including during pairing, never runs. Covered by `command_from_other_chat_ignored` (Task 3).

---

### Task 1: Spike (gates everything else)

Throwaway. It proves the three risks from the spec before any product code is written. If any check fails, **stop and report to the user**. Don't continue.

**Files:**
- Create: `spike/wt-launch/main.rs` (not part of the crate)

- [ ] **Step 1: Write `spike/wt-launch/main.rs`.**
  - `#![windows_subsystem = "windows"]`.
  - Spawn `wt.exe` with the argv `-w 0 nt -d <arg1> powershell.exe -NoLogo -NoExit -Command claude`.
  - Drop the `Child`.
- [ ] **Step 2: Build and run it detached.**
  - `rustc -O spike/wt-launch/main.rs -o spike/wt-launch/wt-launch.exe`
  - Then in PowerShell: `Start-Process spike\wt-launch\wt-launch.exe -ArgumentList 'D:\projects\agent-islands'`
  - Expected: a new tab in the open Windows Terminal window (or a new window if none is open), using the default profile colors, in `D:\projects\agent-islands`, with `claude` running.
- [ ] **Step 3: Check a drive root.** Run it again with the argument `C:\`. Expected: the tab opens at `C:\`. If not, record it: Task 2 then rejects drive roots, with the reply `Could not open C:\`.
- [ ] **Step 4: Check the trust prompt.** Run it with a new empty folder, for example `%TEMP%\ai-spike-untrusted`.
  - Record whether Claude shows a trust prompt that blocks.
  - If it blocks, the reply in Task 3 for `/claude` gets the suffix ` (accept the folder trust prompt on the PC if it is new)`.
- [ ] **Step 5: Check phone text.** With the island running, paired and Away on, send plain text from the phone to the session from Step 2. Expected: it arrives as a prompt.
- [ ] **Step 6: Write the results** (pass/fail per step) as a `## Spike results` section at the end of the spec. Delete `spike/wt-launch/`. Commit the spec change: `docs: record phone-commands spike results`.

---

### Task 2: Command parsing and folder checks (pure)

**Files:**
- Create: `src/command.rs`
- Modify: `src/main.rs` (add `mod command;`)

**Interfaces:**
- Produces:
  - `pub enum Cmd { Open(Vec<String>), Claude(Vec<String>), Help, Away(bool) }` (derive `Debug, PartialEq, Clone`)
  - `pub fn parse(text: &str) -> Option<Result<Cmd, String>>`
    - `None`: not a command.
    - `Err`: the exact usage reply.
  - `pub fn check_dir(stdout: &str) -> Option<std::path::PathBuf>`
  - `pub fn wt_args(cmd: &Cmd, dir: &std::path::Path) -> Vec<std::ffi::OsString>`
  - `pub const HELP: &str` (the text is in Step 3).

- [ ] **Step 1: Write the failing tests** in a `#[cfg(test)] mod tests` block in `src/command.rs`.

```rust
#[test] fn parses_three_verbs() {
    assert_eq!(parse("/open agent"), Some(Ok(Cmd::Open(vec!["agent".into()]))));
    assert_eq!(parse("  /CLAUDE@MyBot foo  bar "), Some(Ok(Cmd::Claude(vec!["foo".into(), "bar".into()]))));
    assert_eq!(parse("/help"), Some(Ok(Cmd::Help)));
    assert_eq!(parse("/away ON"), Some(Ok(Cmd::Away(true))));
    assert_eq!(parse("/away off"), Some(Ok(Cmd::Away(false))));
    assert_eq!(parse("/away"), Some(Err("Usage: /away on|off".into())));
}
#[test] fn other_text_is_not_a_command() {
    for t in ["/clear", "/compact now", "open agent", "", "/", "/opener x"] { assert_eq!(parse(t), None, "{t}"); }
}
#[test] fn bad_names_get_usage() {
    assert_eq!(parse("/open"), Some(Err("Usage: /open <name>".into())));
    assert_eq!(parse("/claude a b c d e f g h i"), Some(Err("Usage: /claude <name>".into())));
    assert_eq!(parse(&format!("/open {}", "x".repeat(101))), Some(Err("Usage: /open <name>".into())));
    assert_eq!(parse("/open a\u{7}b"), Some(Err("Usage: /open <name>".into())));
}
#[test] fn check_dir_accepts_existing_absolute_dir() {
    let d = std::env::temp_dir();
    assert_eq!(check_dir(&format!("{}\r\n", d.display())), Some(d.clone()));
    assert_eq!(check_dir(&format!("{}\\\n", d.display())), Some(d));
}
#[test] fn check_dir_rejects_wt_separators() {
    for s in ["C:\\a;b\n", "C:\\a\"b\n", "", "relative\\x\n", "C:\\definitely\\missing\\ai\n"] { assert_eq!(check_dir(s), None, "{s}"); }
}
#[test] fn wt_args_are_fixed() {
    let d = std::path::Path::new("D:\\p");
    assert_eq!(wt_args(&Cmd::Open(vec![]), d), ["-w", "0", "nt", "-d", "D:\\p"].map(Into::into).to_vec());
    assert_eq!(wt_args(&Cmd::Claude(vec![]), d), ["-w", "0", "nt", "-d", "D:\\p", "powershell.exe", "-NoLogo", "-NoExit", "-Command", "claude"].map(Into::into).to_vec());
}
```

- [ ] **Step 2: Run the tests.** `cargo test command::` Expected: FAIL (nothing defined yet).
- [ ] **Step 3: Implement the four items.**
  - `parse`:
    - Trim, then require a leading `/`. The first whitespace token, minus any `@...` suffix and lowercased, is the verb.
    - `help` ignores its arguments.
    - For `open`/`claude`, the rest is `split_whitespace`. The trimmed rest must be 1 to 100 chars, with 1 to 8 words, and no `char::is_control`.
  - `check_dir`:
    - Strip trailing `\r`/`\n`. Reject empty input, or input containing `;` or `"`.
    - Strip one trailing `\` when the length is over 3.
    - Require `is_absolute() && is_dir()`.
    - Drive roots: follow Task 1 Step 3's result.
  - `HELP`:

    ```
    /open <name> - new terminal tab in the zoxide match for <name>
    /claude <name> - same, then start claude
    /away on|off - turn Away on or off
    /help - this list
    ```

  - `/away` takes exactly one argument, `on` or `off` (case-insensitive). Anything else is `Usage: /away on|off`.

- [ ] **Step 4: Run the tests.** `cargo test command::` Expected: PASS.
- [ ] **Step 5: Commit.** `feat: parse phone commands and check zoxide folders`

---

### Task 3: Running commands and routing them from the phone

**Files:**
- Modify: `src/command.rs`
- Modify: `src/phone.rs`:
  - add a `run_cmd` field to `Phone` and its `Default`;
  - add a command branch at the top of the paired `Incoming::Text` arm (around line 304).

**Interfaces:**
- Consumes: Task 2's `Cmd`, `parse`, `check_dir`, `wt_args`, `HELP`.
- Produces:
  - `pub enum QueryError { NotInstalled, NoMatch, Failed }`
  - `pub fn run(cmd: &Cmd, query: impl Fn(&[String]) -> Result<String, QueryError>, launch: impl Fn(&[std::ffi::OsString]) -> std::io::Result<()>) -> String`. It returns the reply text.
  - `pub fn spawn(cmd: Cmd, out: std::sync::mpsc::Sender<crate::telegram::Out>)`. It runs `run` with the real zoxide and wt on a new thread, then sends `Out::Send { session_id: None, text, buttons: vec![], tag: None, entities: vec![] }`.
  - `Phone.run_cmd: fn(Cmd, Sender<Out>)`. The default is `command::spawn`.

- [ ] **Step 1: Write the failing tests.** In `src/command.rs`:

```rust
#[test] fn run_passes_words_verbatim() {
    let seen = std::cell::RefCell::new(vec![]);
    let reply = run(&Cmd::Open(vec!["foo\"".into(), "&".into(), "$(x)".into()]),
        |w| { seen.borrow_mut().push(w.to_vec()); Err(QueryError::NoMatch) }, |_| panic!("must not launch"));
    assert_eq!(seen.into_inner(), vec![vec!["foo\"".to_string(), "&".into(), "$(x)".into()]]);
    assert_eq!(reply, "No folder matches foo\" & $(x)");
}
#[test] fn run_replies() {
    let tmp = std::env::temp_dir();
    let ok = |_: &[String]| Ok(format!("{}\n", tmp.display()));
    assert_eq!(run(&Cmd::Open(vec!["a".into()]), ok, |_| Ok(())), format!("Opened {}", tmp.display()));
    assert_eq!(run(&Cmd::Claude(vec!["a".into()]), ok, |_| Ok(())), format!("Started claude in {}", tmp.display()));
    assert_eq!(run(&Cmd::Open(vec!["a".into()]), ok, |_| Err(std::io::ErrorKind::NotFound.into())), "Could not open Windows Terminal");
    assert_eq!(run(&Cmd::Open(vec!["a".into()]), |_| Err(QueryError::NotInstalled), |_| Ok(())), "zoxide is not installed");
    assert_eq!(run(&Cmd::Open(vec!["a".into()]), |_| Err(QueryError::Failed), |_| Ok(())), "Could not look up a");
    assert_eq!(run(&Cmd::Open(vec!["a".into()]), |_| Ok("C:\\x;y\n".into()), |_| panic!()), "No folder matches a");
    assert_eq!(run(&Cmd::Help, |_| panic!(), |_| panic!()), HELP);
}
```

  In the `src/phone.rs` tests, use the existing `paired()` helper. It gives chat 7 and an `Out` receiver. Add a fake runner:

```rust
fn fake_run(c: crate::command::Cmd, out: Sender<Out>) {
    let _ = out.send(Out::Send { session_id: None, text: format!("ran {c:?}"), buttons: vec![], tag: None, entities: vec![] });
}
#[test] fn command_runs_while_away() {
    let (mut p, rx) = paired(); p.away.store(true, Ordering::Relaxed); p.run_cmd = fake_run;
    p.incoming(text(7, "/open agent", None), &two(), Instant::now());
    assert_eq!(sent_texts(&rx), vec!["ran Open([\"agent\"])"]);
}
#[test] fn command_refused_while_away_off() {
    let (mut p, rx) = paired(); p.run_cmd = fake_run;
    p.incoming(text(7, "/claude agent", None), &two(), Instant::now());
    assert_eq!(sent_texts(&rx), vec!["Turn Away on to use commands"]);
}
#[test] fn usage_reply_runs_nothing() {
    let (mut p, rx) = paired(); p.away.store(true, Ordering::Relaxed); p.run_cmd = fake_run;
    p.incoming(text(7, "/open", None), &two(), Instant::now());
    assert_eq!(sent_texts(&rx), vec!["Usage: /open <name>"]);
}
#[test] fn other_slash_text_goes_to_session() {
    let (mut p, rx) = paired(); p.away.store(true, Ordering::Relaxed); p.run_cmd = fake_run;
    p.incoming(text(7, "/clear", None), &two(), Instant::now());
    assert_eq!(sent_texts(&rx), vec!["Which session?"]);
}
#[test] fn command_from_other_chat_ignored() {
    let (mut p, rx) = paired(); p.away.store(true, Ordering::Relaxed); p.run_cmd = fake_run;
    p.incoming(text(8, "/open agent", None), &two(), Instant::now());
    let (mut u, urx) = unpaired(); u.away.store(true, Ordering::Relaxed); u.run_cmd = fake_run;
    u.incoming(text(7, "/open agent", None), &two(), Instant::now());
    assert!(sent_texts(&rx).is_empty() && sent_texts(&urx).is_empty());
}
```

  `sent_texts(&Receiver<Out>) -> Vec<String>` drains `Out::Send` texts with `try_iter`. Add it next to the existing helpers if no equivalent exists. If `two()` is not a two-session fixture, use whichever one is: `/clear` must reach the `Which session?` branch.

- [ ] **Step 2: Run the tests.** `cargo test command:: phone::` Expected: FAIL.
- [ ] **Step 3: Implement `run`.**
  - `Help` returns `HELP`.
  - Otherwise: call `query(words)`, then `check_dir`, then `launch(&wt_args(...))`.
  - `<name>` in replies is `words.join(" ")`. `<dir>` is `dir.display()`.
  - Map errors to the exact replies from Global Constraints. A `check_dir` failure replies `No folder matches <name>`.
- [ ] **Step 4: Implement the real `zoxide` query** (private `fn zoxide(words: &[String]) -> Result<String, QueryError>`).
  - Use `Command::new("zoxide").arg("query").arg("--").args(words)`, with `creation_flags(CREATE_NO_WINDOW)`, stdin null and stdout piped.
  - A spawn `NotFound` error is `NotInstalled`.
  - Poll `try_wait` every 50 ms for up to 5 s. On timeout, `kill()` and `wait()`, then return `Failed`.
  - Exit code 1 is `NoMatch`. Any other non-zero exit is `Failed`. On success, read stdout to a string; non-UTF-8 output is `Failed`.
- [ ] **Step 5: Implement the real launch** (private `fn launch(args: &[OsString]) -> io::Result<()>`). `Command::new("wt.exe").args(args).spawn().map(drop)`.
- [ ] **Step 6: Implement `spawn`** as described under Interfaces.
- [ ] **Step 7: Route in `Phone::incoming`.** In the `Incoming::Text { chat, text, reply_to, .. } if chat == paired` arm, before the existing routing:
  - `match command::parse(&text)`:
    - `None`: fall through.
    - `Some(Err(u))`: `say(u)`.
    - `Some(Ok(Cmd::Away(on)))`: return the new `Action::Away(on)`. Core calls `set_away(on)`, then the phone says `Away on` or `Away off`. It is allowed with Away off.
    - `Some(Ok(Cmd::Open | Cmd::Claude))` with Away off: `say("Turn Away on to use commands")`.
    - `Some(Ok(Cmd::Help))`: always allowed.
    - `Some(Ok(c))`: if `self.out()` is `Some(o)`, call `(self.run_cmd)(c, o.clone())`.
  - Return `Action::None` for every other `Some` case.
  - Add `Action::Away(bool)` to `phone::Action`, and handle it in `Core::handle`'s `Msg::Phone` arm: `self.set_away(on)`, then reply with `Away on`/`Away off` and the actual resulting state (use `Phone::say`, made `pub(crate)`).
  - Extra test, `away_command_works_while_away_off`: with Away off, `/away on` returns `Action::Away(true)` and runs nothing.
  - Add `run_cmd: crate::command::spawn` to `Phone::default()`.
- [ ] **Step 8: Run the tests.** `cargo test` Expected: all pass.
- [ ] **Step 9: Commit.** `feat: run /open, /claude and /help from the phone while away`

---

### Task 4: Bounded phone state

**Files:**
- Modify: `src/phone.rs`: `drop_ask` (around line 485), `note_sent` (around line 475), `forget` (around line 445)

**Interfaces:** none new. `timed_out` stays a `HashSet<u64>` and `ask_msgs` stays a `HashMap<i64, (u64, usize)>`.

- [ ] **Step 1: Write the failing tests** in the `src/phone.rs` tests.

```rust
#[test] fn timed_out_and_ask_msgs_are_capped() {
    let mut p = Phone::default();
    for id in 0..300u64 { p.asks.insert(id, test_ask("s")); p.note_sent(id as i64, Some(&format!("q:{id}:0"))); p.drop_ask(id); }
    assert_eq!(p.timed_out.len(), 256);
    assert!(!p.timed_out.contains(&0) && p.timed_out.contains(&299));
    assert!(p.ask_msgs.len() <= 256 && !p.ask_msgs.contains_key(&0));
}
#[test] fn forget_drops_session_ask_msgs() {
    let mut p = Phone::default();
    p.asks.insert(1, test_ask("s")); p.note_sent(10, Some("q:1:0"));
    p.forget("s");
    assert!(p.ask_msgs.is_empty());
}
```

  `test_ask(session_id: &str) -> Ask` builds an `Ask` with one question and empty answers. It lives in the tests module, which can see private fields.

- [ ] **Step 2: Run the tests.** `cargo test phone::` Expected: FAIL.
- [ ] **Step 3: Implement the caps and cleanup.**
  - `drop_ask`: after the insert, while `timed_out.len() > 256`, remove the minimum.
  - `note_sent`: after the insert, while `ask_msgs.len() > 256`, remove the minimum key.
  - `forget`: collect the ids of the session's asks before the existing `asks.retain`, then `ask_msgs.retain(|_, (id, _)| !ids.contains(id))`.
  - Note: `timed_out` has no link to a session, so the cap alone bounds it. This is a deviation from the spec's "remove by session". It's fine because late taps for forgotten sessions fall back to "Already answered".
- [ ] **Step 4: Run the tests.** `cargo test` Expected: all pass.
- [ ] **Step 5: Commit.** `fix: cap held-question bookkeeping for long-running islands`

---

### Task 5: Token at rest, Run value, and the `forget` subcommand

**Files:**
- Modify: `Cargo.toml`: add the `windows` features `"Win32_Security_Cryptography"` and `"Win32_System_Registry"`.
- Modify: `src/settings.rs`
- Modify: `src/main.rs`: add the `forget` subcommand.

**Interfaces:**
- Produces, in `settings`:
  - `pub fn protect(plain: &str) -> Option<String>`: DPAPI, then lowercase hex.
  - `pub fn unprotect(hex: &str) -> Option<String>`
  - `pub fn load_token(path: &Path) -> Option<String>`: reads `phone_token` and unprotects it.
  - `pub fn set_run(name: &str, exe: Option<&Path>) -> bool`: `Some` writes `"\"<exe>\""` as `REG_SZ` (with `RegSetKeyValueW`); `None` deletes it (`RegDeleteKeyValueW`, where missing counts as success). Returns success.
  - `pub fn get_run(name: &str) -> Option<String>`: `RegGetValueW` with `RRF_RT_REG_SZ`.
  - `pub const RUN_NAME: &str = "AgentIslands";`
  - `pub fn autostart(path: &Path, token: Option<&str>)`:
    - `Some(t)` with `t` non-empty: `set(path, "phone_token", protect(t))` and `set_run(RUN_NAME, Some(&current_exe()))`.
    - `None`: set `phone_token` to null and `set_run(RUN_NAME, None)`.

- [ ] **Step 1: Write the failing tests** in the `src/settings.rs` tests.

```rust
#[test] fn dpapi_roundtrip_and_tamper() {
    let h = protect("123:ABC").unwrap();
    assert!(!h.contains("123:ABC"));
    assert_eq!(unprotect(&h).as_deref(), Some("123:ABC"));
    let mut bad = h.clone(); bad.replace_range(10..12, if &h[10..12] == "00" { "11" } else { "00" });
    assert_eq!(unprotect(&bad), None);
    assert_eq!(unprotect("zz"), None);
}
#[test] fn autostart_stores_and_clears_token() {
    let p = tmp("auto");
    save_to(&p, Side::Right).unwrap();
    autostart_with(&p, Some("t:1"), "AgentIslandsTest");
    assert_eq!(load_token(&p).as_deref(), Some("t:1"));
    assert!(!std::fs::read_to_string(&p).unwrap().contains("t:1"));
    assert!(get_run("AgentIslandsTest").is_some_and(|v| v.starts_with('"') && v.ends_with('"')));
    autostart_with(&p, None, "AgentIslandsTest");
    assert_eq!((load_token(&p), get_run("AgentIslandsTest"), load_from(&p)), (None, None, Side::Right));
}
```

  So that tests never touch the real `AgentIslands` value, `autostart` calls a private `autostart_with(path, token, run_name)`. The tests call that directly.

- [ ] **Step 2: Run the tests.** `cargo test settings::` Expected: FAIL.
- [ ] **Step 3: Implement the items listed under Interfaces.**
  - Entropy: `b"agent-islands phone token v1"`.
  - Flags: `CRYPTPROTECT_UI_FORBIDDEN`.
  - Free the output blob with `LocalFree`.
  - Hex encoding uses std only; no new crate.
- [ ] **Step 4: Add `forget` to `src/main.rs`.** It runs before `ui::run`, next to `hook`/`listen`.
  - If `settings::path()` is `Some(p)`: call `autostart(&p, None)` and `set(&p, "away", false.into())`.
  - Print `Removed the stored phone token and the login entry. If the island is running, close it or turn Away off.`
  - Exit 0.
  - The binary is `windows_subsystem = "windows"`, so stdout may not show. That's acceptable; the README documents the effect.
- [ ] **Step 5: Run the tests.** `cargo test` Expected: all pass. Then `reg query HKCU\Software\Microsoft\Windows\CurrentVersion\Run /v AgentIslandsTest` should report that the value is not found, which confirms the test cleaned up.
- [ ] **Step 6: Commit.** `feat: store the phone token with DPAPI and manage the login entry`

---

### Task 6: Wire pairing to autostart, the exit rule, and the empty pill

**Revised 2026-11-20:** autostart and the "stay running" rule follow **pairing**, not Away. In the steps below, read "Away on" as "paired (chat != 0) with a non-empty token":
- `set_away` no longer touches autostart.
- Pairing (`Paired`), or a hook token arriving while paired, calls `autostart(p, Some(token))`.
- `disconnect` and an empty token call `autostart(p, None)`.
- `tick`, startup reconcile and the empty pill use "paired with a token".
- The test becomes `tick_stays_while_paired_with_no_sessions`: unpairing lets it exit after 3 s.
- The pill outline stays amber only when Away is on.
- The settings card shows `Listening for commands` under Phone when paired.
- The Away sub-line is `Phone commands on; starts at login`.

**Files:**
- Modify: `src/ui/mod.rs`:
  - `Core::handle` (the token branch, around line 39)
  - `Core::set_away` (around line 158)
  - `Core::tick` (around line 194)
  - `run()` (around lines 255-265)
- Modify: `src/phone.rs`: add `pub fn token(&self) -> &str`.
- Modify: `src/ui/views.rs`: `pill` (around line 62)

**Interfaces:**
- Consumes: Task 5's `settings::autostart`, `settings::load_token`.

- [ ] **Step 1: Write the failing tests** in the `src/ui/mod.rs` tests.

```rust
#[test] fn tick_stays_while_away_with_no_sessions() {
    let mut c = paired_core();
    c.set_away(true);
    let t0 = Instant::now();
    assert!(!c.tick(t0, 0));
    assert!(!c.tick(t0 + Duration::from_secs(10), 0));
    c.set_away(false);
    assert!(!c.tick(t0 + Duration::from_secs(11), 0));
    assert!(c.tick(t0 + Duration::from_secs(15), 0));
}
```

  `paired_core()` has `phone.settings == None`, so `set_away` never touches the real registry or settings in tests. Keep it that way: every `autostart` call is guarded by `if let Some(p) = &self.phone.settings`.

- [ ] **Step 2: Run the test.** `cargo test ui::` Expected: FAIL. The second assert fails because `tick` exits.
- [ ] **Step 3: Change the exit rule.** In `tick`, treat `self.phone.away.load(Relaxed)` like non-empty state: reset `empty_since` and return false.
- [ ] **Step 4: Tie autostart to Away.** In `set_away`, after the existing settings write, with `if let Some(p)`:
  - `on`: `settings::autostart(p, Some(self.phone.token()))`.
  - `!on`: `settings::autostart(p, None)`.

  In the `Msg::Event` token branch, after `set_token(t)`: if `t` is non-empty and Away is on, call `settings::autostart(p, Some(&t))`. The existing empty-token path already calls `set_away(false)`, which clears it.
- [ ] **Step 5: Reconcile at startup in `run()`.** Replace the current `away.store(...)` logic.
  - Let `p = settings::path()`.
  - `away_on = chat != 0 && load_away(p)`, plus a token from `load_token(p)`.
  - If both hold: `core.phone.set_token(token)`, `away.store(true)`, `autostart(p, Some(&token))` (this refreshes the exe path after plugin updates).
  - Otherwise: `away.store(false)`, `autostart(p, None)`, and if `load_away(p)`, `settings::set(p, "away", false.into())`.
  - **Order matters:** set the token after `core.phone.chat` and `core.phone.settings` are assigned.
- [ ] **Step 6: Show the empty pill while away.** In `views::pill`, size by `core.state.sessions.len().max(away as usize)`. An empty pill with the amber outline then stays clickable, and the right-click settings card still opens. Then change the Away row's sub-line in `views::settings_card` from `Send everything to Telegram` to `Telegram + phone commands; starts at login`.
- [ ] **Step 7: Run the tests.** `cargo test` Expected: all pass. Update any existing test that asserts the old sub-line string.
- [ ] **Step 8: Commit.** `feat: keep the island running at login while away`

---

### Task 7: README and end-to-end check

**Files:**
- Modify: `README.md`, section `## Away mode` (around line 104)

- [ ] **Step 1: Add a `### Phone commands` subsection.**
  - The three commands, and that they need Away on and only come from the paired phone.
  - That pairing stores the bot token encrypted for your Windows user (DPAPI) and adds the `AgentIslands` login entry. The island then keeps running ("Listening for commands"). Unpairing removes both.
  - The two caveats from the spec:
    - update, then reboot before opening Claude, means no island at login;
    - uninstalling with Away on leaves both behind: run `agent-islands.exe forget` first.
- [ ] **Step 2: Build and do the manual end-to-end check.**
  1. Run `cargo build --release`, then `./build-plugin.ps1` (or the local build steps in the README `## Local build`). Install it locally.
  2. Open a Claude session. Pair the phone. Turn Away on. Then close every Claude session.
  3. Expected: the island stays, showing an empty amber pill. `reg query HKCU\Software\Microsoft\Windows\CurrentVersion\Run /v AgentIslands` shows the exe. `settings.json` has a hex `phone_token` and no plaintext token.
  4. From the phone, send `/open agent-islands`. Expected: a new tab in `D:\projects\agent-islands` and the reply `Opened D:\projects\agent-islands`.
  5. Send `/claude agent-islands`. Expected: Claude starts, and the phone gets `Started` for the new session. A plain text reply reaches that session.
  6. Sign out and back in (or reboot). Expected: the island is running and `/help` replies.
  7. Send `/away off`, then `/away on`, from the phone with no sessions open. Expected: replies `Away off` / `Away on`, and the pill outline switches.
  8. Disconnect on the island. Expected: the island exits within about 3 s. The Run value is gone and `phone_token` is null.
- [ ] **Step 3: Soak check.** With Away on and no sessions, send `/help` 50 times over an hour, plus a few `/open` commands. Task Manager private bytes should stay within a few MB of the starting value. Record the numbers in the spec's `## Spike results` section.
- [ ] **Step 4: Run the suite.** `cargo test` Expected: all pass.
- [ ] **Step 5: Commit.** `docs: phone commands and login start`
