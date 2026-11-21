# Readable Phone Messages Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make Telegram messages from several agents easy to read while away: STE-style replies, collapsed details, code-font todos, `[project]` headers, one-tap buttons with visible outcomes.

**Architecture:** All message shaping stays in `src/phone.rs` (`message`, `send_question`, `send_to`) and `src/telegram.rs` (`render`/`place`/`cut`, new `fold`, new `Out::Edit`). Tap outcomes edit the tapped message statelessly from `callback_query.message`. The STE instruction rides on the `UserPromptSubmit` hook: the island answers with the Away flag, the hook prints `additionalContext`.

**Tech Stack:** Rust, serde_json, ureq, Telegram Bot API, Claude Code hooks.

**Spec:** `docs/superpowers/specs/2026-11-20-phone-readability-design.md`

## Global Constraints

- Header: `<circle> [<project>]`, plus ` · permission` / ` · question` / ` · error` on messages that need the user. No other emoji in phone text.
- `Started` and `Ended` stay. `You (PC): ...` goes.
- The 3500-char cut (`telegram::cut`) applies to the reply only; the todo block is never cut.
- Todos: one `pre` entity, lines `[x] <subject>` (completed) and `[ ] <subject>` (anything else).
- STE only while Away is on. The prompt hook waits at most 300 ms for the island; on any failure it prints nothing.
- Every button tap gets `answerCallbackQuery`; a final tap also edits the message: buttons removed, `\n<outcome>` appended.
- Outcome strings, exact: `Allowed`, `Denied`, `Answered`, `Sent to [<project>]`, `Queued: [<project>] is busy`, `[<project>] can't receive messages right now`, `Too late, answered in the terminal`, `Already answered`.

## Review Focus

1. A Stop reply whose first paragraph is a code block: the split must not land inside the `pre` entity. Test in Task 2.
2. A Stop reply longer than 3500 chars with todos: todos must still arrive whole after the cut. Test in Task 2.
3. A tap on a message whose `callback_query.message` has no `text` (old message): toast only, no edit, no panic. Test in Task 3.
4. A second tap on an already-answered Allow/Deny (edit failed or raced): outcome `Already answered`, no crash. Test in Task 3 (existing Core test, updated).
5. Island not running or too old when a prompt is submitted: hook prints nothing and returns within ~300 ms. Test in Task 4.

---

### Task 1: Headers, status words, drop `You (PC)`

**Files:**
- Modify: `src/phone.rs` (`message`, `send_question`, `on_event` "Working on it", session/question picker labels in `incoming`, `send_to`)
- Test: `src/phone.rs` tests module

**Interfaces:**
- Produces: `pub fn head(s: &Session) -> String` in `src/phone.rs`, returns `format!("{} [{}]", circle(s.color), s.project)`. Tasks 2-3 use it.

- [ ] **Step 1: Update the existing tests to the new texts** (they fail now)

In `src/phone.rs` tests:
- `started_ended_and_pc_prompt`: `"🟠 [X]\nStarted"`, `"🟠 [X]\nEnded"`, and `msg(&r)` with `prompt = "hi"` is `None`.
- `empty_stop_and_bare_waiting`: `"🟠 [X] · question\nWaiting for you"` (both cases).
- `card_has_allow_deny_buttons`: `"🟠 [X] · permission\nWants to run a command npm i"`.
- `stop_failure_and_waiting`: `"🟠 [X] · error\nturn failed"`, `"🟠 [X] · error\nrate_limit"`, `"🟠 [X] · question\nPick option for probe test: X or Y?"`, `"🟠 [X] · question\nClaude is waiting"`.
- Every other test asserting `"🟠 X\n..."` (e.g. `"Working on it"`, `"X can't receive messages right now"`, `"Queued: X is busy"`): switch to the bracket form from Global Constraints.

- [ ] **Step 2: Run** `cargo test phone::` — Expected: the updated tests FAIL on text mismatch.

- [ ] **Step 3: Implement**
- Add `head(s)`. In `message`, build the header as `head(s)` plus the status suffix chosen with the body: card → ` · permission`, AskUserQuestion and `Notification` waiting → ` · question`, `StopFailure` → ` · error`. Bodies lose their `Permission: ` / `Waiting for you: ` / `Error: ` prefixes; a waiting notification with no message keeps the body `Waiting for you`.
- `UserPromptSubmit` arm → `return None`.
- `send_question`, "Working on it", and picker button labels use `head(s)` (question picker label: `format!("{}: {}", head(s), header)`).

- [ ] **Step 4: Run** `cargo test` — Expected: all PASS.

- [ ] **Step 5: Commit** `git commit -am "feat(phone): [project] headers, status words, no PC prompt echo"`

---

### Task 2: Collapsed details and code-font todos

**Files:**
- Modify: `src/telegram.rs` (new `fold`), `src/phone.rs` (`message`, Stop arm and final assembly)
- Test: both files' tests modules

**Interfaces:**
- Consumes: `head(s)` from Task 1.
- Produces: `pub fn fold(text: &str, entities: Vec<Value>) -> Vec<Value>` in `src/telegram.rs`.

`fold` algorithm (the tests do not fully pin it):
```
split = UTF-16 offset just after the first "\n\n" or the 3rd "\n" in text, whichever is earlier
if no such point, or text after split is only whitespace: return entities unchanged
if split falls strictly inside a pre/code entity: split = that entity's offset (if that makes split 0, return unchanged)
clip every entity crossing split into two pieces (one each side); drop zero-length pieces
push {"type":"expandable_blockquote","offset":split,"length":total_units - split}
```

- [ ] **Step 1: Write failing tests in `src/telegram.rs`**
- `fold_short_reply_has_no_quote`: `fold("a\nb", vec![])` is empty.
- `fold_quotes_after_first_paragraph`: text `"r1\nr2\n\nmore\nmore"` → contains `{"type":"expandable_blockquote","offset":7,"length":9}`.
- `fold_caps_visible_at_three_lines`: text `"1\n2\n3\n4\n5"` → quote offset `6`.
- `fold_never_splits_code`: `render("intro\n\n```\nl1\nl2\nl3\nl4\n```\n\nend")`, then `fold`: the quote's offset is ≤ the `pre` entity's offset, and no entity starts inside the `pre` range except the `pre` itself.

- [ ] **Step 2: Write failing tests in `src/phone.rs`**
- Update `stop_lists_todos`: text is `"🟠 [X]\ndone\n[x] A\n[ ] B"` and entities contain `{"type":"pre","offset":12,"length":11}` (UTF-16: `🟠` counts 2).
- Update `stop_formats_the_answer_but_not_todos`: text `"🟠 [X]\nhi\n[ ] **B**"`; entities are bold `{offset 7, length 2}` and `pre` over the todo line.
- New `stop_cut_keeps_todos`: `last_assistant_message` = `"x".repeat(5000)`, one todo `T` → text ends with `"... (full answer in the terminal)\n[ ] T"`.

- [ ] **Step 3: Run** `cargo test` — Expected: new/updated tests FAIL.

- [ ] **Step 4: Implement `fold`, then rewire the Stop arm**
Stop: `(answer, spans) = render(...)`, `spans = fold(&answer, spans)`; the message text is `head + "\n" + cut(&answer)`, with entities from `place(spans, head_with_newline, &answer)`. If todos exist, append `"\n"` (only when the answer is non-empty) plus the todo lines joined by `"\n"`, and push one `pre` entity over exactly the todo lines (offset = UTF-16 length of everything before them). Non-Stop messages are unchanged.

- [ ] **Step 5: Run** `cargo test` — Expected: all PASS.

- [ ] **Step 6: Commit** `git commit -am "feat(phone): collapse reply details, todos in code font"`

---

### Task 3: One-tap buttons with visible outcomes

**Files:**
- Modify: `src/telegram.rs` (`Incoming::Button`, `parse_update`, `Out`, `send_loop`), `src/phone.rs` (`ack`, new `settle`, `send_to`, `incoming`, `tap`, `pick_question`, `Action::Answer`), `src/ui/mod.rs` (Core `Action::Answer` handling, tests building `Incoming::Button`)
- Modify: `docs/superpowers/specs/2026-11-20-phone-readability-design.md`: change the multi-select line to "Toggle taps keep the message unchanged (toast shows the picks); only the final submit edits it." (No re-render: YAGNI, `Out::Edit` needs no buttons.)

**Interfaces:**
- Produces in `src/telegram.rs`:
  - `pub struct Tap { pub callback_id: String, pub message_id: i64, pub text: String, pub entities: Vec<Value> }` (`text` empty when Telegram sent none).
  - `Incoming::Button { chat: i64, data: String, date: i64, tap: Tap }` (replaces `callback_id`).
  - `Out::Edit { message_id: i64, text: String, entities: Vec<Value> }` → `editMessageText` with `chat_id`, `message_id`, `text`, `entities`, no `reply_markup`.
- Produces in `src/phone.rs`:
  - `pub fn ack(&self, tap: Tap, toast: &str)`: answer only (toggle taps, unknown data).
  - `pub fn settle(&self, tap: Tap, outcome: &str)`: `Out::Answer` (toast = outcome), then, if `tap.text` is non-empty, `Out::Edit` with text `tap.text + "\n" + outcome` and `tap.entities`.
  - `Action::Answer(u64, Reply, Option<Tap>)`.
  - `fn send_to(&mut self, sid: &str, text: &str, sessions: &[Session]) -> String`: returns the outcome (`Sent to [p]` / `Queued: [p] is busy` / `[p] can't receive messages right now`) instead of saying it.

- [ ] **Step 1: Write failing tests**
- `telegram.rs` `parse_button`: callback with `"message": {"message_id": 9, "chat": {"id": 5}, "date": 1800, "text": "hi", "entities": [{"type":"bold","offset":0,"length":2}]}` → `tap.message_id == 9`, `tap.text == "hi"`, one entity; without `text` → `tap.text == ""`.
- `phone.rs`: typed message with one idle session → `sent_texts(&rx) == vec!["Sent to [X]"]`.
- `phone.rs`: session-picker tap `s:<id>` → rx yields `Out::Answer { text: "Sent to [X]" }` then `Out::Edit { message_id: 9, text: "Which session?\nSent to [X]", .. }`.
- `phone.rs`: stale button (date < started) → `Out::Answer` text `"Too late, answered in the terminal"` then `Out::Edit` ending in that line.
- `phone.rs`: same stale tap with empty `tap.text` → only `Out::Answer`, no `Out::Edit`.
- `phone.rs`: multi-select toggle tap → only `Out::Answer` with `"Picked: A"`, no `Out::Edit`.
- `ui/mod.rs`: Allow tap on a live card → `Out::Edit` text ends `"\nAllowed"`; Deny → `"\nDenied"`; second Allow tap on the same id → ends `"\nAlready answered"`. Update every existing `Incoming::Button { .. }` construction to the new shape (add a `tap(id: &str)` test helper with `message_id: 9, text: "card"`).

- [ ] **Step 2: Run** `cargo test` — Expected: compile errors/FAILs on the new shapes.

- [ ] **Step 3: Implement**
- `parse_update` fills `Tap` from `callback_query.id` and `callback_query.message.{message_id,text,entities}`.
- `send_loop` handles `Out::Edit` (no session, no tag, same 3-try loop; the last try drops `entities` as today).
- Phone: final taps (`q:` answer/done, `w:`, `s:`, stale, `gone`) call `settle`; toggle and unrecognised taps call `ack`. A finished answer still goes to Core as `Action::Answer(id, r, Some(tap))`. A `record` that sends the next question settles with `Answered`.
- Core: outcome = `""` → `Allowed` / `Denied` / `Answered` by `Reply`; otherwise the existing text. Call `self.phone.settle(tap, outcome)`.
- Typed text path: `let o = self.send_to(...); self.say(o, vec![])`. Picker path: `self.settle(tap, &self.send_to(...))`.
- Apply the spec wording change listed under Files.

- [ ] **Step 4: Run** `cargo test` — Expected: all PASS.

- [ ] **Step 5: Commit** `git commit -am "feat(phone): buttons vanish after a tap and show the outcome"`

---

### Task 4: STE instruction while Away

**Files:**
- Modify: `src/hook.rs` (`read_reply`, `forward_with_token`, new `prompt_output`, `STE` const), `src/server.rs` (`serve`)
- Test: tests modules in both files

**Interfaces:**
- Produces: `fn read_reply(f: File, wait: Duration) -> Option<Value>` (existing callers pass `Duration::from_secs(605)`); `pub fn prompt_output(reply: &Option<Value>) -> Option<String>`; `const STE: &str`.
- Pipe protocol: for a `UserPromptSubmit` event, `serve` writes `{"away":true}` or `{"away":false}` + `\n` to the pipe, then forwards the event as today.

STE text (exact):
> The user reads this reply on a phone. Write in ASD-STE100 Simplified Technical English. First line: the result. Then: what you need from the user, if anything. Then details. One idea per sentence. Max 20 words per sentence. Active voice. No filler, no pleasantries. Keep code, paths, commands and error text exact.

- [ ] **Step 1: Write failing tests**
- `hook.rs` `prompt_output_only_when_away`: `prompt_output(&Some(json!({"away": true})))` parses to `hookSpecificOutput.hookEventName == "UserPromptSubmit"` and `additionalContext == STE`; `{"away": false}`, `None` and `{}` → `None`.
- `server.rs` `prompt_submit_gets_away_flag`: using the `send_with` helper with a `UserPromptSubmit` envelope and `away = true`, the client end reads `{"away":true}`; with `away = false`, `{"away":false}`; the `Msg::Event` is still sent.
- `hook.rs` `prompt_reply_wait_is_short`: `read_reply` on a pipe whose server never writes returns `None` in under 1 s with `Duration::from_millis(300)` (use the test pipe helpers already in `hook.rs`/`server.rs` tests).

- [ ] **Step 2: Run** `cargo test` — Expected: FAIL.

- [ ] **Step 3: Implement**
- `serve`: when `raw["hook_event_name"] == "UserPromptSubmit"`, write the away line before `tx.send`.
- `forward_with_token`: for `UserPromptSubmit`, after writing the message, `return prompt_output(&read_reply(f, Duration::from_millis(300)))`.

- [ ] **Step 4: Run** `cargo test` — Expected: all PASS.

- [ ] **Step 5: Commit** `git commit -am "feat(hook): STE reply instruction while away"`

---

### Task 5: Real-phone check

- [ ] **Step 1:** `cargo build --release`, install the way the project does (see `README` / recent release commits), restart the island.
- [ ] **Step 2:** With Away on and 2 sessions: send a prompt from the phone. Check: reply starts with a short result line, details are collapsed, todos are in a code font, the header reads `🟠 [<repo>]`, and there's no `You (PC)` message.
- [ ] **Step 3:** Trigger a gated command, tap Allow. The buttons vanish and `Allowed` is appended. Tap a session picker: the buttons vanish and `Sent to [<repo>]` appears.
- [ ] **Step 4:** Report the results. Don't merge to `main` without the user's go-ahead.
