# User hands-on checklist

Prerequisite: run `build-plugin.ps1`, install per README, restart Claude Code sessions.
Format: what to do -> what should happen.

## Task 10 (plugin and full flow)
- [ ] Open 5 terminals, each running Claude Code (send a prompt in each) -> 5 blobs appear.
- [ ] Give one session a long task with todos -> its blob bounces; expanded view shows the todo list updating.
- [ ] Trigger a permission prompt (e.g. a Bash command not allowed) and click Allow -> tool runs; card closes; session returns to working.
- [ ] Same, click Deny -> tool is denied with "Denied from Agent Islands"; card closes.
- [ ] Same, do nothing for 30 s (the island timeout) -> card changes to "Waiting for you" with "Go to terminal"; the normal terminal prompt appears.
- [ ] Make Claude ask a question (AskUserQuestion) -> card opens "Has a question for you"; "Go to terminal" focuses the right terminal.
- [ ] Let a session finish -> "finished" pill appears; clicking it clears it.
- [ ] Close all Claude Code sessions -> island exits within about 3 s (check Task Manager for `agent-islands.exe`).
- [ ] While typing in a terminal, let the island appear/update -> island never steals typing focus.

## Deferred live checks (spike.md, Task 9 result)
- [ ] Two Windows Terminal windows, several tabs each; put an AskUserQuestion waiting in a background tab of the non-front window; click "Go to terminal" -> the right window comes to the front (also when minimized) and the right tab is selected.
- [ ] Click anything on the island while another app is focused -> the target window is brought to the foreground (SetForegroundWindow not refused; the AttachThreadInput fallback has not been tested live).

## Interaction checks
- [ ] Accidental-click guard: make a card appear while the mouse is elsewhere, then move onto the card -> buttons are dimmed until the mouse moves onto the card, then become active.
- [ ] Press the mouse button before the card appears and release over it -> that click in progress does not count as a button press.
- [ ] Click a blob -> its panel/card expands.
- [ ] Click inside the panel (non-button area) -> panel stays open.
- [ ] Click outside the panel -> panel closes.
- [ ] With more than one pending permission card (any sessions) -> the card shows the oldest one with "1 of N" (N = pending cards); there is no stepping: answer it and the next oldest takes its place.

## Command gate
Run each item in a scratch git repo, once in auto mode and once in manual (default) mode.
- [ ] Auto mode: ask Claude to run `git status` -> no card; the command runs.
- [ ] Auto mode: ask Claude to run `git commit --allow-empty -m test` -> card "Wants to run a gated command" with the command shown; Deny blocks it (Claude sees "Denied from Agent Islands").
- [ ] Auto mode: same command again -> Allow runs it (`git log` shows the commit).
- [ ] Manual mode: `git status` -> no gate card (Claude Code's own prompt, if any, as before).
- [ ] Manual mode: `git commit --allow-empty -m test` -> gate card appears; Deny blocks it; Allow runs it.
- [ ] Gate card ignores clicks in its first 600 ms and until the pointer moves onto it, like the permission card.
- [ ] Leave the gate card unanswered for 30 s -> command is denied ("No answer in Agent Islands within 30 seconds").
