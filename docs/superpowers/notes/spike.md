# Hook spike results (Claude Code 2.1.325, Windows 11)

Method: headless `claude -p --model haiku --plugin-dir spike/plugin` run from PowerShell, logging every hook via `spike/log-hook.ps1` to `spike/events.jsonl`. Interactive-only checks are listed at the end. Fixtures: `tests/fixtures/`.

General facts
- Hook commands are executed through Git Bash (`bash.exe`), which then launches `powershell.exe`. A quoted Windows path in the `hooks.json` command works (`powershell ... -File "D:\projects\agent-islands\spike\log-hook.ps1"`). In JSON the backslashes must be escaped; an unescaped `\a` or `\p` makes the whole hooks file fail to load (`Failed to load hooks ... Invalid escape character`).
- Common fields: `session_id`, `cwd`, `hook_event_name`; most also `transcript_path`, `prompt_id`, `permission_mode`, `effort.level`. `SessionStart` has `source` ("startup"). `SessionEnd` has `reason` ("other" for `-p`). `Stop` has `last_assistant_message`, `stop_hook_active`, `background_tasks`, `session_crons`.
- Headless needs `CLAUDE_CODE_ENABLE_TASKS=1` for `TaskCreate`/`TaskUpdate` to be offered (without it `-p` offered neither; with `CLAUDE_CODE_ENABLE_TASKS=0` plus `CLAUDE_CODE_ENABLE_TODO_TOOLS=1` it offered `TodoWrite`). Interactive default is untested; register hooks for all three tools.
- Events seen: SessionStart, UserPromptSubmit, PreToolUse, PostToolUse, TaskCreated, TaskCompleted, PermissionRequest, Stop, SessionEnd. Not seen headless: Notification, PostToolUseFailure, PermissionDenied, StopFailure. `AskUserQuestion` is not available headless (pending manual check).
- `PreToolUse`/`PostToolUse` for `ToolSearch` also fire (a deferred-tool lookup precedes the first task/todo call). Matcher `*` hooks see it.

## Q1. Todo tools and field names
Two mechanisms exist; both seen.

`TaskCreate` (fixture `PostToolUse-TaskCreate.json`):
- `tool_input`: `{"subject":"Task A","description":"Task A"}` (no status; a new task is implicitly `pending`).
- The id appears ONLY in `tool_response.task.id` (string `"1"`,`"2"`,...) alongside `tool_response.task.subject`. `PreToolUse` has no id.
- Separate event `TaskCreated`: `{task_id:"1", task_subject:"Task A", task_description:"Task A"}`.

`TaskUpdate` (fixture `PostToolUse-TaskUpdate.json`):
- `tool_input`: `{"taskId":"1","status":"in_progress"}` (only changed fields; no subject).
- `tool_response`: `{"success":true,"taskId":"1","updatedFields":["status"],"statusChange":{"from":"pending","to":"in_progress"}}`.
- Statuses seen: `pending` (implicit), `in_progress`, `completed`.
- Separate event `TaskCompleted`: `{task_id, task_subject, task_description}`. In the log it appeared interleaved with the in_progress PostToolUse events, so do not rely on its ordering relative to PostToolUse.
- Consequence: the island must keep its own map id -> {subject, status}: subject from the `TaskCreate` response (or `TaskCreated`), status from `TaskUpdate.tool_input.status` (or `tool_response.statusChange.to`). Id key names differ: `task.id` / `taskId` / `task_id`.
- Parallel TaskUpdate calls: all `PreToolUse` events arrived batched before the `PostToolUse` events, and PostToolUse order did not follow call order (in_progress for 1,2,3 then completed 1,2,3). Apply by taskId.

`TodoWrite` (fixture `PostToolUse-TodoWrite.json`): full list replace. `tool_input.todos[]` = `{content, status, activeForm}` (no id; identity is list position). `tool_response.newTodos` / `oldTodos` hold the same shape. Statuses `pending|in_progress|completed`.

Matcher to register: `TodoWrite|TaskCreate|TaskUpdate`.

## Q2. Notification `notification_type`
No Notification fired in headless runs. Pending manual check.

## Q3. Does Stop fire on Esc interrupt?
Pending manual check.

## Q4. Ancestor chain
Evidence (hook powershell upward, from `events.jsonl`; headless run launched in a Windows Terminal PowerShell tab, with this agent's own claude above it):
`powershell.exe (hook) -> bash.exe -> bash.exe -> claude.exe (the session) -> powershell.exe -> cmd.exe -> claude.exe (outer) -> powershell.exe -> WindowsTerminal.exe -> explorer.exe`
- The Claude process is `claude.exe`, the FIRST `claude.exe` above the hook; the hook is two `bash.exe` levels below it.
- `OpenConsole.exe` is NOT in the ancestor chain. `WindowsTerminal.exe` is (pid 5868; same pid as the WT window in Q8).
- Algorithm: walk parents from the hook, take the first `claude.exe` as the session pid, continue to `WindowsTerminal.exe` for the window.

## Q5. session_title / tab title
`session_title` is NOT present on `UserPromptSubmit` in `-p` mode (keys: session_id, cwd, prompt_id, permission_mode, hook_event_name, prompt, transcript_path). Interactive result pending manual check. Partial evidence from Q8: WT tab names look like `✳ Public API SDG endpoints`, `◐ Bots flow and Scopus worker`, `◑ Claude Code dynamic island`: a status glyph followed by a short conversation title, not the session id or cwd. Matching must tolerate a leading glyph + space.

## Q6. PermissionRequest deny output
Headless evidence (`spike/plugin-deny`, `-p`, Bash `mkdir spike_probe_dir`): `PermissionRequest` fires for Bash that needs approval; input has `tool_name`, `tool_input.command`, `tool_input.description`, `permission_suggestions` (fixture `PermissionRequest.json`). With the hook printing the exact deny JSON, Claude reported Bash returned exactly `Denied from Agent Islands` and the directory was not created. Without the deny plugin the same call reports "needs approval" (no decision). So the output schema is accepted and the message reaches the model. Interactive confirmation that no terminal prompt appears: pending manual check.

## Q7. windows_subsystem exe as hook
Built `spike/winexe` (`#![windows_subsystem = "windows"]`, reads stdin, writes `{}` to stdout), registered for `UserPromptSubmit` with a quoted absolute path. Debug log: `Hooks: UserPromptSubmit ["...winexe.exe"] (plugin spike-winexe@inline) finished with status 0` and `Hook UserPromptSubmit (UserPromptSubmit) success:\n{}`. The exe also received the full JSON on stdin. No hook error. CONFIRMED.

## Q8. UI Automation on Windows Terminal
`System.Windows.Automation` (PowerShell 5.1): the WT top-level window has class `CASCADIA_HOSTING_WINDOW_CLASS`, Name = active tab title. Descendants with ControlType `TabItem` exist, one per tab, `Name` = tab title: `Windows PowerShell`, `✳ Public API SDG endpoints`, `◐ Bots flow and Scopus worker`, `◑ Claude Code dynamic island`. Each supports `SelectionItemPattern` (plus ScrollItem, VirtualizedItem), so `Select()` is available. `Select()` was deliberately NOT called; actually switching tabs is pending manual check.

## Fixtures
`tests/fixtures/`: SessionStart, UserPromptSubmit (prompt scrubbed), PreToolUse (Bash), PostToolUse (Bash), PostToolUse-TaskCreate, PostToolUse-TaskUpdate, PostToolUse-TodoWrite, TaskCreated, TaskCompleted, PermissionRequest, Stop, SessionEnd. `transcript_path` is scrubbed. Not captured: Notification, PreToolUse-AskUserQuestion, StopFailure, PostToolUseFailure, PermissionDenied.

## Manual checks for the user
Run from `D:\projects\agent-islands\spike\sandbox` in a Windows Terminal tab. The log plugin writes `spike\events.jsonl` (delete it first for a clean run).

1. Q3 Esc / Q2 / AskUserQuestion: `claude --plugin-dir ..\plugin`. Prompt: "Ask me a question with AskUserQuestion (options X, Y), then run `mkdir probe1`." Answer the question; when the Bash permission prompt shows, wait about 60 s idle (watch for a Notification), then Allow. Then prompt "Write a 500 word story" and press Esc mid-stream. Then `/exit`.
   Report from events.jsonl: all `notification_type` values and messages; the `PreToolUse` payload for `AskUserQuestion` (copy to `tests/fixtures/PreToolUse-AskUserQuestion.json`); whether a `Stop` line appears after the Esc.
2. Q6 interactive: `claude --plugin-dir ..\plugin-deny`. Prompt: "Run `mkdir probe2`". Expect NO permission prompt in the terminal, Claude saying it was denied with "Denied from Agent Islands", and `probe2` not existing. Check `spike\deny-events.jsonl` has the input.
3. Q5: `claude --plugin-dir ..\plugin`, send any prompt, then look at the WT tab title and the `UserPromptSubmit` line in `spike\events.jsonl`: is there a `session_title` key, and does the tab title contain it?
4. Q8 select: with 2+ WT tabs open, run in PowerShell:
   `Add-Type -AssemblyName UIAutomationClient; $r=[Windows.Automation.AutomationElement]::RootElement; $t=$r.FindAll('Descendants',(New-Object Windows.Automation.PropertyCondition([Windows.Automation.AutomationElement]::ControlTypeProperty,[Windows.Automation.ControlType]::TabItem))); $t[0].GetCurrentPattern([Windows.Automation.SelectionItemPattern]::Pattern).Select()`
   Confirm the first tab becomes active.

## Manual check results (user, 2026-11-15, interactive session)

- Q2: the only `notification_type` seen is `permission_prompt` (message "Claude needs your permission"), fired right after each `PermissionRequest`. No idle notification appeared within the wait.
- AskUserQuestion fires `PreToolUse` AND `PermissionRequest` (tool_name `AskUserQuestion`) AND `Notification` `permission_prompt`. The island must treat a `PermissionRequest` for `AskUserQuestion` as a question: show "Has a question for you", and the hook must print nothing and exit immediately so the terminal shows the question.
- The shell tool on this machine is `PowerShell` (tool_name `PowerShell`, tool_input `{command, description}`), not `Bash`. The detail line must treat `PowerShell` like `Bash`. Check 2 with the `Bash`-matcher deny plugin therefore never fired; deny output itself was already confirmed headless.
- Q3: after Esc mid-stream, a `Stop` event fired. Interrupts do produce `Stop`.
- Q5: `UserPromptSubmit` has no `session_title`. The Windows Terminal tab title is a status glyph plus the session's AI title, e.g. `◐ AskUserQuestion probe with mkdir`. The AI title is in the transcript (`transcript_path`) as lines `{"type":"ai-title","aiTitle":"..."}`; the last such line is current. Go to terminal should read it from the transcript and match tab names that end with it.
- Q8: `SelectionItemPattern.Select()` on a Windows Terminal TabItem switches tabs (confirmed by the user).
- New fixtures: `PreToolUse-AskUserQuestion.json`, `PermissionRequest-AskUserQuestion.json`, `PermissionRequest-PowerShell.json`, `Notification-permission_prompt.json`.

## Task 9 result
Verified live (agent, 2026-11-15, without switching the user's tabs or windows):
- `ai_title` on this session's real transcript returned `Claude Code dynamic island`.
- `go_to_terminal(5868, Some("Claude Code dynamic island"))` (5868 = this session's `WindowsTerminal.exe`) returned true in about 45 ms; the matched tab was the already-selected one, and afterwards the same tab was still the only selected one.
- `go_to_terminal` with a bogus pid returns false, with and without a hint (unit test).

Left for the user (Task 10 checklist): two WT windows, several tabs, an `AskUserQuestion` waiting in a background tab of the non-front window; click "Go to terminal" and confirm the right window comes to the front (including from minimized) and the right tab is selected. Also confirm `SetForegroundWindow` is not refused when clicking the island (the `AttachThreadInput` fallback is untested live).
