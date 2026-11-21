# Readable phone messages: design

Date: 2026-11-20
Status: approved
Builds on: `2026-11-17-telegram-phone-design.md`, `2026-11-18-away-mode-design.md`, `2026-11-19-phone-commands-design.md`

## Intent

While away, the user drives 2-3 Claude agents from Telegram. After each turn they read the reply, mostly in full, and decide the next prompt. They also ask agents to research and report back. Detail is needed. The problem is volume: with three agents the chat becomes a wall of long text.

Today (`src/phone.rs` `message`):

- Every `Stop` sends the whole last reply (up to 3500 chars), then the todo list as plain `[x] ` / `[ ] ` lines.
- Every prompt typed on the PC is echoed as `You (PC): ...`.
- Inline buttons stay on the message forever. Taps after the first do nothing.
- Some actions give no visible feedback: an Allow/Deny tap (empty callback answer), and a typed message delivered to an idle session.

## Goals

1. **Short replies while away.** While Away is on, every prompt the agent receives carries an instruction to answer in ASD-STE100 Simplified Technical English: result first, then what is needed from the user, short sentences, active voice, no filler. The full reply still goes to the phone.
2. **Collapsed details.** In a `Stop` message, the opening of the reply is always visible. The rest sits in a Telegram `expandable_blockquote` (tap to expand). A short reply has no quote.
3. **Todos in a code font.** The full todo list, always visible, below the reply, as one `pre` block:
   ```
   [x] Read phone.rs
   [ ] Run tests
   ```
4. **Header `<circle> [<project>]`.** Keep the colored circle (it matches the island overlay). The project name (folder name, normally the GitHub repo name) goes in square brackets. Messages that need the user add a status word: `🟠 [payments-api] · permission`. No other emoji anywhere in phone text.
5. **Drop `You (PC)` messages.** `Started` and `Ended` stay: they show the user an agent is alive.
6. **Buttons go after one tap.** The tapped message is edited: buttons removed, an outcome line appended (`Allowed`, `Denied`, `Sent to [payments-api]`, `Too late, answered in the terminal`, `Already answered`). Applies to Allow/Deny (permissions and gated commands), the session picker, the question picker, and AskUserQuestion option buttons.
7. **Visible feedback for every action.** A typed message delivered to an idle session gets `Sent to [<project>]`. Existing replies (`Queued: X is busy`, `X can't receive messages right now`) stay, reworded to the bracket form.

## Non-goals

- The pinned all-agents board. Planned later, separately.
- One chat thread per agent, a reply queue, or AI summaries by a second model.
- Changing how replies render Markdown (`telegram::render`) beyond what the quote needs.
- STE when Away is off.

## Anti-goals

- Losing reply content. The full reply must still reach the phone (the 3500-char cut stays as it is).
- Hiding the todo list or the first lines in the collapsed quote.
- A tap that looks accepted but did nothing, or buttons left live after an answer.

## Approaches considered

- **Phone-shaped replies (STE instruction) + collapsed quote + noise cut.** Picked. Attacks length at the source, small change, reuses the `away` flag.
- **Reply queue, one agent at a time.** Rejected by the user: picking an agent, replying, then scrolling back up to the picker for the next agent is worse than reading one mixed chat.
- **One Telegram topic per agent.** Not chosen: biggest change, unverified for private bot chats.
- **Second-model summaries (Haiku).** Not chosen: cost and an API key; STE gets most of the gain for free.

## Design

### STE instruction (goal 1)

- `UserPromptSubmit` becomes a hook that waits for a one-line island reply, like gated `PreToolUse` does today.
- The island answers with whether Away is on. If on, the hook prints:
  `{"hookSpecificOutput":{"hookEventName":"UserPromptSubmit","additionalContext":"<STE text>"}}`
- No island or no reply: the hook prints nothing; the prompt goes through unchanged. A prompt must never be blocked by this.
- Covers prompts typed on the PC and phone prompts delivered through the `listen` pipe, since both arrive as `UserPromptSubmit`.
- STE text (constant in code, final wording is the builder's):
  > The user reads this reply on a phone. Write in ASD-STE100 Simplified Technical English. First line: the result. Then: what you need from the user, if anything. Then details. One idea per sentence. Max 20 words per sentence. Active voice. No filler, no pleasantries. Keep code, paths, commands and error text exact.

### Stop message layout (goals 2-4)

```
🟠 [payments-api]                 <- header line
Found the cause. Stop sends the full reply.   <- visible part
Changed phone.rs. Tests pass.
┃ Details ...                      <- expandable_blockquote, rest of reply
[x] Read phone.rs                  <- pre block, todos
[ ] Run tests
```

- Visible part: the reply up to the first blank line, at most 3 lines. Everything after goes in one `expandable_blockquote` entity.
- Telegram forbids nesting blockquotes and may reject `pre`/`code` inside other entities (see the existing `Doc::plain` comment). Entities from `render` that cannot sit inside the quote are dropped (text kept). The builder confirms the exact rule in the Bot API docs.
- The 3500-char cut applies to the reply only. The todo block is never cut.
- Status words in the header: `· permission` (Allow/Deny card, incl. gated), `· question` (AskUserQuestion, waiting notification), `· error` (`StopFailure`). The body drops its old `Permission:` / `Waiting for you:` / `Error:` prefixes.

### Header everywhere (goal 4)

- `head` in `message` becomes `<circle> [<project>]` (+ status). Same for the held-question messages and the session/question picker button labels.
- No other emoji in any phone text.

### Buttons and feedback (goals 6-7)

- `Incoming::Button` also carries the tapped message's `message_id`, `text` and `entities` (all present in `callback_query.message`).
- New `Out::Edit { message_id, text, entities }`: `editMessageText` with no `reply_markup`, which removes the buttons. Text is the original text + `\n` + outcome line. Stateless: the island stores nothing new.
- Every tap still gets `answerCallbackQuery` (Telegram needs it to stop the spinner). Then the edit.
- Allow/Deny: Core already picks the outcome (`""` means it answered; else "Too late..." / "Already answered"). Map `""` to `Allowed` / `Denied` for the edit line.
- Multi-select AskUserQuestion: Toggle taps keep the message unchanged (toast shows the picks); only the final submit edits it.
- `send_to` success on an idle session: `say("Sent to [<project>]")`. After a session-picker tap, the picker message itself is edited to the outcome instead of a new message.

## Calls I made (user may override)

- Same-project agents: no `[repo 2]` numbering. The colored circle already tells them apart.
- Visible part rule: first paragraph, max 3 lines.
- STE applies to every prompt while Away is on, including ones typed on the PC and `<task-notification>` prompts (their replies also reach the phone).

## Settled after builder review

- **Hook wait / version skew.** The `UserPromptSubmit` hook reads the island's reply with a short timeout (about 300 ms). Timeout, old island, no island, bad line: print nothing. The prompt is never delayed by more than that.
- **Split rule.** Split the *rendered* text (after `render`): the visible part ends at the first blank line or after the 3rd line break, whichever comes first. A one-paragraph reply of 3 lines or fewer has no quote. If the split falls inside a `pre`/`code` entity, move it to before that entity. Entities crossing the split are clipped to each side.
- **Code inside the quote.** Try keeping `pre`/`code` inside the `expandable_blockquote`. If Telegram rejects the message, the existing last-try-plain fallback in `send_loop` still delivers the text. The builder checks the Bot API rule and drops only what Telegram forbids.
- **3500-char cut.** Counted on the reply text, before the todo block is appended.
- **Multi-select questions.** Toggle taps keep the message unchanged; only the final submit edits it (`Out::Edit` carries no buttons).
- **Old messages.** If `callback_query.message` has no text (very old message), only `answerCallbackQuery` is sent, with the outcome as its toast text.

## Left to the builder

- Exact STE wording, within the rules above.
- How the island answers `UserPromptSubmit` on the existing pipe protocol (`server.rs` `serve`) and how long the hook waits (keep it short; the island is local).
- Entity-nesting fallback inside the quote.
- Test updates: the `phone.rs` tests assert exact texts like `"🟠 X\nStarted"` and `"You (PC): ..."`.
