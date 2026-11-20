# Phone commands while away: design

Date: 2026-11-19
Status: approved
Builds on: `2026-11-18-away-mode-design.md`, `2026-11-17-telegram-phone-design.md`

## Intent

When the user asks Claude to open a terminal or another project, the result is inconsistent: sometimes a subprocess, sometimes an uncolored terminal, sometimes the wrong folder. The user works with zoxide (`z <name>`).

The user wants a plain command runner, with no AI in the loop, used from the phone while away:

- They send a short command to the Telegram bot.
- The PC opens a Windows Terminal tab in the right folder, the way `z <name>` would. It optionally starts `claude` there.
- That new Claude session then shows up on Telegram like any other, so the user can drive it from the phone.

This has to work with no Claude session running. Today the island only runs while a Claude session exists.

## Goals

- Telegram commands from the paired chat, accepted only while Away is on:
  - `/open <name>`: a new Windows Terminal tab in the folder `zoxide query <name>` returns.
  - `/claude <name>`: the same, and runs `claude` in that tab.
  - `/help`: lists the commands.
- Every command gets a Telegram reply saying what ran, or why nothing ran.
- The island runs independently of Claude while Away is on:
  - It survives the last session ending.
  - It starts at Windows login.
  - It has the bot token without any Claude hook.
- Memory use stays flat over days of running.

## Non-goals

- Commands from the island UI or a hotkey. Phone only.
- A user-editable command map. The commands are fixed in code.
- Typing into, or reusing, an existing terminal. Every command opens a new tab.
- Commands while Away is off.
- Natural language. Only the exact commands above.

## Anti-goals (a failure even if it works)

- Any path from Telegram text into a shell parser: `cmd`, `powershell`, or `wt`'s own `;` splitting.
- A command accepted from any chat other than the paired one, or while Away is off.
- The token written in plain text anywhere: disk, logs, the registry, or Telegram.
- Typed commands written to disk or logged.
- An island that stays running after the user turns Away off with no sessions open.
- Memory growth proportional to uptime.

## Constraints and facts

- **The token arrives only through hooks.** It comes from the plugin option `CLAUDE_PLUGIN_OPTION_TELEGRAM_BOT_TOKEN`, read in `hook.rs:213`, and travels in the pipe envelope. `Msg::Event.token` handles it: `Some("")` means the link was turned off.
- **Messages from before the island started are dropped** (`Phone.started`, `phone.rs:273`). Commands therefore work only while the island is already running.
- **Exit rule:** `Core::tick` exits 3 s after the state is empty.
- **Away** is persisted as `settings.json` key `away`. It is only on while paired: `set_away` refuses otherwise, and clearing the token or disconnecting turns it off.
- **Plain text** goes to a session, or answers `No open sessions`. Text starting with `/` currently gets no special handling.
- **Tools on this machine:**
  - `zoxide.exe` is installed through winget, in the user's PATH.
  - `wt.exe` is an app execution alias in `%LOCALAPPDATA%\Microsoft\WindowsApps`.
  - `pwsh` is not installed; Windows PowerShell 5.1 is.
- **Rust `std::process::Command` on Windows does not search the current directory** for the executable (fixed in 1.58). Arguments are quoted per the MSVCRT rules.
- **`wt.exe` splits its own command line on `;`**, even inside an argument. It treats `;` as a new-tab subcommand.
- **Two lists in `Phone` are never pruned:** `timed_out` and `ask_msgs`. They grow per held question for the island's whole life, harmless today only because the island exits.

## Approaches

| Approach | Trade-off | Decision |
|---|---|---|
| Hotkey command box on the PC | Instant, local. Useless away from the PC. | Rejected: the user wants this while away |
| **Telegram `/` commands while Away is on** | Reuses the pairing, Away, and the link. Phone access to the PC needs a tight allowlist. | **Chosen** |
| Token in RAM only; island stays alive while Away is on | No secret on disk. After a reboot or crash, the island is off until a Claude session starts. | Rejected by the user |
| **Token DPAPI-encrypted in `settings.json`, island starts at login** | Survives reboots. A stored secret: malware running as the user could decrypt it. | **Chosen by the user** |
| User-editable `commands.json` | Flexible. A second place that can run code, and a template language to secure. | Rejected (YAGNI) |

## Design

### 1. Token at rest (DPAPI)

- **Storing:** when the island receives a non-empty token from a hook that differs from the stored one, it encrypts it and saves it.
  - Encryption: `CryptProtectData`, current user scope, no UI (`CRYPTPROTECT_UI_FORBIDDEN`), with a fixed entropy constant `b"agent-islands phone token v1"`.
  - Storage: `settings.json` key `phone_token`, as a hex string. Saved through `settings::set`, read-modify-write like `side`.
- **Clearing:** when a hook sends `Some("")` (the user turned the link off), the island deletes `phone_token` and does what it does today: Away goes off.
- **Loading:** at island start, if `phone_token` decrypts, the island calls `phone.set_token` before any hook arrives. Decryption failure leaves the token unset (another user, a copied file, or a corrupt value).
- **Precedence:** a token from a hook always wins over the stored one.
- **Disconnect** (unpairing) keeps the stored token. It belongs to the plugin config, not to the pairing.
- The token never goes to logs, Telegram, or the registry. The decrypted buffer is freed with `LocalFree` right after it is copied into the `String`.

### 2. Lifetime and login start

- **Exit rule** becomes: exit 3 s after the state is empty **and** Away is off. While Away is on, the island stays up with zero sessions.
- **Run key:** `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`, value `AgentIslands`, data `"<current exe path>"` with no arguments, which runs `ui::run`.
  - **Away turned on:** write the value.
  - **Away turned off,** including the automatic off when pairing or the token is lost: delete the value.
  - **Island start while Away is on:** rewrite the value with the current exe path. Plugin updates move the exe to a new cache folder.
- **At login:**
  - Away persisted on, token decrypts, still paired: the island runs, the link connects, and commands work.
  - Otherwise: Away is off or turns off, the Run value is deleted, and the island exits after 3 s as today.
- **Existing single-instance and replacement rules are unchanged.** A login-started island owns the pipe, and a newer hook replaces an older island as today. The replacement island rewrites the Run value because Away is persisted on.

### 3. Commands

**Parsing.** Text from the paired chat whose first character is `/` is a command, never a session prompt.

- Format: `/<verb>` or `/<verb> <name>`. The verb is ASCII and compared case-insensitively.
- Telegram's `/verb@BotName` form is accepted; the `@BotName` suffix is stripped.

**Gate, checked in this order:**

| Condition | Reply | Effect |
|---|---|---|
| Not from the paired chat | none | ignored, like today |
| Away is off | `Turn Away on to use commands` | nothing |
| Unknown verb | `Unknown command. /help` | nothing |

**`<name>` validation:**

- After trimming, 1 to 100 characters.
- No control characters.
- Otherwise reply `Usage: /open <name>` and run nothing.

**Folder lookup** (on a worker thread, never the UI thread):

1. Run `zoxide query -- <name>`:
   - as `Command::new("zoxide").args(["query", "--", name])`;
   - with no window (`CREATE_NO_WINDOW`) and stdin null;
   - with a 5 s timeout. On timeout, kill it.
   - The `--` stops a name starting with `-` from being read as a flag.
2. Take stdout, UTF-8, and trim the trailing newline. It is accepted only if:
   - exit code 0;
   - it is an absolute path to an existing directory;
   - it contains no `;` and no `"`.
3. Failure replies:
   - no match: `No folder matches <name>`;
   - zoxide not found: `zoxide is not installed`;
   - any other failure: `Could not look up <name>`.

**Launch.** Only fixed argv items; the only variable is `<dir>`.

- `/open`: `wt.exe -w new -d <dir>`. Opens the default profile, so the user's colors and shell apply.
- `/claude`: `wt.exe -w new -d <dir> powershell.exe -NoLogo -NoExit -Command claude`. When `claude` exits, the tab stays open at a prompt in `<dir>`. The literal `claude` is fixed text.
- **The `Child` handle is dropped right away.** Nothing waits on wt, which exits once it hands off to the terminal.
- **Reply:**
  - success: `Opened <dir>` or `Started claude in <dir>`;
  - spawn error: `Could not open Windows Terminal`.

**`/help`** replies with the three commands and one line each.

**What `/claude` gets:** its normal permissions. Never `--dangerously-skip-permissions` or any permission-mode flag, so the command gate and permission prompts still reach the phone. The new session's `SessionStart` hook reaches the island like any other, and the phone gets `Started`.

### 4. Memory over long uptime

- **`timed_out` and `ask_msgs`:** when a session is forgotten (`Phone::forget`) or its asks are dropped, remove that session's request ids from `timed_out` and its message ids from `ask_msgs`.
  - Also cap each at 256 entries by dropping the oldest. Late taps on very old questions then get no reply at all, which is acceptable.
- **No new unbounded state.** Commands keep no history, no per-command records, and no stored text.
- **Soak check:** a debug-only test path runs 1,000 command parses plus the routing, with zoxide and wt stubbed by a test seam. It asserts every `Phone` collection is back to its starting size.
  - A manual check: run the island with Away on for an hour of idle time and some commands. Private bytes in Task Manager should stay flat (within a few MB).

### 5. Settings card

- No new controls. The Away row's sub-line becomes `Telegram + phone commands`.
- When Away is on, the island also starts at login. Say this in the sub-line or the README, builder's call on which (see below).

## What the user decided

- Phone (Telegram) only, and only while Away is on.
- The island runs without Claude: the token is stored DPAPI-encrypted and the island starts at login (option B).
- Opening a folder works like `z <name>`, through zoxide.
- `claude` can be started in the opened terminal from the phone.
- Security and memory are hard requirements: no leak, no shell injection.

## Calls I made (user can override)

- **The commands:** `/open`, `/claude`, `/help`, fixed in code with no config file.
- **Always a new tab** (`-w new` opens a new window; the builder may choose `-w 0 nt` to open a tab in the most recent window, if the spike shows it behaves). Never keystrokes into an existing terminal.
- **The shell:** `powershell.exe -NoExit` hosts `claude`, because `pwsh` is not installed.
- **The Run key exists only while Away is on.** Turning Away off removes it.
- **Disconnect keeps the stored token,** and clearing the plugin option removes it.

## Left to the builder

- Exact module placement. Probably a new `src/command.rs` for parse, validate, and launch, and DPAPI helpers in `settings.rs`.
- The test seam for zoxide and wt (for example, a function pointer on `Phone`).
- Whether the "starts at login" note lives in the settings sub-line or the README.
- New `windows` crate features: `Win32_Security_Cryptography` and `Win32_System_Registry`.

## Risks to verify in a spike before building

- **Folder trust prompt.** `claude` in a folder it has never trusted shows a trust dialog in the terminal, which the phone can't answer. Confirm what happens. If it blocks, `/claude` replies `If this folder is new to Claude, accept the trust prompt on the PC` (or the builder finds a supported way to detect it).
- **`wt.exe` started from a login-started island** (no console, `windows_subsystem = "windows"`) opens a visible terminal window with the user's default profile.
- **Phone prompts reach the session.** `phone.ts` registers its `listen` pipe for a session started this way, so text from the phone reaches it.

## Settled after the builder check (my calls unless marked)

- **Empty island while away:** the pill stays visible with the amber Away outline and zero blobs. Its right-click settings card still works, so Away can be turned off at the PC with no session open.
- **Network at login:** no work needed. The `getUpdates` poller already retries with a sleep and reports `LinkProblem`. Messages sent before the island started are still dropped, and the `Not delivered` notice already covers that.
- **Only the three exact verbs are commands.** `/open`, `/claude` and `/help` (optionally with `@BotName`) are commands. Any other `/` text (`/clear`, `/compact`, and so on) goes to a session as it does today. There is no `Unknown command` reply. The Away-off reply goes only to the paired chat, so it reveals nothing.
- **Several words** are split on whitespace into separate zoxide keywords, as `z foo bar` does: `zoxide query -- foo bar`. There are at most 8 words, and the total is capped at 100 characters.
- **Tabs:** the default is `wt.exe -w 0 nt -d <dir> ...`, a new tab in the most recent window, which opens a window if none exists. If the spike shows that fails from a windowless process, fall back to `-w new`.
- **The returned folder:** trim one trailing `\` unless it is a drive root (`C:\`). The spike confirms `wt -d C:\` works; if it doesn't, reply `Could not open <dir>`.
- **The spike runs first and gates the build.** If `wt` opens nothing from a login-started island, or phone text doesn't reach a `/claude` session, stop and go back to the user. Don't ship part of the feature quietly.
- **Memory tests:** unit tests on `forget`, the drop path and the 256 cap for `timed_out` and `ask_msgs`. The stubbed soak test stays as well.

## User decisions after the builder check

- **The token is on disk only while Away is on.** This narrows section 1:
  - The token is stored when Away turns on, or when a hook delivers a new token while Away is on.
  - Turning Away off, by hand or automatically, deletes both `phone_token` and the Run value.
  - With Away off the island exits when empty. The next Claude session's hook provides the token again.
- **Leftovers after an update or uninstall: option a.**
  - The README documents both gaps:
    - "update, then reboot before opening Claude": no island at login until the next Claude session;
    - uninstalling with Away on leaves the Run value and the encrypted token behind.
  - New subcommand `agent-islands.exe forget`. It deletes `phone_token`, sets `away` to false, and deletes the Run value. It prints what it removed and exits 0. It works whether or not the island is running. A running island also turns Away off when it next reads settings; the builder decides how, for example the pipe or the next start.

## Open questions

None.

## Revision 2026-11-20: listening while paired (user decision)

The user pointed out a lockout: forget to turn Away on before the last session closes, and no command can ever run. The fix, which replaces the "only while Away is on" lifetime rules above:

- **While a phone is paired and a token is known, the island keeps running.** This is called **"Listening for commands"**.
  - It survives the last session ending.
  - It starts at login.
  - The token stays DPAPI-encrypted in `settings.json` while paired.
- **Unpairing removes both the token and the Run value**, as does clearing the token or running `agent-islands.exe forget`. With no pairing, the island exits when empty, as before.
- **New phone commands `/away on` and `/away off`.**
  - They are accepted from the paired chat whether Away is on or off.
  - They reply `Away on` / `Away off`.
  - `/away on` follows the same rule as the switch: it needs pairing, which is a given in the paired chat.
- **Away still gates `/open` and `/claude`.**
- **Display:** on screen, not in the tray, because that is the least code. While listening with no sessions, the empty pill stays visible: amber outline when Away is on, normal outline otherwise. Its settings card works as today. The Away sub-line becomes `Phone commands on; starts at login`, and a `Listening for commands` line shows under Phone when paired.
- **`/help`** lists `/away on|off` too.
