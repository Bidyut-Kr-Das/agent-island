# Telegram phone link: design

Date: 2026-11-17
Status: approved

## Intent

The user wants to follow and drive their Claude Code sessions from their phone when they are away from the PC. On the phone they want two things:

1. **Updates**: what each session is doing, as a normal chat conversation, one new message per event (never editing an old message in place).
2. **Instructions**: replying to a session's message in Telegram becomes the next prompt of that exact session, as if typed at the PC, even when the session has been idle for hours.

Permission prompts and gated commands, which the island already shows as Allow/Deny cards, must also be answerable from the phone.

## Goals

- One Telegram bot, owned by the island, serves every open session.
- A reply to a session's message is submitted to that session as the user's own prompt (it shows in the terminal as the user's prompt line, not as a system note).
- Allow/Deny from the phone uses the same decision path as a click on the island card.
- Off by default. Nothing talks to Telegram until the user enters a bot token and pairs their phone.
- Near-zero cost while idle: no polling loops on the PC side.

## Non-goals (v1)

- One message per tool call. Tool activity is not mirrored; gated commands already reach the phone as Allow/Deny.
- Away-from-PC detection or quiet hours. Every message notifies normally.
- Answering AskUserQuestion dialogs from the phone. The phone gets a notification only.
- Voice notes. The user dictates with the phone keyboard's mic button, which arrives as text.
- Rich formatting. Messages are plain text.
- Editing or deleting earlier Telegram messages.
- A web app, Tailscale, a cloud relay, or any open inbound port.

## Anti-goals (a failure even if it works)

- Any chat other than the paired one can see updates or drive a session.
- A session started with plain `claude` needs a special flag or launch command to receive phone prompts.
- Prompts from the phone arrive as system notes instead of as the user's own prompt.
- Leftover processes or growing memory after sessions close.

## Constraints and facts established

- Windows 10/11, Rust 2024. The island is `agent-islands.exe` with an egui overlay; the hook client is the same exe run as `agent-islands.exe hook`. Hook events reach the island over the named pipe `\\.\pipe\agent-islands-<USERNAME>` (`src/pipe.rs`, `src/server.rs`).
- The island exits about 3 seconds after its last session ends, and is restarted by the next hook event.
- Permission cards wait 30 s in the island (`server.rs`) and the hook entries allow 40 s (`plugin/hooks/hooks.json`). After that the terminal prompt takes over (permission request) or the command is denied (gated command).
- Hook payloads already carry what the messages need: `UserPromptSubmit.prompt`, `Stop.last_assistant_message`, `Notification.message` and `notification_type`, the PermissionRequest/gated PreToolUse fields the island already summarizes in `detail.rs`.
- Session name: the git repo folder name, as `state::display_parts` gives it for the blobs. Session color: an index 0..5 into `ui::theme::SESSION` (orange, blue, purple, pink, teal, indigo).

### Spike results (2026-11-17, throwaway, deleted)

- **asyncRewake Stop hook** can wake an idle session, and a waiter survived 3 h idle. Rejected: the text arrives as a system note, not as the user's prompt.
- **Claude Code mod + `$.prompt.submit({ text, asUser: true })`**: a mod's `session.start` hook spawned a helper with `$.process.spawn`. The helper blocked on a named pipe. A line pushed into the pipe was submitted as the user's prompt within 1 s (`$.prompt.submit (phone-spike): "what is 2+2?"` in the debug log, and the prompt hooks ran on it like a typed prompt).
- **Helper cost** (Rust, std + `windows` crate): 0.5 MB private memory, 1 thread, 0 ms CPU over 29 min idle; 5 helpers about 2.6 MB in total. When the session closed, Claude Code killed the helper (0.1 s before `SessionEnd`) and no process was left.
- **Channels** (`notifications/claude/channel`) would also work but need `--dangerously-load-development-channels` on every launch, because a custom channel is not on Anthropic's allowlist. Rejected.

## Approaches considered

| Approach | Trade-off | Decision |
|---|---|---|
| **Telegram bot, island owns it** | No server, no open port, push notifications for free. Messages pass through Telegram's servers. | **Chosen** by the user |
| Tailscale + island web app | Own pipe, private, but needs a web app, web push, and Tailscale on both devices | Not chosen |
| Own cloud relay, LAN page, public tunnel, ntfy | More work, home-only, public attack surface, or buttons-only respectively | Not chosen |

For delivering a phone reply into a session:

| Approach | Trade-off | Decision |
|---|---|---|
| **Mod in the plugin + one helper process per session, waiting on a pipe** | Real user prompt, any session, no flag. One 0.5 MB idle process per session. | **Chosen** by the user ("stick to long running process") |
| Mod holding a long HTTP request to the island | No helper processes, but unknown request time limits | Not chosen |
| Channels | Real mechanism, but a dev flag on every launch | Rejected (flag) |
| asyncRewake Stop hook | No flag, but arrives as a system note | Rejected (not a real prompt) |
| Writing into the console input / simulated keys | Fragile, can fight the user's typing or steal focus | Rejected |
| Mod polling a file | Polling | Rejected by the user |

## Design

### Parts

```text
Phone (Telegram) <-> api.telegram.org <-> island: telegram thread (HTTPS out only)
                                              |
                       island core (state, cards, routing)
                                              |  existing named pipe
            +---------------------------------+------------------------------+
   agent-islands.exe hook               agent-islands.exe listen <session_id>   (one per session)
   (hook events in, decisions out)       started by the plugin's mod; prints each
                                         pushed prompt line; the mod submits it
                                         with $.prompt.submit({ text, asUser: true })
```

1. **Telegram thread (new, in the island).** Starts only when a bot token is present. Long-polls `getUpdates` (one request held open by Telegram, `timeout` about 50 s), sends messages with `sendMessage`, answers button taps with `answerCallbackQuery`. HTTPS client: a new dependency (`ureq` with rustls is the expected pick; builder's call).
2. **Listener helper (new mode of the same exe).** `agent-islands.exe listen <session_id>` connects to the island's existing pipe, sends a `listen` envelope with the session id, then blocks reading. Each line the island writes is one prompt; the helper prints it to stdout. If the island is not running it starts it, like the hook client does. If the pipe breaks (island restarted), it reconnects.
3. **Mod (new, in the plugin).** A hooks module listed under `modules` in the plugin. On `session.start` it spawns the helper with the session id (`$.session.id()`) and the exe under `$.plugin.root`, reads its stdout line by line, and calls `$.prompt.submit({ text, asUser: true })` for each line. Claude Code kills the helper when the session ends.
4. **Island core changes.** Keeps one listener connection per session id (newest wins). Routes a phone reply to the right listener. Turns hook events into outgoing Telegram messages. Accepts Allow/Deny from Telegram through the same answer path as a card click.

### Setup and pairing (off by default)

1. The plugin declares an optional, sensitive `userConfig` field: `telegram_bot_token`. Install shows it; leaving it blank keeps the feature off. It can be set or cleared later under `/plugin`, agent-islands, configure. Claude Code stores sensitive values in its credentials store (`~/.claude/.credentials.json` on Windows).
2. Claude Code exports it to hook processes as `CLAUDE_PLUGIN_OPTION_TELEGRAM_BOT_TOKEN`. The hook client passes it on to the island (the island it spawns inherits the environment; an island that is already running gets it from the next hook envelope).
3. With a token and no paired chat, the island panel shows a pairing line: `Phone: send 482913 to @<bot username>` (username from `getMe`, code random 6 digits, new each island start).
4. The first chat that sends exactly that code is paired. The island saves its chat id in `%APPDATA%\agent-islands\settings.json` (next to `side`) and replies `Paired`. Updates from any other chat are ignored, with no reply.
5. The island's right-click menu gets **Disconnect phone**, which forgets the chat id. Clearing the token in `/plugin` turns the feature fully off.

### Outgoing messages

Every message is a new message, plain text, normal notification. First line: a color circle and the session name. No other emojis.

Color circle per palette index: 0 orange `🟠`, 1 blue `🔵`, 2 purple `🟣`, 3 pink `🔴`, 4 teal `🟢`, 5 indigo `⚫` (builder's call on indigo; see Open questions).

| Event | Message body |
|---|---|
| `SessionStart` (new session) | `Started` |
| `UserPromptSubmit` typed at the PC | `You (PC): <prompt>` |
| `UserPromptSubmit` that came from the phone | Not sent (the user already has it in the chat) |
| PermissionRequest / gated PreToolUse (a card) | `Permission: <verb> <detail>` with inline buttons Allow / Deny |
| `Stop` | `last_assistant_message`, then the todo list as `[x] subject` / `[ ] subject`, one line each |
| `StopFailure` | `Error: <message>` |
| `Notification` waiting for input, or AskUserQuestion | `Waiting for you: <message or question text>` |
| `SessionEnd` | `Ended` |
| Tool calls, todo changes | Not sent |

Long texts are cut to fit Telegram's 4096-character limit (about the first 3500 characters, then `... (full answer in the terminal)`).

### Incoming messages

- **Reply to a session's message:** the island maps the replied-to message id to its session and writes the text to that session's listener. The island keeps a map from sent message id to session id for open sessions.
- **Message that is not a reply:** with exactly one open session it goes there. With several, the bot answers `Which session?` with one button per session (color circle + name); tapping a button delivers the held text.
- **Session has no listener** (closed, or the mod is not loaded): the bot answers `<name> can't receive messages right now`.
- **Allow / Deny button:** `answerCallbackQuery`, then the same `Core::answer(req_id, Reply)` a card click uses. A tap on a card that is already answered or timed out gets `Already answered` or `Too late, answered in the terminal`.
- **Pairing code** from an unpaired chat: as in Setup.
- Anything from a chat that is not the paired one: ignored.

### Lifetime and failure

- Telegram thread lives and dies with the island. Network errors: retry with backoff, never crash the island.
- Helper per session: started at `session.start`, killed by Claude Code at session end (verified in the spike).
- Island restart: helpers reconnect; the sent-message-to-session map is lost, so replies to messages sent before the restart fall back to the "not a reply" rule.

## What the user decided

- Telegram over Tailscale and the other transports.
- New message per event, a real conversation; no in-place edits.
- Reply goes to that session as the user's real prompt; long-running helper per session, not polling and not an HTTP long request.
- Setup flow: optional token in the plugin settings, pairing code on the island, Disconnect in the right-click menu.
- Header: color circle + session name only; no other emojis.
- No away detection; every message notifies.
- Events: the set in the Outgoing table (option A); no per-tool-call messages.
- Voice: phone keyboard dictation for now.
- Permission window stays 30 s; the existing gate pause covers being away.

## Left to the builder

- HTTPS client crate and JSON shapes for the Bot API calls.
- Envelope format for `listen` on the existing pipe.
- How the hook client hands the token to an island that is already running.
- Reconnect backoff values.
- Exact cut length for long messages.
- Whether `hooks.json` can hold both the existing `hooks` and the new `modules` key, or the mod needs its own entry (the docs do not say; check with `claude plugin validate`).

## Calls made after the builder check (user can override)

- **Reply while the session is busy.** `$.prompt.submit` waits until the session is idle, then starts a turn (mods API docs), so a reply sent mid-turn or while a dialog is open is queued, not dropped or typed into the dialog. If the session is not idle when the island delivers the text, the bot answers `Queued: <name> is busy`. If it is idle, no extra message; the next updates show it ran.
- **Phone prompts not echoed.** The island remembers the text it last pushed to each session. The next `UserPromptSubmit` in that session with identical text is treated as the phone's and not sent. The only misfire is typing the same text at the PC at the same moment, which is acceptable.
- **Mod not available.** Updates and Allow/Deny come from hooks and work in every session. Replies need a Claude Code version with mods (`$.prompt.submit`, `$.process.spawn`); without it there is no listener and the bot answers `<name> can't receive messages right now`. If `hooks.json` can't hold `modules` next to `hooks`, the plan checks the manifest options before anything else is built.
- **Pairing brute force.** The code exists only while no chat is paired. After 5 wrong attempts (from any chat) or 10 minutes, the island makes a new code and shows it.
- **Settings file.** `settings.json` becomes read-modify-write so `side` and the paired chat id don't overwrite each other.
- **Indigo session color.** Telegram has no indigo circle; palette index 5 uses `⚫`.

## Open questions

None.

Resolved: the permission window stays 30 s, paired phone or not. A tap after that gets `Too late, answered in the terminal`. When away, the user pauses the command gate on the island, which the island already supports.
