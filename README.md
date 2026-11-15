# Agent Islands

A dynamic-island style overlay for Claude Code sessions on Windows. Claude Code hooks call `agent-islands.exe hook`, which forwards events to the island.

## First install

From the repo root (needs Rust):

1. `powershell -File .\build-plugin.ps1` (builds the exe into `plugin\bin\`).
2. In Claude Code: `/plugin marketplace add D:\projects\agent-islands` (use this repo's path).
3. In Claude Code: `/plugin install agent-islands@agent-islands-local`.
4. Restart running Claude Code sessions. The plugin applies to every session; the island starts itself on the first hook event.

## Update after code changes

1. `powershell -File .\build-plugin.ps1`. It stops any running island from `plugin\bin` or the Claude Code plugin cache, rebuilds, copies the exe, and bumps the patch version in `plugin\.claude-plugin\plugin.json`.
2. In a shell: `claude plugin update agent-islands@agent-islands-local`. (Claude Code detects updates by version and caches each version; without the bump nothing would update.) If it reports nothing new, first run `/plugin marketplace update agent-islands-local`.
3. Restart Claude Code sessions, or run `/reload-plugins` in a running one. The island restarts on the next hook event.

Remove: `/plugin uninstall agent-islands@agent-islands-local`.

Hands-on test list: `docs/superpowers/notes/user-checklist.md`.
