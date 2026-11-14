# Agent Islands: Design

Date: 2026-11-15
Status: approved look, design pending approval

## 1. Intent

The user runs several Claude Code sessions at once, each in its own Windows Terminal tab or window, on different projects. Today they have to cycle through terminals to learn which session is working, which is waiting on them, and which has finished.

Agent Islands is a small floating "dynamic island" at the top-left edge of the screen, in the spirit of Apple's Dynamic Island, that answers those questions at a glance and lets the user act on the one thing that most often blocks a session: a permission prompt.

It is an update display, not an app the user lives in. It must cost almost nothing in RAM and CPU. It runs only while at least one Claude Code session is open, and exits when the last one closes.

## 2. Goals, non-goals, anti-goals

### Goals

- Show one mascot (a "blob") per open Claude Code session, whatever its state: idle, working, waiting on the user, or finished. Five open sessions means five blobs. (Updated by the user on 2026-11-15; the earlier version showed only working and waiting sessions.)
- Clicking the island expands it. The user picks a session and sees that session's todo list: completed items with a check mark and the current item with a spinner. Pending items are hidden.
- When a session asks for permission, show what is being requested (tool and one-line detail, e.g. `Bash  npm install zod@3.23.8`) with Allow and Deny buttons that answer Claude Code directly.
- When a session waits on the user for anything else (a question via AskUserQuestion, an input request), show "Has a question for you" or "Waiting for you" plus a "Go to terminal" button that brings that session's Windows Terminal window (and, best effort, its tab) to the front.
- When a session finishes, show a "<project> finished" pill that stays until the user has seen it.
- Start automatically when a hook fires; exit automatically when the last Claude Code session closes. The user never launches it by hand.
- Lowest practical RAM and CPU.

### Non-goals (MVP)

- VS Code integrated terminal, other terminals, other agents (Codex etc.). Windows Terminal and Claude Code only.
- macOS / Linux.
- Media, maps, or any Apple-style island features unrelated to agent status.
- Showing full question text or answering questions from the island.
- Showing pending (not yet started) todos.
- History, settings UI, sounds, tray icon.

### Anti-goals (would be a failure even if it works)

- A background process that keeps running after every Claude Code session has closed.
- Stealing keyboard focus from the terminal the user is typing in when the island appears or changes.
- An Allow button that does not show exactly what is being allowed.
- Any path that can approve a permission without a real user click.
- Noticeable RAM/CPU cost (target below, Section 8).
- Emojis anywhere in UI, code, or docs (user preference).

## 3. Constraints

- Windows 11. Windows Terminal is the only supported terminal for the MVP.
- Claude Code 2.1.325 is installed. Rust 1.98 / cargo 1.98 are installed.
- Delivered as a Claude Code plugin: the plugin registers hooks; the hooks call one bundled executable.
- No network access. Everything is local.

## 4. Approaches considered

| | Approach | Trade-off |
|---|---|---|
| **A (picked)** | Rust, single native `agent-islands.exe`, UI drawn with egui | About 15-30 MB RAM, near-zero CPU, hook calls start in milliseconds. Apple-style motion has to be coded by hand. |
| B | Tauri (Rust + WebView2) | Easiest styling and motion via CSS, but WebView2 costs about 80-150 MB across processes: 4-5 times A. |
| C | C# WPF on .NET 8 | Good animation and built-in UI Automation, but about 60 MB RAM, needs the .NET runtime, and each hook call pays 100 ms or more of startup. |

Rejected outright: Electron (too heavy), Python/AutoHotkey (cannot draw a clean transparent rounded pill).

The user picked A because low resource use is the top priority and one exe that is both the hook client and the UI keeps hooks fast and installation trivial.

## 5. Architecture

One executable, two modes:

- `agent-islands.exe hook` is run by every hook. It reads the hook's JSON from stdin, sends it to the running island, and exits. For `PermissionRequest` only, it waits for the user's answer and prints the decision JSON to stdout.
- `agent-islands.exe` (no arguments) is the island UI process. It owns the window and all state.

### Process lifecycle

1. A hook runs `agent-islands.exe hook`.
2. The hook tries to connect to the island over a named pipe (name: `\\.\pipe\agent-islands-<user SID or username>`).
3. If no island is listening, the hook starts `agent-islands.exe` detached (no console window), then retries the connection for up to about 2 seconds.
4. The island exits when no Claude Code session is open, after a short grace period (about 3 seconds, so closing one terminal and opening another does not cause a flicker of exit and restart).
5. Only one island may run. The island creates the pipe with "first instance" semantics; a second island that cannot create it exits immediately.

### Security requirements

- The pipe must be restricted to the current user.
- Before trusting a reply (especially an Allow), the hook must verify the pipe server is a process whose executable path equals its own `agent-islands.exe` path (e.g. via the server's process ID). If verification fails, the hook outputs nothing, so Claude Code falls back to its normal terminal prompt.
- An Allow decision is produced only by a mouse click on the Allow button for that exact request. No keyboard shortcut for Allow in the MVP. No "allow always".
- Any hook-side failure (island not reachable, timeout, bad data) results in no decision output, so Claude Code behaves exactly as if the plugin were not installed.

## 6. Hook events and what the island does

Every hook input includes `session_id`, `cwd`, and `transcript_path`. Sessions are keyed by `session_id`. The project name shown is the last folder name of `cwd`; the full `cwd` is shown underneath in the expanded view.

| Hook (matcher) | Island behavior |
|---|---|
| `SessionStart` | Session appears as an **idle** blob. Record cwd, project name and the Claude Code process ID. A session that was already open before the plugin was installed appears at its first event of any kind. |
| `UserPromptSubmit` | Session becomes **working**. Clears the previous todo list for that session. Also clears an unseen "finished" pill for this session. Store `session_title` if present (used for tab matching, Section 9). |
| `PostToolUse` on the todo tools | Replace the session's todo list. See "Todo source" below. |
| `PermissionRequest` | Session becomes **waiting**. Show the permission card (Section 7.4). The hook process blocks until the user clicks Allow or Deny, or until 30 seconds pass. On timeout it prints nothing, so the normal terminal prompt appears, and the card changes to "Waiting for you" with "Go to terminal". |
| `PreToolUse` (`AskUserQuestion`) | Session becomes **waiting** with "Has a question for you" and "Go to terminal". Never blocks or alters the tool call. |
| `PostToolUse` (any tool) for a waiting session | Session returns to **working** and its card closes. This covers permissions and questions answered in the terminal. The builder may narrow the matcher to keep hook calls cheap, but every tool that can trigger a permission prompt or question must be covered. |
| `Notification` | If it says the agent needs input and no permission or question card is already open for that session, the session becomes **waiting** with "Waiting for you" and "Go to terminal". Notifications that duplicate an open card, or idle reminders after a session already finished, are ignored. |
| `Stop` | Session becomes **idle** and an unseen "finished" entry is added (Section 7.6). Its blob stays. |
| `StopFailure` | Same as Stop, with the text "stopped with an error" instead of "finished". |
| `SessionEnd` | Session is removed entirely: its blob and any unseen finished entry disappear. |

Leaving the waiting state: any later event for the same session (`PreToolUse`, `PostToolUse`, `PostToolUseFailure`, `PermissionDenied`, `UserPromptSubmit`, `Stop`, `StopFailure`) closes that session's waiting card. This covers a permission the user denied in the terminal after the island timed out; a denied tool fires no `PostToolUse`, but Claude Code moves on and fires other events. To support this, register a cheap `PreToolUse` hook for all tools (it only forwards the event; it never outputs a decision). It is not needed for `AskUserQuestion` alone.

While a `PermissionRequest` hook is blocking, Claude Code does not show its own terminal prompt, so the user cannot answer in the terminal during those 30 seconds. If the island exits or the pipe breaks while a hook waits, the hook prints nothing and exits at once.

### PermissionRequest schema (verified against Claude Code docs for the 2.1.x line)

Input, on top of the common fields: `hook_event_name: "PermissionRequest"`, `tool_name`, `tool_input`, optional `permission_suggestions`, and optional `mcp_server` for MCP tools.

Output, printed to stdout as JSON:

```json
{ "hookSpecificOutput": { "hookEventName": "PermissionRequest",
    "decision": { "behavior": "allow" } } }
```

or `"decision": { "behavior": "deny", "message": "Denied from Agent Islands" }`. Printing nothing leaves the normal permission prompt in place. The builder must confirm this on the installed 2.1.325 with one real prompt before building the card. If the event behaves differently, stop and report back; do not substitute a `PreToolUse` decision, because `PreToolUse` fires for tools that would never have prompted.

Hook output: every hook except `PermissionRequest` prints nothing and exits 0 as fast as possible. Hooks must never block Claude Code for more than a few milliseconds when the island is already running.

Hook timeouts in `hooks.json`: `PermissionRequest` gets a timeout above the island's 30-second wait (e.g. 40 seconds), so the island's timeout always wins. All others get a short timeout (e.g. 5 seconds).

### Todo source

Claude Code 2.1.325 manages todos through task tools, and may also expose `TodoWrite`. Hook events `TaskCreated` and `TaskCompleted` also exist (they carry `task_id` and `task_subject`). The builder must check on the installed version which tool calls carry the full status (pending / in_progress / completed) and register `PostToolUse` hooks for exactly those, for example matcher `TodoWrite|TaskCreate|TaskUpdate`. The island keeps, per session, an ordered list of `{id, subject, status}` and shows only `completed` and `in_progress` items.

If a working session has no todos, the panel shows one line with a spinner: "Working".

### Stale sessions

A session that is closed or interrupted may never send `Stop` or `SessionEnd` (for example a killed terminal). The hook records the Claude Code process ID (by walking up the parent process chain from the hook process). The island checks every few seconds whether that process is still alive and removes the session if it is not.

Known gap: if the user presses Esc to interrupt and Claude Code does not fire `Stop` for interrupts, the blob stays "working" until the next event. The builder should check whether `Stop` fires on interrupt and note the result.

## 7. UI

The locked visual references are `docs/superpowers/mockups/island-left.html` (placement and layout: vertical pill on the left edge, panels to the right) and `docs/superpowers/mockups/island-blob.html` (blob, typography, colors, cards, motion). Where they differ, `island-left.html` wins.

### 7.1 Placement and window behavior

- Left edge of the primary monitor: 8 px from the left, about 96 px from the top, so it clears maximized title bars and the Windows Terminal tab strip. (Changed by the user on 2026-11-15 from top center, because Windows has window controls and other items at the top.)
- Always on top, borderless, transparent outside the drawn shape, no taskbar button, no Alt+Tab entry.
- Must not take keyboard focus when it appears or changes. It takes focus only when the user clicks it.
- Clicking outside the expanded panel collapses it.
- DPI aware (per-monitor), so it is crisp at 125% / 150% scaling.

### 7.2 Collapsed island

- A vertical, fully rounded pill (corner radius = half its width). Blobs stack top to bottom.
- Contains one blob per open session, in the order the sessions first appeared. Idle blobs sit still and are slightly dimmed (about 50% opacity, calm open eyes); working, waiting and finished blobs are full strength.
- Blob size about 20 px. Pill grows and shrinks downward smoothly as blobs come and go.
- Working blob: gentle squash-and-bounce loop (about 0.9 s).
- Waiting blob: eyes look up, small "o" mouth, and an amber "!" badge at the top right that pulses.
- Clicking the pill expands it.

### 7.3 Expanded panel

- Opens a panel to the right of the pill (8 px gap), top-aligned with the pill, with a short spring-like animation (about 250 ms). Panel about 280-340 px wide, 8 px corners. The pill stays where it is.
- The pill itself is the session picker: clicking a blob selects it, and the selected blob has a short 2 x 12 px white bar on its left. The panel shows only the selected session.
- A 1 px divider, then the selected session's project name (14 px, semibold), its full path (11 px, monospace, muted), then the todo list.
- Todo list: completed items show a green check circle and muted text; the current item shows a spinner and medium-weight text. Pending items are not shown.
- Default selection when opening: the session that most recently changed.

### 7.4 Permission card

- Opens automatically (no click needed) when a `PermissionRequest` arrives, without taking keyboard focus.
- Contents: waiting blob, project name, "Wants to run a command" (or the matching phrase for the tool, e.g. "Wants to edit a file"), a monospace box with the tool name in amber followed by a one-line detail, then **Deny** and **Allow** buttons.
- One-line detail per tool: `Bash` shows the command; `Edit`/`Write`/`MultiEdit`/`NotebookEdit` show the file path; `WebFetch` shows the URL; MCP tools show the server and tool name; anything else shows the tool name plus the first short string field of its input. Text that is too long is cut with an ellipsis; hovering shows the full text in a tooltip.
- Deny returns `deny` with a short message ("Denied from Agent Islands"). Allow returns `allow` with no changes to the input or permissions.
- If several permission requests are pending, show one at a time with "1 of 3", oldest first.
- After the 30-second timeout, the card switches to the "Waiting for you" form (7.5).

### 7.5 Waiting card (question or other input)

- Opens automatically, same as 7.4.
- Waiting blob, project name, "Has a question for you" (AskUserQuestion) or "Waiting for you" (anything else), and one **Go to terminal** button.
- Closes when the session returns to working.

### 7.6 Finished pill

- When a session stops, its blob in the pill shows the happy face and green check badge, and a small rounded label opens to its right with the project name in semibold and "finished" in muted text.
- If more than one session finished and has not been seen, the pill shows their blobs and "3 finished".
- "Seen" means: the user clicks the finished pill (which opens the expanded panel on that session), or the session starts working again. Not a timer.
- All other open sessions keep their blobs in the pill alongside the finished entry.

### 7.7 Design tokens (from the locked mockup)

- Fonts: Geist (UI text) and Geist Mono (paths, commands), embedded in the exe. Both are SIL OFL licensed. Weights 400, 500, 600.
- Background `#0c0c0e`, hairline border `rgba(255,255,255,.07)`, text `#f2f2f4`, secondary `#b4b4bc`, muted `#8a8a94`.
- Session colors, assigned in order to live sessions and reused when freed: tangerine `#ff6b35`, azure `#1e9bff`, orchid `#b85cff`, magenta `#ff3d8b`, teal `#14c8c0`, indigo `#5b6cff`. A seventh live session reuses the first color.
- Status colors, never used as session colors: green `#2bd45f` (allow, done), amber `#ffb800` (waiting), red `#ff4d4d` (deny text).
- Corner radii: collapsed pill fully rounded; panel and cards 8 px; buttons and command box 4 px.
- Blob: the shape, highlight, eyes, and badges in the mockup's SVG, drawn as vector shapes (no image files).

## 8. Performance targets

- Island process: under 30 MB working set in normal use.
- CPU: repaint only while something animates; cap animation at 30 fps when collapsed; zero repaints when nothing moves.
- Hook client: completes in under 20 ms when the island is already running (except `PermissionRequest`, which waits for the user).
- No background process once every Claude Code session has closed.

## 9. Go to terminal

Step 1 (required): bring the Windows Terminal window that hosts the session to the front. The hook records the process chain from the hook up to the Windows Terminal process; the island finds that process's top-level window and activates it. Because the user just clicked the island, Windows allows the island to set the foreground window.

Step 2 (best effort, behind a spike): select the right tab inside that window using Windows UI Automation, matching the tab title against what Claude Code sets as the terminal title (likely related to `session_title`). Before building it, run a throwaway spike to confirm: (a) how Windows Terminal's process tree links a tab to its shell, (b) what title Claude Code gives the tab, and (c) that UI Automation can select a tab by name. If the spike fails, ship step 1 only.

## 10. Packaging and install

- A Claude Code plugin folder containing `.claude-plugin/plugin.json`, `hooks/hooks.json`, and the built `agent-islands.exe`. Hook commands reference the exe through the plugin root variable, so the plugin works from any install path.
- The user installs the plugin once. Nothing else to configure.

## 11. Decided by the user

- Rust single exe (approach A).
- Windows Terminal only for the MVP.
- Allow/Deny buttons for permissions, always showing what is being allowed.
- "Go to terminal" for questions and other waits; if that turns out too hard, all waits fall back to "Waiting for you" plus "Go to terminal".
- The look: Geist fonts, the v4 palette, blob mascot, full pill when collapsed, 8 px panel corners, 4 px buttons.
- One blob per open Claude Code session, whatever its state; pending todos hidden; finished pill stays until seen.

## 12. Calls made by Claude (user may override)

- 30-second wait on permission requests before falling back to the terminal prompt.
- Each session's color is its identity; state is shown by motion and badges, not color changes.
- New prompt clears that session's previous todo list.
- Multiple pending permissions are queued oldest first.
- Island position: left edge, 8 px in, about 96 px down. Permission and waiting cards open to the right of the pill, top-aligned with the blob that is asking.
- Hover tooltip shows the full text of a truncated permission detail.
- Allow is mouse-only, no keyboard shortcut.
- Any later event for a session closes its waiting card (Section 6).
- The very first hook, the one that starts the island, may wait for the island to come up (normally well under half a second, at most about 2 seconds). Only that one call may exceed the 20 ms target.
- Placement is recomputed on the primary monitor each time the island appears. It is a normal always-on-top window with no special handling for full-screen apps.
- The transparent area around the shape is click-through. The expanded panel collapses when it loses focus; a permission or waiting card that opened on its own stays open until it is answered or the session moves on.
- The tab-select spike (Section 9) is time-boxed to half a day. If it does not reliably select the right tab by then, ship window-only focus.
- The built `agent-islands.exe` ships prebuilt inside the plugin folder. If a plugin update replaces it while an old island is running, pipe verification fails safely (no decision output) until the old island exits.
- The project name and path come from the session's first event and do not change if Claude later runs `cd`.
- Idle blobs are still and dimmed to about 50% opacity, so working and waiting sessions stand out when many terminals are open.

## 13. Left to the builder

- Choice of crates (egui backend, Windows API bindings, pipe library) within the RAM target.
- Exact pipe message format.
- Exact animation curves, as long as they match the mockup's feel.
- How `agent-islands.exe` is built and placed into the plugin folder.
- Test approach: at minimum, a way to feed recorded hook JSON into `agent-islands.exe hook` and check the island's state, plus one check that a failed pipe verification never outputs an Allow.
