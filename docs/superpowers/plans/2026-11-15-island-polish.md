# Island Polish Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Correct session names, a visible and larger island, a left/right side setting, random session colors, and no lost blobs or todos across island restarts.

**Architecture:**
- Changes stay inside the existing modules, plus one new file, `src/settings.rs`.
- Pure helpers carry unit tests; Win32 and egui wiring stays thin.
- The todo replay runs on a short-lived thread and reports back through the existing `Msg` channel.

**Tech Stack:** Rust 2024, eframe/egui 0.37 (glow), windows 0.62.2, serde_json. No new dependencies.

**Spec:** `docs/superpowers/specs/2026-11-15-island-polish-design.md`

## Global Constraints

- No new crates.
- Every task is test-first: write the failing test, watch it fail, implement, watch it pass.
- `cargo test` (debug) only; never `cargo test --release`.
- Palette and shapes are unchanged except BG `#1c1d22` and LINE 14% white. Corners: collapsed pill fully rounded, panels and cards 8 px, buttons 4 px. Fonts: Geist and Geist Mono.
- The island stays at 0% CPU when idle: no new polling loops.
- No emojis anywhere: code, comments, commits or UI copy.
- Write commit messages in Conventional Commits style.
- The command gate shows an approval card for `git commit`. The user must click Allow on the island for each commit.

## Review Focus

1. **A cwd that no longer exists or is unreadable** (deleted folder, network path): the name falls back to the last two components of the cwd text, with no panic. Test in Task 2.
2. **A cwd at a drive root or with a single component** (`D:\`, `/`): the name is a single component or empty, never ` > ` with an empty side. Test in Task 2.
3. **A missing, truncated or huge transcript, or lines that aren't JSON:** the replay skips bad lines and returns what it could parse; a missing file gives an empty list. Test in Task 6.
4. **A corrupt or hand-edited `settings.json`** (wrong type, unknown side): the side falls back to Left. Test in Task 4.
5. **The island exe can't be read by the hook** (locked, deleted after an update): verification fails closed (unverified), never panics. Test in Task 5.

---

### Task 1: Random session colors

**Files:**
- Modify: `src/state.rs:122-127`, plus the tests at `src/state.rs:625-643`

**Interfaces:**
- Produces: `fn pick_color(used: &[usize], seed: u64) -> usize` (private to `state.rs`). It returns an index in `0..6`.

- [ ] **Step 1: Write the failing tests.**
  - `pick_color_avoids_used`: for every seed in `0..200`, `pick_color(&[0, 2, 3, 5], seed)` is in `{1, 4}`.
  - `pick_color_any_when_all_used`: for every seed in `0..200`, `pick_color(&[0, 1, 2, 3, 4, 5], seed) < 6`.
  - `pick_color_spreads`: over seeds `0..200` with nothing used, at least 4 different values appear.
  - Replace `seventh_session_reuses_first_color` and `freed_color_is_reused`. Six sessions get six distinct colors. After session 2 ends, the next new session gets exactly session 2's old color, because it is the only free one.
- [ ] **Step 2: Run `cargo test pick_color` and confirm it fails to compile.**
- [ ] **Step 3: Implement `pick_color`.**
  - Free colors are `(0..6)` minus `used`. If none are free, choose from all six.
  - Choose with `seed as usize % candidates.len()`.
  - In `State::apply`, the seed is `std::collections::hash_map::RandomState::new().hash_one(sid)`, which is random per call.
- [ ] **Step 4: Run `cargo test` and confirm all pass.**
- [ ] **Step 5: Commit.** `feat: random color among unused session colors`

### Task 1b: A new todo set replaces a finished one

The bug: idle panels keep the last todos (Task 12). When the session starts its next set, TaskCreate appends to the old list, so the list grows without end and the panel overflows.

**Files:**
- Modify: `src/state.rs`, `apply_todo` at :84-106 and its tests

**Interfaces:**
- No new names. `apply_todo` keeps its signature. Task 6 reuses it, so the replay gets the same rule.

- [ ] **Step 1: Write the failing tests.**
  - `new_task_after_all_done_starts_fresh_list`: A and B, both Completed, then an Upsert of C (a new id) gives exactly `[C]`.
  - `new_task_while_some_open_appends`: A Completed and B InProgress, then an Upsert of C gives `[A, B, C]`.
  - `upsert_existing_id_never_clears`: A Completed, then an Upsert of A again gives `[A]` with the new value.
- [ ] **Step 2: Run `cargo test new_task upsert_existing` and confirm it fails.**
- [ ] **Step 3: Implement.**
  - In the `Upsert` arm, when the id is unknown and `!todos.is_empty()` and every todo is `Completed`, clear `todos` before pushing.
  - `TodoWrite` (`Replace`) already replaces the list.
- [ ] **Step 4: Run `cargo test` and confirm all pass.**
- [ ] **Step 5: Commit.** `fix: new todo set replaces a finished list`

### Task 2: Session name from repo, last two folders

**Files:**
- Modify: `src/state.rs` (replace `project_name` at :79-82, the session update in `apply`, and the tests at :312-318 and :646-651)
- Modify: `src/ui/views.rs:85` and the panel head height at :98 (remove the path line)

**Interfaces:**
- Produces: `pub fn display_name(cwd: &str) -> String` in `state.rs`.
- `Session.project` holds the display name. `Session.cwd` holds the latest non-empty cwd.

- [ ] **Step 1: Write the failing tests.** They use `std::env::temp_dir()` subfolders created in the test.
  - `display_name_uses_repo_root`: `<tmp>/ws/repo/.git` (a directory) and `cwd = <tmp>/ws/repo/app/src` give `"ws > repo"`.
  - `display_name_git_file_counts`: a `.git` *file* (a worktree) also marks the root.
  - `display_name_no_repo_uses_cwd`: `<tmp>/a/b/c` with no `.git` gives `"b > c"`.
  - `display_name_missing_path`: `r"Z:\nope\gone\deep"` gives `"gone > deep"`.
  - `display_name_root_and_single`: `r"D:\"` gives `"D:"`; `"/home"` gives `"home"`; `""` gives `""`.
  - `display_name_trailing_and_forward_slashes`: keep the intent of the old test. `r"D:\projects\x-app\"` gives `"projects > x-app"`.
  - Replace `cwd_pinned_from_first_event` with `name_follows_cwd`. A later event with `cwd = r"D:\elsewhere\proj"` updates `cwd` and sets `project` to `"elsewhere > proj"`. An event with an empty cwd changes nothing.
  - Update the assertions at :306 and :651 to the new form.
- [ ] **Step 2: Run `cargo test display_name name_follows` and confirm it fails.**
- [ ] **Step 3: Implement `display_name`.**
  - Walk up from `Path::new(cwd)` to the first ancestor where `join(".git").exists()`. If none is found, use cwd.
  - Take the last two normal components of that path, splitting on both `\` and `/` and ignoring empty parts, and join them with `" > "`.
  - In `apply`, recompute `project` only when `ev.cwd` is non-empty and differs from `s.cwd`, to keep filesystem calls rare.
- [ ] **Step 4: Remove the path line in the panel.**
  - The panel already shows `project` as its title; a second line with the same text adds nothing.
  - Delete `path` and its painting, and drop `2.0 + path.size().y` from `head`.
  - Update any panel-layout test that counted that line.
- [ ] **Step 5: Run `cargo test` and confirm all pass.**
- [ ] **Step 6: Commit.** `fix: session name follows cwd, repo root, last two folders`

### Task 3: Visible tone and 1.25x size

**Files:**
- Modify: `src/ui/theme.rs:4-5`
- Modify: `src/ui/views.rs` (add `pub const ZOOM: f32 = 1.25;` beside `MARGIN`)
- Modify: `src/ui/mod.rs:98` (window size), `:116` (zoom), `:307-317` (`place`)

**Interfaces:**
- Produces: `views::ZOOM`. `views::WIN_W: f32 = 400.0` is the logical window width, the counterpart of the existing `WIN_H`; make both `pub`.

- [ ] **Step 1: Write the failing test.**
  - In the `theme.rs` tests module (create it), `bg_and_line_values`:
    - `BG == Color32::from_rgb(0x1c, 0x1d, 0x22)`
    - `LINE == Color32::from_white_alpha(36)`. 14% of 255 is about 36; use `from_white_alpha` so the alpha is premultiplied correctly.
- [ ] **Step 2: Run it and confirm it fails.**
- [ ] **Step 3: Set the values and wire the zoom.**
  - Set BG and LINE.
  - Call `cc.egui_ctx.set_zoom_factor(views::ZOOM)` after `setup_fonts`.
  - Window inner size becomes `[WIN_W * ZOOM, WIN_H * ZOOM]`.
  - In `place`, multiply the logical offsets (`8.0 - MARGIN`, `96.0 - MARGIN`) by `ZOOM` as well as the DPI scale.
  - `region` and `cursor` already use `ctx.pixels_per_point()`, which includes zoom. Check that, and don't change them.
- [ ] **Step 4: Run `cargo test` and confirm all pass.**
- [ ] **Step 5: Check it by eye.**
  - Run `cargo run` with `AGENT_ISLANDS_OPEN` against a fake session. The `tests/island.rs` helpers show how to send events.
  - Confirm the pill, panel and click targets line up and nothing is clipped at the window edges.
  - Report what you saw.
- [ ] **Step 6: Commit.** `feat: lighter island tone and 1.25x size`

### Task 4: Left/right side setting

**Files:**
- Create: `src/settings.rs`
- Modify: `src/main.rs` (add `mod settings;`)
- Modify: `src/ui/views.rs`:
  - pill x at :39
  - selbar at :55
  - `panel_rect` at :265
  - card rect at :492
  - `label_rect` at :497
  - new `settings_card`
- Modify: `src/ui/mod.rs`:
  - `App` gets `side: Side` and `settings_open: bool`
  - a right-click on the pill toggles `settings_open`
  - `place` takes `side`

**Interfaces:**
- Produces, in `src/settings.rs`:
  - `#[derive(Clone, Copy, PartialEq, Debug, Default)] pub enum Side { #[default] Left, Right }`
  - `pub fn load_from(path: &Path) -> Side`
  - `pub fn save_to(path: &Path, side: Side) -> std::io::Result<()>`
  - `pub fn path() -> Option<PathBuf>`: `%APPDATA%\agent-islands\settings.json`
  - The file holds `{"side":"left"}` or `{"side":"right"}`. Anything else loads as Left.
- Produces, in `views.rs`:
  - `pub fn beside(pill: Rect, w: f32, side: Side) -> f32`: the left x of a box `w` wide beside the pill. Left side: `pill.right() + 8.0`. Right side: `pill.left() - 8.0 - w`.
  - `pill_x(side) -> f32`: Left gives `MARGIN`; Right gives `WIN_W - MARGIN - PILL_W`.
  - `panel_rect`, the card rect and `label_rect` gain a `side: Side` parameter and use `beside`.
  - `pub fn settings_card(ui, pill: Rect, side: Side) -> (Rect, Option<Side>)`: a card beside the pill, top-aligned with it, `CARD_W` wide. It has the title "Settings" (Geist SemiBold 14) and a row "Side" with two 4 px buttons, "Left" and "Right". The current side's button is filled with FG at 10% alpha. It returns the clicked side.

- [ ] **Step 1: Write the failing tests.**
  - `settings.rs`:
    - `roundtrip_right`: `save_to(p, Right)` then `load_from(p)` gives `Right`.
    - `missing_file_is_left`.
    - `garbage_is_left`, for each of: `"not json"`, `{"side":3}` and `{"side":"top"}`.
  - `views.rs`:
    - `beside_left_and_right`: for a pill with x from 8 to 42, `beside(pill, 300.0, Left) == 50.0` and `beside(pill, 300.0, Right) + 300.0 == 0.0` (the right edge is `pill.left() - 8.0`).
    - `pill_x_right_hugs_window_edge`: `pill_x(Right) + PILL_W + MARGIN == WIN_W`.
    - `panel_rect_right_side_left_of_pill`: `panel_rect(pill, 100.0, Right).right() == pill.left() - 8.0`.
- [ ] **Step 2: Run `cargo test settings beside pill_x panel_rect_right` and confirm it fails.**
- [ ] **Step 3: Implement `settings.rs`.**
  - Use `serde_json`. `save_to` creates the parent directory and writes the whole file.
- [ ] **Step 4: Thread `side` through `views.rs`.**
  - Every caller passes `self.side`.
  - On the Right side, the selbar sits 6 px right of the blob box. This mirrors the Left side.
- [ ] **Step 5: Wire it in `mod.rs`.**
  - Load the side at startup: `settings::path().map(|p| settings::load_from(&p)).unwrap_or_default()`.
  - `win::place(hwnd, side)`, with `k = ZOOM * dpi / 96`. The pill sits 8 logical px from the edge either way.
    - Left: `x = work.left + (8 - MARGIN) * k`
    - Right: `x = work.right - 8 * k - (WIN_W - MARGIN) * k`
    - Both: `y = work.top + (96 - MARGIN) * k`
  - `pill.secondary_clicked()` toggles `settings_open`.
  - While `settings_open` is set, the settings card replaces the panel/finished-pill branch, but not a permission or waiting card. Permission and waiting cards take priority, and `settings_open` stays set underneath them.
  - Clicking a side: `save_to`, set `self.side`, call `win::place(hwnd, side)`.
  - An outside click closes the card, using the existing `outside_click` path.
  - Add the card rect to the region shapes like the panel (radius 8).
- [ ] **Step 6: Run `cargo test` and confirm all pass.**
- [ ] **Step 7: Check it by eye.**
  - Right-click the pill, choose Right. The island jumps to the right edge, and panels and cards open leftward.
  - Restart the island: it stays on the right.
  - Choose Left again.
  - Report what you saw.
- [ ] **Step 8: Commit.** `feat: left or right side setting`

### Task 5: Verify the island by exe content

**Files:**
- Modify: `src/hook.rs:46-53` (`canon`, `same_exe`) and its test at :248-253

**Interfaces:**
- Produces: `pub fn same_exe(a: &Path, b: &Path) -> bool`. The signature is unchanged; the meaning is now "same file content".

- [ ] **Step 1: Write the failing tests.**
  - `same_exe_true_for_byte_identical_copy`: copy `std::env::current_exe()` to a temp file; `same_exe(&exe, &copy)` is true.
  - `same_exe_false_for_different_content`: compare against `C:\Windows\notepad.exe`; the result is false.
  - `same_exe_false_when_unreadable`: compare against a path that doesn't exist; the result is false, with no panic.
  - Keep the case and canonicalize test: the same path in different case is still true.
- [ ] **Step 2: Run `cargo test same_exe` and confirm the copy test fails.**
- [ ] **Step 3: Implement.**
  - If the canonical paths are equal, return true right away; that is the common case, so don't read the files.
  - Otherwise, compare `metadata().len()` first, then `std::fs::read(a)? == std::fs::read(b)?`. Any error gives false.
- [ ] **Step 4: Run `cargo test` and confirm all pass.**
- [ ] **Step 5: Measure the hook latency.**
  - Time 20 hook calls (`Measure-Command`, as in Task 10b), with the hook running from a copy in a different folder.
  - Report the median. Expect under 40 ms.
- [ ] **Step 6: Commit.** `fix: hook verifies the island by exe content, not path`

### Task 6: Rebuild todos from the transcript

**Files:**
- Modify: `src/event.rs` (add `replay`)
- Modify: `src/server.rs` (`Msg::Replay` variant)
- Modify: `src/ui/mod.rs`:
  - `Core` holds `tx: Sender<Msg>`
  - `handle` spawns the replay for new sessions
  - `Msg::Replay` applies the result
- Modify: `src/state.rs` (`pub fn set_todos(&mut self, session_id: &str, todos: Vec<Todo>)`)
- Test fixture: `tests/fixtures/transcript.jsonl`

**Interfaces:**
- Produces:
  - `pub fn replay(text: &str) -> Vec<Todo>` in `event.rs`.
  - `Msg::Replay { session_id: String, todos: Vec<Todo> }`.
  - `State::set_todos`.
  - `Core::new(tx: Sender<Msg>)`.

**How a transcript encodes the todo calls:**
- An assistant line, `"type":"assistant"`, has `message.content[]` items `{"type":"tool_use","id","name","input"}`.
- The result is on a later line, `"type":"user"`. Its `message.content[]` has `{"type":"tool_result","tool_use_id"}`, and the line has a top-level `toolUseResult` holding the tool response. For TaskCreate that is `{"task":{"id":"1",...}}`.
- Lines of other types (for example `api-request-shape`, which embeds tool schemas) are ignored.

- [ ] **Step 1: Write the fixture and the failing tests.**
  - `tests/fixtures/transcript.jsonl` holds these lines, in order:
    - TaskCreate "A" (result id 1)
    - TaskCreate "B" (id 2)
    - TaskUpdate 1 in_progress
    - TaskUpdate 1 completed
    - an `api-request-shape` line mentioning `"name":"TaskCreate"`
    - a line `not json`
    - a TaskCreate tool_use with no result line (cut-off transcript)
  - `replay_rebuilds_task_list`: the result is `[A Completed id 1, B Pending id 2]`.
  - `replay_todowrite_replaces`: a TodoWrite with two items, after the tasks, gives exactly those two.
  - `replay_empty_and_garbage`: `replay("")` and `replay("x\n{")` are empty.
  - `state.rs`: `set_todos_replaces_for_known_session_only`.
  - `ui/mod.rs`, Core test: a first event for a session whose `transcript_path` points to the fixture sends `Msg::Replay` with the 2 todos within 2 s. Read it from the channel the Core was built with. A second event for the same session doesn't spawn a second replay.
- [ ] **Step 2: Run `cargo test replay set_todos` and confirm it fails.**
- [ ] **Step 3: Implement `replay`.**
  - Walk the lines.
  - Collect `tool_use` items whose name is TaskCreate, TaskUpdate or TodoWrite, by id.
  - When a `tool_result` for one of them arrives, build a `HookEvent { tool_name, tool_input, tool_response: toolUseResult, .. }` and fold `todo_change` into the list. Reuse `state::apply_todo`; make it `pub(crate)`.
  - Apply every call only when its result arrives, and skip results whose `tool_result.is_error == true`, so failed calls never change the list.
- [ ] **Step 4: Wire it.**
  - In `Core::handle`, if the session id wasn't in `state.sessions` before `apply` and `ev.transcript_path` is non-empty, spawn a thread that reads the file (`std::fs::read_to_string`; on error, do nothing) and sends `Msg::Replay`.
  - `Msg::Replay` calls `state.set_todos`.
  - The wake closure is not available in Core, so the UI picks the message up on its next frame (33 ms while working, 3 s idle). That is acceptable; add a `ponytail:` comment naming the 3 s ceiling.
  - `ponytail:` comment: a replay that lands after live todo events overwrites them with the transcript's state, which already contains them. That's fine unless the transcript write lags the hook.
- [ ] **Step 5: Run `cargo test` and confirm all pass.**
- [ ] **Step 6: Commit.** `feat: rebuild session todos from transcript on first sight`

### Task 7: Release

**Files:**
- Modify: `plugin/.claude-plugin/plugin.json`. `build-plugin.ps1` bumps it.

- [ ] **Step 1: Run `cargo test` and `cargo clippy --all-targets -- -D warnings`.** Both must be clean.
- [ ] **Step 2: Run `powershell -File build-plugin.ps1`.** It stops the running island, builds the release, copies the exe and bumps the version.
- [ ] **Step 3: Commit and tag.**
  - Commit: `chore: plugin version <new>`
  - Tag: `git tag v<new>`
- [ ] **Step 4: Report the user's steps.**
  1. Run `claude plugin update agent-islands@agent-islands-local`.
  2. Restart the Claude Code sessions.
