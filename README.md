# Agent Islands

A dynamic-island style overlay for Claude Code sessions on Windows. Claude Code hooks call `agent-islands.exe hook`, which forwards events to the island.

## Install (one time)

1. Build the plugin (needs Rust):

   ```powershell
   powershell -File D:\projects\agent-islands\build-plugin.ps1
   ```

   This runs `cargo build --release` and copies the exe to `plugin\bin\`. Re-run it after code changes.

2. In Claude Code, add this repo as a local marketplace and install the plugin:

   ```
   /plugin marketplace add D:\projects\agent-islands
   /plugin install agent-islands@agent-islands-local
   ```

   Restart running Claude Code sessions (or run `/reload-plugins`) so they pick up the hooks. The plugin then applies to every session; the island starts itself on the first hook event.

To remove: `/plugin uninstall agent-islands@agent-islands-local`.

Hands-on test list: `docs/superpowers/notes/user-checklist.md`.
