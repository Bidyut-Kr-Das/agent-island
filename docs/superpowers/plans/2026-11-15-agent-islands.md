# Agent Islands Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A Windows "dynamic island" that shows one blob mascot per open Claude Code session, its todo progress, permission Allow/Deny cards, and "Go to terminal" for other waits, driven entirely by Claude Code plugin hooks.

**Architecture:** One Rust executable, `agent-islands.exe`. `agent-islands.exe hook` is the hook client: it reads hook JSON from stdin, forwards it over a per-user named pipe, and for `PermissionRequest` waits for the decision. `agent-islands.exe` with no arguments is the UI: an always-on-top transparent egui window that owns a pure state machine (`state.rs`) fed by a pipe-server thread. The hook client starts the UI on demand; the UI exits 3 s after the last session closes.

**Tech Stack:** Rust 1.98 (2024 edition), `eframe`/`egui` (glow backend, no default features beyond what is needed), `serde` + `serde_json`, `windows` crate (Win32 pipes, processes, windows, UI Automation). No other dependencies without a reason written in the commit.

**Spec:** `docs/superpowers/specs/2026-11-15-agent-islands-design.md` (read it first; section numbers below refer to it). Visual references: `docs/superpowers/mockups/island-left.html` (placement and layout; wins on conflicts) and `docs/superpowers/mockups/island-blob.html` (blob, type, colors, cards).

## Global Constraints

- Windows 11 only; Windows Terminal is the only terminal handled for "Go to terminal".
- Claude Code 2.1.325. Hook JSON arrives on stdin; every hook input has `session_id`, `cwd`, `transcript_path`, `hook_event_name`.
- Island process under 30 MB working set; hook client under 20 ms when the island is running (only the launching call may take up to about 2 s).
- Repaint only while something animates; at most 30 fps; at most 10 Hz cursor polling when nothing animates.
- Pipe name `\\.\pipe\agent-islands-<USERNAME>`, current-user only, remote clients rejected.
- A hook prints a decision only for `PermissionRequest`, only after the user clicked, and only after verifying the pipe server's exe path equals its own. Every failure path prints nothing and exits 0.
- Permission wait: 30 s in the island; `PermissionRequest` hook timeout 40 s in `hooks.json`; all other hooks 5 s.
- Deny message text: `Denied from Agent Islands`.
- Fonts: Geist 400/500/600 and Geist Mono 400, embedded with `include_bytes!`, OFL license file shipped beside them.
- Colors: bg `#0c0c0e`, border `rgba(255,255,255,.07)`, text `#f2f2f4`, secondary `#b4b4bc`, muted `#8a8a94`; sessions `#ff6b35 #1e9bff #b85cff #ff3d8b #14c8c0 #5b6cff` in that order; ok `#2bd45f`, warn `#ffb800`, deny `#ff4d4d`.
- Radii: collapsed pill fully round; panel and cards 8 px; buttons and command box 4 px.
- Plugin root path contains spaces (`C:\Users\Bidyut Kr. Das\...`): every hook command must quote the exe path.
- No emojis in UI text, code, comments, commits or docs.

## Review Focus

1. Hook arrives while the island is in its 3 s exit grace or just exiting: the hook must reconnect or respawn, never hang Claude Code, never print a decision. (Task 4 test `connect_failure_prints_nothing`.)
2. Two `PermissionRequest`s at once from one session (parallel tool calls): both queue, oldest first, and each answer goes to its own hook. (Task 1 test `two_permissions_same_session_queue_in_order`.)
3. Huge or multi-line `tool_input` (heredoc Bash command, long path): detail line is one line, cut with an ellipsis. (Task 2 test `multiline_command_uses_first_line_with_ellipsis`.)
4. Events for a session that never sent `SessionStart` (plugin installed mid-session): session is created on its first event of any kind. (Task 1 test `unknown_session_created_on_any_event`.)
5. Seven or more open sessions: colors repeat from the first, the vertical pill grows downward, nothing panics. (Task 1 test `seventh_session_reuses_first_color`.)

---

## File Structure

```
Cargo.toml
src/main.rs        arg dispatch: "hook" -> hook::run(), else ui::run()
src/state.rs       pure session state machine (no Win32, no egui)
src/event.rs       HookEvent parsing from hook JSON + todo extraction
src/detail.rs      permission card one-line detail
src/procs.rs       ancestor process chain, pid liveness, exe path of pid
src/pipe.rs        named pipe server/client, server-pid verification
src/hook.rs        hook client flow and decision output
src/server.rs      pipe server thread feeding the UI
src/ui/mod.rs      eframe app, window flags, exit logic
src/ui/theme.rs    color/radius constants, font loading
src/ui/blob.rs     blob painter (states: idle, working, waiting, done)
src/ui/views.rs    pill, expanded panel, permission/waiting cards, finished pill
src/focus.rs       Go to terminal: window activation + UI Automation tab select
assets/fonts/      Geist-Regular.ttf, Geist-Medium.ttf, Geist-SemiBold.ttf, GeistMono-Regular.ttf, OFL.txt
plugin/.claude-plugin/plugin.json
plugin/hooks/hooks.json
plugin/bin/agent-islands.exe   (build output, copied by Task 10)
tests/fixtures/*.json   recorded hook payloads from Task 0
docs/superpowers/notes/spike.md
```

---

### Task 0: Spike: confirm hook facts on the installed Claude Code (throwaway, time-boxed half a day)

Nothing from this task ships except `docs/superpowers/notes/spike.md` and `tests/fixtures/*.json`.

**Files:**
- Create: `spike/plugin/.claude-plugin/plugin.json`, `spike/plugin/hooks/hooks.json`, `spike/log-hook.ps1` (deleted at the end)
- Create: `docs/superpowers/notes/spike.md`, `tests/fixtures/<EventName>[-variant].json`

- [ ] **Step 1: `git init` in `D:\projects\agent-islands`, add `.gitignore` with `target/`, `.superpowers/`, `spike/`, `plugin/bin/`. Commit the spec and this plan.**

- [ ] **Step 2: Write a logging plugin.** `log-hook.ps1` appends `[Console]::In.ReadToEnd()` plus the process ancestry (`Get-CimInstance Win32_Process` walking `ParentProcessId`, name + pid) as one JSON line to `spike/events.jsonl`. `hooks.json` registers it for every event in spec Section 6 plus `PostToolUseFailure`, `PermissionDenied`, `TaskCreated`, `TaskCompleted`, matcher `*` where matchers apply.

- [ ] **Step 3: Run `claude --plugin-dir spike/plugin` in a Windows Terminal tab and exercise:** a prompt that creates a todo list of 3+ items; a Bash command that triggers a permission prompt (deny once in terminal, allow once); an `AskUserQuestion`; Esc to interrupt mid-turn; `/exit`.

- [ ] **Step 4: Answer each in `spike.md` with evidence lines from `events.jsonl`:**
  1. Which tool names carry todo status (`TodoWrite`? `TaskCreate`/`TaskUpdate`?) and exact field names for id, subject, status (`pending`/`in_progress`/`completed`), including where a created task's id appears (`tool_input` or `tool_response`).
  2. `notification_type` values seen and which mean "needs input".
  3. Does `Stop` fire on Esc interrupt?
  4. The ancestor chain: process names from hook up to `WindowsTerminal.exe` (is the Claude process `claude.exe`? is `OpenConsole.exe` in the chain?).
  5. Is `session_title` present on `UserPromptSubmit`, and does the Windows Terminal tab title contain it? (Check the tab title by eye.)
  6. Manually test `PermissionRequest` output: temporarily make the hook print `{"hookSpecificOutput":{"hookEventName":"PermissionRequest","decision":{"behavior":"deny","message":"Denied from Agent Islands"}}}` for Bash and confirm the terminal shows no prompt and Claude reports the denial. If this fails, STOP and report to the user (spec Section 6).
  7. With a test exe built `#![windows_subsystem = "windows"]` that echoes stdin to stdout, confirm hook stdout still reaches Claude Code (register it for `UserPromptSubmit` printing `{}`; no error in `claude --debug`).
  8. UI Automation: with `inspect.exe` or a 20-line PowerShell `System.Windows.Automation` script, confirm Windows Terminal tabs are `TabItem` elements whose `Name` is the tab title, and that `SelectionItemPattern.Select()` switches tabs.

- [ ] **Step 5: Copy one real payload per event (and per todo tool) into `tests/fixtures/`, scrubbed of prompt text. Delete `spike/`. Commit `spike.md` and fixtures.**

If any later task's field names disagree with `spike.md`, `spike.md` wins; update the test fixtures and keep the test names.

---

### Task 1: Session state machine

**Files:**
- Create: `Cargo.toml`, `src/main.rs`, `src/event.rs`, `src/state.rs`
- Test: unit tests inside `src/state.rs` and `src/event.rs` (`#[cfg(test)]`)

**Interfaces:**
- Produces:
  ```rust
  // event.rs
  pub struct HookEvent { pub name: String, pub session_id: String, pub cwd: String,
      pub tool_name: Option<String>, pub tool_input: serde_json::Value,
      pub tool_response: serde_json::Value, pub notification_type: Option<String>,
      pub session_title: Option<String> }
  pub fn parse(raw: &serde_json::Value) -> Option<HookEvent>
  pub enum TodoChange { Replace(Vec<Todo>), Upsert(Todo), SetStatus { id: String, status: TodoStatus } }
  pub fn todo_change(ev: &HookEvent) -> Option<TodoChange>
  // state.rs
  pub enum TodoStatus { Pending, InProgress, Completed }
  pub struct Todo { pub id: String, pub subject: String, pub status: TodoStatus }
  pub enum Status { Idle, Working, Waiting(WaitKind) }
  pub enum WaitKind { Permission, Question, Other }
  pub enum Finish { Ok, Error }
  pub struct Session { pub id: String, pub project: String, pub cwd: String, pub color: usize,
      pub status: Status, pub todos: Vec<Todo>, pub finished: Option<Finish>,
      pub title: Option<String>, pub claude_pid: Option<u32>, pub terminal_pid: Option<u32>,
      pub changed_at: u64 }
  pub struct Permission { pub req_id: u64, pub session_id: String, pub tool: String,
      pub detail: String, pub timed_out: bool }
  pub struct Pids { pub claude: Option<u32>, pub terminal: Option<u32> }
  pub struct State { pub sessions: Vec<Session>, pub permissions: std::collections::VecDeque<Permission> }
  impl State {
      pub fn apply(&mut self, ev: &HookEvent, pids: Pids, now: u64) -> Option<u64>; // Some(req_id) for PermissionRequest
      pub fn resolve(&mut self, req_id: u64);          // answered (allow or deny)
      pub fn timeout(&mut self, req_id: u64);          // -> card becomes "Waiting for you"
      pub fn mark_seen(&mut self, session_id: &str);
      pub fn prune(&mut self, alive: impl Fn(u32) -> bool);
      pub fn is_empty(&self) -> bool;
  }
  ```
  `Permission.detail` is filled by `detail::detail_line` (Task 2); until then use the tool name.

- [ ] **Step 1: Write failing tests in `state.rs`** using a helper `ev(name, session, extra_json)`:
  - `session_start_adds_idle_session`: project is last folder of cwd (`D:\projects\payments-api` gives `payments-api`), status Idle.
  - `unknown_session_created_on_any_event`: first event `PreToolUse` creates the session.
  - `prompt_sets_working_and_clears_todos_and_finished`.
  - `stop_sets_idle_and_finished_ok`; `stop_failure_sets_finished_error`.
  - `session_end_removes_session_and_its_permissions`.
  - `permission_request_sets_waiting_and_returns_req_id`.
  - `two_permissions_same_session_queue_in_order`: req ids increase; `permissions` front is the first; `resolve(first)` leaves second; session still Waiting(Permission) until both resolved.
  - `timeout_marks_permission_timed_out_and_wait_other`.
  - `pretooluse_ask_user_question_sets_waiting_question`.
  - `any_later_event_clears_waiting`: after Waiting(Question), a `PostToolUse` sets Working; after Waiting(Other), `Stop` sets Idle. Pending (not timed-out) permissions are not removed by other events; they leave only via `resolve`/`timeout`.
  - `notification_needs_input_sets_waiting_other_unless_card_open`; `notification_idle_ignored`. (Values per `spike.md` Q2.)
  - `seventh_session_reuses_first_color`; `freed_color_is_reused`: colors are the lowest index not used by live sessions, else `count % 6`.
  - `cwd_pinned_from_first_event`.
  - `prune_removes_dead_claude_pid`.
  - `is_empty_true_only_with_no_sessions`.
- [ ] **Step 2: Write failing tests in `event.rs`** against `tests/fixtures/` files via `include_str!`: `parse_reads_common_fields`, `todo_change_from_<tool>` for each todo tool found in Task 0 (status mapping, completed and in_progress kept in order).
- [ ] **Step 3: Run `cargo test` and confirm they fail to compile or fail.**
- [ ] **Step 4: Implement.** `state.rs` uses no Win32 or egui. Todo list display filter (completed + in_progress) belongs to the view, not the state.
- [ ] **Step 5: `cargo test` passes. Commit `feat: session state machine`.**

---

### Task 2: Permission detail line

**Files:** Create `src/detail.rs` (tests inline). Modify `src/state.rs` to call it.

**Interfaces:**
- Produces: `pub fn detail_line(tool: &str, input: &serde_json::Value, mcp_server: Option<&str>) -> (String /*verb phrase*/, String /*detail*/)` and `pub fn truncate(s: &str, max_chars: usize) -> String`.

- [ ] **Step 1: Failing tests:**
  - `bash_shows_command`: `("Wants to run a command", "npm install zod@3.23.8")`.
  - `edit_shows_path` for `Edit`, `Write`, `MultiEdit`, `NotebookEdit` (`file_path` / `notebook_path`): verb `Wants to edit a file`.
  - `webfetch_shows_url`: verb `Wants to fetch a URL`.
  - `mcp_shows_server_and_tool`: tool `mcp__github__create_issue` gives `github / create_issue`, verb `Wants to use a tool`.
  - `other_tool_uses_first_short_string_field`; `other_tool_without_strings_shows_tool_name`.
  - `multiline_command_uses_first_line_with_ellipsis`: `"cat <<EOF\nabc\nEOF"` gives `"cat <<EOF ..."`.
  - `truncate_counts_chars_not_bytes`: a 100-char string with non-ASCII cut to 60 chars ending in `...`.
- [ ] **Step 2: Run, fail. Step 3: Implement (max 60 chars for the detail; full text kept separately for the tooltip as `Permission.full`). Add `pub full: String` to `Permission`.**
- [ ] **Step 4: `cargo test` passes. Commit `feat: permission detail line`.**

---

### Task 3: Process helpers

**Files:** Create `src/procs.rs`.

**Interfaces:**
- Produces:
  ```rust
  pub fn ancestors(pid: u32) -> Vec<(u32, String)>   // (pid, exe file name), self first, stops at root or cycle
  pub fn find_pids(chain: &[(u32, String)]) -> Pids // claude = first name matching spike Q4, terminal = first "WindowsTerminal.exe"
  pub fn is_alive(pid: u32) -> bool
  pub fn exe_path(pid: u32) -> Option<std::path::PathBuf>
  ```
  Use `CreateToolhelp32Snapshot` once per `ancestors` call; `OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION)` + `GetExitCodeProcess` for `is_alive`; `QueryFullProcessImageNameW` for `exe_path`.

- [ ] **Step 1: Failing tests:** `ancestors_of_self_starts_with_self_and_includes_parent` (first entry is `std::process::id()`, length >= 2); `find_pids_picks_names` on a hand-written chain using the names recorded in `spike.md`; `is_alive_self_true_and_bogus_false` (pid `u32::MAX - 3`); `exe_path_of_self_matches_current_exe`.
- [ ] **Step 2: Run, fail. Step 3: Implement. Step 4: Pass. Commit `feat: process helpers`.**

---

### Task 4: Named pipe and hook client

**Files:** Create `src/pipe.rs`, `src/hook.rs`. Modify `src/main.rs`.

**Interfaces:**
- Consumes: `procs::{ancestors, find_pids, exe_path}`.
- Produces:
  ```rust
  // pipe.rs
  pub fn pipe_name() -> String                            // \\.\pipe\agent-islands-<USERNAME>
  pub struct Server;  impl Server { pub fn create_first() -> std::io::Result<Server>;  // FILE_FLAG_FIRST_PIPE_INSTANCE
                                    pub fn accept(&mut self) -> std::io::Result<std::fs::File>; }
  pub fn connect() -> std::io::Result<(std::fs::File, u32 /*server pid*/)> // GetNamedPipeServerProcessId
  // wire format: one JSON line each way
  // hook -> island: {"event": <raw hook json>, "chain": [[pid, "name"], ...]}
  // island -> hook (PermissionRequest only): {"decision":"allow"} or {"decision":"deny"}
  // hook.rs
  pub enum Reply { Allow, Deny }
  pub fn decision_output(reply: Option<Reply>, verified: bool) -> Option<String>
  pub fn run() -> i32   // always 0
  ```
  Pipe security descriptor `D:P(A;;GA;;;OW)`, flags `PIPE_REJECT_REMOTE_CLIENTS`. On connect failure (`ERROR_FILE_NOT_FOUND`), spawn `current_exe()` with no args, `DETACHED_PROCESS | CREATE_NO_WINDOW`, then retry every 50 ms up to 2 s; `ERROR_PIPE_BUSY` uses `WaitNamedPipeW` 500 ms. Verification: `exe_path(server_pid)` equals `current_exe()` after `canonicalize`. `run()` reads the reply line only when `hook_event_name == "PermissionRequest"`, with a 35 s guard. Add `#![windows_subsystem = "windows"]` to `main.rs` (spike Q7).

- [ ] **Step 1: Failing tests in `hook.rs`:**
  - `allow_verified_prints_allow_json`: equals `{"hookSpecificOutput":{"hookEventName":"PermissionRequest","decision":{"behavior":"allow"}}}`.
  - `deny_verified_prints_deny_with_message` (`Denied from Agent Islands`).
  - `unverified_prints_nothing` for Allow and Deny.
  - `no_reply_prints_nothing`.
  - `connect_failure_prints_nothing`: run the client logic against a pipe name nobody serves with spawning disabled (`run_with(name, spawn: bool, stdin)` internal fn) and assert empty output and return within 3 s.
- [ ] **Step 2: Failing test in `pipe.rs`:** `roundtrip_line_and_server_pid` (server on a test-unique pipe name in a thread, client connects, server pid equals `std::process::id()`, one line each way).
- [ ] **Step 3: Run, fail. Step 4: Implement. Step 5: Pass. Commit `feat: pipe and hook client`.**

---

### Task 5: Island process core (server thread, liveness, exit)

**Files:** Create `src/server.rs`, `src/ui/mod.rs` (headless part only). Modify `src/main.rs`.

**Interfaces:**
- Consumes: `pipe::Server`, `state::State`, `procs::{find_pids, is_alive}`.
- Produces:
  ```rust
  pub enum Msg { Event { raw: serde_json::Value, chain: Vec<(u32, String)>, reply: Option<std::sync::mpsc::Sender<hook::Reply>> },
                 Timeout(u64) }
  pub fn spawn(tx: std::sync::mpsc::Sender<Msg>, wake: impl Fn() + Send + 'static) -> std::io::Result<()>
  // ui/mod.rs
  pub struct Core { pub state: State, pub replies: std::collections::HashMap<u64, std::sync::mpsc::Sender<hook::Reply>>, empty_since: Option<std::time::Instant> }
  impl Core { pub fn handle(&mut self, msg: Msg, now: u64); pub fn answer(&mut self, req_id: u64, r: hook::Reply);
              pub fn tick(&mut self, now: std::time::Instant) -> bool /*true = exit*/ }
  ```
  One thread per accepted connection. For a `PermissionRequest` the connection thread creates a channel, sends `Msg::Event` with `reply: Some(tx)`, waits `recv_timeout(30 s)`; on reply writes the decision line; on timeout sends `Msg::Timeout(req_id)` and closes without writing. `wake` calls `egui::Context::request_repaint`. If `Server::create_first` fails, the process exits 0 (another island owns the pipe). `tick` prunes dead pids every 3 s and returns true after `is_empty()` held for 3 s.

- [ ] **Step 1: Failing tests on `Core`:** `answer_sends_reply_and_resolves`; `timeout_msg_marks_timed_out`; `tick_exits_after_3s_empty_not_before`; `tick_does_not_exit_while_session_exists`.
- [ ] **Step 2: Run, fail. Step 3: Implement. Step 4: Pass.**
- [ ] **Step 5: Manual check:** run `agent-islands.exe` with a temporary stdout log of state, pipe `tests/fixtures/SessionStart.json` through `agent-islands.exe hook`, confirm the session appears; pipe `SessionEnd.json`, confirm the process exits about 3 s later. Commit `feat: island core`.

---

### Task 6: Window, theme, blob, collapsed pill

**Files:** Create `src/ui/theme.rs`, `src/ui/blob.rs`, `src/ui/views.rs` (pill only). Modify `src/ui/mod.rs`. Add `assets/fonts/*` (static TTFs and `OFL.txt` from the `vercel/geist-font` GitHub release).

**Interfaces:**
- Produces:
  ```rust
  pub enum BlobState { Idle, Working, Waiting, Done }
  pub fn paint_blob(p: &egui::Painter, center_bottom: egui::Pos2, size: f32, color: egui::Color32, st: BlobState, t: f64)
  pub fn pill(ui: &mut egui::Ui, core: &Core, t: f64) -> egui::Response
  pub fn setup_fonts(ctx: &egui::Context)   // families: "geist", "geist-medium", "geist-semibold", "geist-mono"
  ```
  Blob geometry in a 24-unit box, from the mockup: body path `M3.5 17 c0-6.5 3.8-11 8.5-11 s8.5 4.5 8.5 11 c0 2.4-2.6 3.5-8.5 3.5 S3.5 19.4 3.5 17z` (cubic beziers); highlight ellipse at (8.4,10.2) r(2,1.2) white 35%; eyes ellipses at x 9.3 / 14.7, y 14, r 1.15 x 1.5 in `#0c0c0e`; waiting: eyes y 12.6 r 1.1 plus mouth circle (12,16.8) r .8 plus amber badge circle (20.5,4.5) r 3.4 with "!" pulsing scale 1 to 1.15 over 1.2 s; done: eyes as arcs plus green check badge; working: squash-bounce 0.9 s (translate -1.6 at 40%, scale (.97,1.03) at 40%, (1.04,.95) at 80%, origin bottom center); idle: still, 50% opacity.
  Window: `ViewportBuilder` transparent, no decorations, always on top, `with_taskbar(false)`, fixed size 400 x 520 logical px, its top-left corner at 8 px from the left and 96 px from the top of the primary monitor work area (recomputed each time the island becomes visible; per-monitor DPI aware so it is crisp at 125% / 150%). Vertical pill height animates smoothly as blobs come and go. After creation set `WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW` on the HWND. Mouse passthrough: each frame, `ViewportCommand::MousePassthrough(!cursor_over_shape)` where the cursor comes from `GetCursorPos` (egui gets no hover while passthrough is on). Repaint: `request_repaint_after(33 ms)` while any blob animates, else `request_repaint_after(100 ms)` for cursor polling.

- [ ] **Step 1: Failing test in `blob.rs`:** `squash_offsets_match_keyframes` for a pure `fn bounce(t: f64) -> (f32 /*dy*/, f32 /*sx*/, f32 /*sy*/)` at t = 0, 0.36, 0.72 s.
- [ ] **Step 2: Run, fail. Step 3: Implement.**
- [ ] **Step 4: Manual check against the mockup** (states 1 and 2): feed fixtures for three sessions, one working, one waiting, one idle. Compare side by side with `island-left.html`. Check focus: typing in a terminal keeps working while the island appears and animates.
- [ ] **Step 5: Measure:** Task Manager working set under 30 MB and CPU near 0 with idle blobs. Record numbers in the commit message. Commit `feat: window and collapsed pill`.

---

### Task 7: Expanded panel

**Files:** Modify `src/ui/views.rs`, `src/ui/mod.rs`.

**Interfaces:**
- Produces: `pub fn panel(ui: &mut egui::Ui, core: &Core, selected: &mut Option<String>, t: f64)` and `pub fn visible_todos(todos: &[Todo]) -> Vec<&Todo>`.
  Panel opens to the right of the vertical pill (8 px gap, top-aligned) with a 250 ms critically damped spring on width, height and opacity. Panel 280-340 px wide, layout per spec 7.3 and `island-left.html` state 2 (no blob row in the panel; the pill is the picker, with a 2 x 12 px white bar left of the selected blob; project 14 px semibold, path 11 px mono muted, todos 13 px). Default selection: session with the latest `changed_at`. No todos while working: one spinner line "Working". Collapse: on a click outside the window (poll `GetAsyncKeyState(VK_LBUTTON)` with cursor outside the shape while expanded) or a click on the pill area again.

- [ ] **Step 1: Failing tests:** `visible_todos_keeps_completed_and_in_progress_in_order`; `default_selection_is_latest_changed`.
- [ ] **Step 2: Run, fail. Step 3: Implement. Step 4: Pass.**
- [ ] **Step 5: Manual check** against mockup state 3 with a fixture-driven todo list. Commit `feat: expanded panel`.

---

### Task 8: Permission card, waiting card, finished pill

**Files:** Modify `src/ui/views.rs`, `src/ui/mod.rs`.

**Interfaces:**
- Consumes: `Core::answer`, `State::mark_seen`, `focus::go_to_terminal` (Task 9; stub returning `false` until then).
- Produces: `pub fn permission_card(...)`, `pub fn waiting_card(...)`, `pub fn finished_pill(...)`, and `pub enum View { Pill, Panel, Permission(u64), Waiting(String) }` with `pub fn current_view(core: &Core, expanded: bool) -> View`.
  Priority: oldest non-timed-out permission, then any waiting session, then expanded panel if expanded, else pill. Cards open without focus. Permission card copy per spec 7.4: verb phrase from Task 2, mono box with tool name in amber then detail, tooltip with `full`, Deny and Allow buttons (mouse only), "1 of N" when N > 1. Waiting card copy: "Has a question for you" (Question) or "Waiting for you" (Other or timed out), one "Go to terminal" button. Finished pill per 7.6: single finished shows happy blob + project semibold + "finished" (or "stopped with an error"); several show their blobs + "N finished"; all other blobs stay in the pill. Clicking the finished pill opens the panel on that session and calls `mark_seen`.

- [ ] **Step 1: Failing tests on `current_view`:** `permission_beats_waiting_beats_panel`; `timed_out_permission_shows_waiting`; `finished_count_reflects_unseen`.
- [ ] **Step 2: Run, fail. Step 3: Implement. Step 4: Pass.**
- [ ] **Step 5: Manual end-to-end with the real plugin (Task 10 hooks.json may be pulled forward for this):** trigger a Bash permission, click Deny, confirm Claude reports the denial; trigger again, click Allow, confirm it runs; let one sit 30 s, confirm the terminal prompt appears and the card switches. Commit `feat: permission and waiting cards, finished pill`.

---

### Task 9: Go to terminal

**Files:** Create `src/focus.rs`. Modify `src/ui/views.rs` to call it.

**Interfaces:**
- Produces: `pub fn go_to_terminal(terminal_pid: u32, title_hint: Option<&str>) -> bool`.
  Step A (required): `EnumWindows`, pick the visible top-level window of `terminal_pid` (class `CASCADIA_HOSTING_WINDOW_CLASS`); if minimized `ShowWindow(SW_RESTORE)`; `SetForegroundWindow`. If refused, use the `AttachThreadInput` fallback. Step B (only if spike Q5 and Q8 passed): UI Automation `IUIAutomation::ElementFromHandle`, find descendant `TabItem` whose `Name` contains the hint, `SelectionItemPattern::Select`. Return true if step A succeeded. With several Windows Terminal windows sharing one `WindowsTerminal.exe` pid, try each window's tabs for the hint and activate the one that matches; with no hint match, activate the most recently active window.

- [ ] **Step 1: Failing test:** `bogus_pid_returns_false`.
- [ ] **Step 2: Run, fail. Step 3: Implement. Step 4: Pass.**
- [ ] **Step 5: Manual check:** two Windows Terminal windows, three tabs, three sessions; an `AskUserQuestion` in a background tab; "Go to terminal" lands on that window (and tab if Step B shipped). Record the result in `spike.md`. Commit `feat: go to terminal`.

---

### Task 10: Plugin packaging and final verification

**Files:** Create `plugin/.claude-plugin/plugin.json`, `plugin/hooks/hooks.json`, `README.md` (install steps only).

**Interfaces:**
- `hooks.json` registers `"\"${CLAUDE_PLUGIN_ROOT}/bin/agent-islands.exe\" hook"` (quoted) for: `SessionStart`, `UserPromptSubmit`, `PreToolUse` (`*`), `PostToolUse` (`*`), `PostToolUseFailure`, `PermissionDenied`, `PermissionRequest` (timeout 40), `Notification`, `Stop`, `StopFailure`, `SessionEnd`; all others timeout 5.

- [ ] **Step 1: `cargo build --release` and copy `target/release/agent-islands.exe` to `plugin/bin/`.**
- [ ] **Step 2: Install with `claude --plugin-dir plugin` (or a local marketplace) and run the full checklist:** 5 terminals with Claude Code show 5 blobs; one working shows bounce and todos; permission Allow/Deny/timeout; question opens card and "Go to terminal"; finished pill appears and clears on click; closing all sessions exits the island within about 3 s (verify in Task Manager); island never steals typing focus.
- [ ] **Step 3: Measure** hook latency (`Measure-Command { Get-Content tests/fixtures/PostToolUse.json | plugin/bin/agent-islands.exe hook }`, expect under 20 ms with island running), island RAM under 30 MB, CPU near 0 when all idle.
- [ ] **Step 4: Commit `feat: plugin packaging`.**
