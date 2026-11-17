# Away mode and phone answers to questions: design

Date: 2026-11-18
Status: approved
Builds on: `2026-11-17-telegram-phone-design.md` (branch `feat/telegram-phone`)

## Intent

In live use of the Telegram link, the user hit two problems:

- A question from Claude (the AskUserQuestion dialog) reached the phone as `Waiting for you: <question>` with no options.
- A text reply to that question became a new prompt, queued behind the open dialog (`Queued: <name> is busy`), so it could not answer the dialog.

The user wants one switch on the island: **Away**.

- **Away on:** everything goes to Telegram, including questions, which can be answered from the phone.
- **Away off:** everything stays on the PC, instantly, and Telegram is silent.

## Goals

- An `Away` switch in the island's right-click settings card. It replaces the `Gating On / Paused` row.
- **Away off:** Telegram sends nothing. Questions show in the terminal at once, as before. Gated commands and permission requests use the island card with the 30 s window, as before.
- **Away on:** Telegram gets every update the phone link sends today. Questions arrive with their options as buttons, and the phone's answer fills the dialog, which then never shows in the terminal. Permission requests and gated commands arrive with Allow / Deny.
- **Away on:** questions, permission requests and gated commands wait up to 10 minutes for an answer.

## Non-goals

- Detecting whether the user is away. The switch is manual.
- Answering questions from the island card. The island keeps its "Waiting for you" card with Go to terminal.
- Answer previews and per-option notes (`preview`, `annotations`) on the phone.

## Anti-goals

- At the desk (Away off), anything shows later than it did before this change.
- An answer from any chat other than the paired one.
- A question that silently never gets answered: after 10 minutes the terminal dialog must open.

## Constraints and facts

- **Spike (2026-11-18, passed):** a `PreToolUse` hook on `AskUserQuestion` printed the following, and Claude Code skipped the dialog and handed Claude the answer `Coffee`:

  ```json
  {"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"allow","updatedInput":{"questions":<as received>,"answers":{"<question text>":"<label>"}}}}
  ```

  `answers` is keyed by question text. A multi-select answer is the labels joined by `, `: this is the convention of the mods API's `$.ui.ask`, and was not exercised in the spike.
- A hook that waits blocks the dialog. Today:
  - The island waits 30 s (`server.rs`).
  - The hook client reads for 35 s (`hook.rs` `read_reply`).
  - `hooks.json` gives `PreToolUse` and `PermissionRequest` a 40 s timeout.
- The gate pause is an in-memory `AtomicBool` (`paused`) shared by the pipe server, the pill (amber outline) and the settings card.
- `Gating Paused` currently makes gated commands skip the gate entirely.

## Approaches

| Approach | Trade-off | Decision |
|---|---|---|
| **Away switch; a question waits in the hook only while away** | Proven pattern (the spike above, and the gated command path). No delay at the desk. | **Chosen** by the user |
| Phone races the terminal dialog through the mod | Dialog instant everywhere. Unknown whether an open dialog can be closed from a mod; needs a spike. | Not needed once Away decides who answers |
| Every question waits 30 s, even at the desk | Simple | Rejected: delays the dialog at the desk |

## Design

### The switch

- The settings card row `Gating: On / Paused` becomes `Away: Off / On`.
- The pill's amber outline now means Away is on.
- `Gating Paused`, the skip-the-gate behaviour, is removed: the user approved this. With Away off, gated commands always get the island card.
- Away is saved in `settings.json` as key `away`, read-modify-write like `side`. It survives island restarts. **Builder's call, see below.**

### Away off

- `Phone::on_event` sends nothing.
- These still work:
  - Pairing replies (`Paired`).
  - Delivery of a phone reply to its session.
  - Replies to taps.
- Questions are not held. Permission and gated cards wait 30 s, as today.

### Away on

- Permission requests and gated commands wait up to 600 s instead of 30 s. They arrive on the phone as today, with Allow / Deny.
- **Question flow:**
  1. The hook client sends the `PreToolUse AskUserQuestion` event and waits for a reply, like a gated command. It does so only when the island answers that Away is on. With Away off the island replies at once with "no hold", and the hook prints nothing.
  2. The island makes a question card and sends one Telegram message per question, in order:
     - Text: header line, then the question text.
     - One button per option, callback `q:<req>:<question index>:<option index>`.
     - Multi-select questions: a tap toggles the option, shown as `[x] label` in the button text. A `Done` button (`q:<req>:<qi>:done`) confirms.
     - A text reply to the question message is an "Other" answer.
  3. When every question has an answer, the island replies to the hook with the answers map. The hook prints `permissionDecision: allow` with `updatedInput = { questions (unchanged), answers }`.
  4. After 600 s with no full answer, the hook prints nothing and the terminal dialog opens. A later tap gets `Too late, answered in the terminal`.
- **Turning Away off releases everything waiting at once:**
  - Questions open in the terminal.
  - Permission requests and gated commands become normal 30 s island cards.

  Coming back to the desk never means waiting out the 10 minutes, and nothing already asked is lost.

### Timeouts

| Where | Today | New |
|---|---|---|
| Island wait, Away off | 30 s | 30 s |
| Island wait, Away on | 30 s | 600 s |
| Hook client read | 35 s | 605 s |
| `hooks.json`: `PreToolUse` and `PermissionRequest` | 40 s | 610 s |

The `hooks.json` timeout only caps how long a hook may block. Non-waiting hooks still return at once.

The gated-command timeout reason becomes `No answer in Agent Islands in time`. It covers both the 30 s and the 10 min window.

### Settings card visuals

The Away row lives in the settings card, which is being redesigned separately: the user found the current buttons dated. This design fixes only the row's content and behaviour: `Away`, two states, Off / On.

## What the user decided

- An Away switch replaces the Gating row. Away off: Telegram silent, everything instant on the PC. Away on: everything to Telegram.
- Questions answered from the phone, with options as buttons.
- 10-minute wait while away, then the terminal.
- `Gating Paused` (skip the gate) removed.
- Phone replies still delivered while Away is off. Only outgoing messages stop.
- "Other" answer by replying with text. Multi-select by toggling, then `Done`. Several questions sent one after another.

## Left to the builder

- Envelope and reply shapes for the question round trip on the pipe.
- Where the island stores answers in progress. Question cards are cleaned up with the session, like permission cards.

## Calls made after the builder check (user can override)

- **Away persisted in `settings.json`.** An island restart while away must not silently turn Telegram off.
- **Away needs a paired phone.**
  - While no phone is paired, or the token is empty, the Away row is disabled and reads `Pair a phone first`. Nothing is ever held with nobody able to answer.
  - If pairing is lost while away (Disconnect, or the token is cleared), Away turns off and everything waiting is released, as when the user turns it off.
- **Plain text while a question is open.** This covers a plain message, not a Telegram reply.
  - Exactly one question waiting on the phone: the text is that question's "Other" answer.
  - None waiting: the usual routing applies.
  - Several waiting: the bot asks `Which question?` with one button per waiting question, the same pattern as `Which session?`.
- **Answer formats are spiked first.** Before building the question flow, the builder repeats the spike for:
  - a multi-select answer (labels joined by `, `);
  - an "Other" free-text answer;
  - a call with two questions.

  A shape that fails is not offered on the phone. That question is not held and opens in the terminal at once, and the phone gets `Waiting for you: <question>` as today.
- **Away on holds every permission request and gated command**, not only questions, as the user asked ("everything goes to Telegram").

## Open questions

None.
