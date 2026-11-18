<div align="center">

# Agent Islands

A dynamic-island style overlay for Claude Code sessions on Windows.<br>
See every session at a glance, follow its todos, and answer permission prompts without hunting for the right terminal.

<p>
  <img alt="Version" src="https://img.shields.io/badge/version-0.1.8-bf4520?style=flat-square">
  <img alt="Platform" src="https://img.shields.io/badge/platform-Windows%2010%20%7C%2011-0078D4?style=flat-square&logo=windows&logoColor=white">
  <img alt="Claude Code plugin" src="https://img.shields.io/badge/Claude%20Code-plugin-D97757?style=flat-square&logo=anthropic&logoColor=white">
  <img alt="Tests" src="https://img.shields.io/badge/tests-171%20passing-2ea44f?style=flat-square">
</p>

<p>
  <img alt="Rust 2024" src="https://img.shields.io/badge/Rust-2024%20edition-000000?style=flat-square&logo=rust&logoColor=white">
  <img alt="egui" src="https://img.shields.io/badge/egui%20%2F%20eframe-0.37-4f5d75?style=flat-square">
  <img alt="windows-rs" src="https://img.shields.io/badge/windows--rs-0.62-0078D4?style=flat-square">
  <img alt="serde" src="https://img.shields.io/badge/serde__json-1.0-4f5d75?style=flat-square">
  <img alt="OpenGL" src="https://img.shields.io/badge/OpenGL-glow-5586A4?style=flat-square&logo=opengl&logoColor=white">
  <img alt="PowerShell" src="https://img.shields.io/badge/PowerShell-build-5391FE?style=flat-square&logo=powershell&logoColor=white">
</p>

<a href="#tech-stack"><img src="https://skillicons.dev/icons?i=rust,windows,powershell" alt="Rust, Windows, PowerShell"></a>

</div>

---

## Features

- **One blob per session.** Working sessions bounce, waiting ones stand out, idle ones dim. Each session keeps its own color.
- **Live todos.** The panel shows each session's todo list as Claude updates it. After an island restart, the list is rebuilt from the session transcript.
- **Answer from the island.** Permission prompts open a card with Allow and Deny. With no answer within 30 seconds, the normal terminal prompt takes over.
- **Go to terminal.** Questions and waiting sessions get a button that brings the right Windows Terminal window and tab to the front.
- **Command gate.** Risky shell commands always ask on the island, even in auto mode: `rm`, `del`, `Remove-Item`, `git commit`, `git push`, `git reset`, and package installs (`npm`, `pnpm`, `yarn`, `bun`, `pip`, `uv`, `cargo`).
- **Finished pill.** Shows sessions that stopped while you looked elsewhere. Click it to clear.
- **Left or right.** Right-click the pill to dock the island on either side of the screen.
- **Phone link (optional).** Get each session's events on Telegram and reply from your phone. Off until you set a bot token. See [Phone link](#phone-link).
- **Stays out of the way.** It never steals typing focus, clicks pass through its transparent areas, and it exits about 3 seconds after the last session ends.

## How it works

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="docs/diagrams/dataflow-dark.svg">
  <img alt="Data flow: a Claude Code hook event goes through the hook client and a named pipe into the island's session state, which drives the overlay; the user's Allow or Deny returns through the hook client to Claude Code." src="docs/diagrams/dataflow.svg">
</picture>

1. **Fire.** Claude Code runs `agent-islands.exe hook` for each event registered in `plugin/hooks/hooks.json`.
2. **Forward.** The hook client reads the event JSON from stdin and sends it over a named pipe. If no island is running, it starts one.
3. **Track.** The island updates its session state: status, todos and open cards. The first time it sees a session, it reads the session transcript to rebuild the todo list.
4. **Show.** The overlay draws the blobs, the panel and the cards with egui on a click-through Win32 window.
5. **Answer.** Your click goes back through the pipe. The hook client prints the decision JSON for Claude Code. It does so only when the island is verified as the same build (the exe content is hashed). Otherwise it never allows anything.

## Install

Windows 10 or 11. In Claude Code:

```text
/plugin marketplace add Bidyut-Kr-Das/agent-island
/plugin install agent-islands@agent-islands
```

The plugin is the zip from the latest GitHub release, pinned by URL and sha256. Restart running Claude Code sessions. The plugin applies to every session, and the island starts itself on the first hook event.

To update: `/plugin marketplace update agent-islands`, then `/plugin update agent-islands@agent-islands` and `/reload-plugins`. To remove: `/plugin uninstall agent-islands@agent-islands`.

## Phone link

The phone link is optional and off by default. Nothing talks to Telegram until you set a bot token.

Setup:

1. In Telegram, create a bot with [@BotFather](https://t.me/BotFather) and copy its token.
2. In Claude Code, run `/plugin`, pick agent-islands, choose configure, and paste the token into **Telegram bot token**.
3. Open a session and click its blob on the island. The panel shows `Phone: send <code> to @<bot>`. Send that 6-digit code to your bot. It answers `Paired`. The code changes after 5 wrong tries or 10 minutes.

What arrives on the phone is one new message per event. The first line is a color circle and the session name, followed by one of:

- **Started.**
- `You (PC): <prompt>` when you send a prompt at the PC.
- `Permission: ...` with Allow and Deny buttons.
- The final answer and todo list, when Claude stops. Markdown in the answer shows as Telegram formatting. Tables show as code blocks, and links to local files show as their label.
- `Error: ...`
- `Waiting for you: ...`
- **Ended.**

Tool calls are not sent.

Replies:

- Reply to a session's message to send your text to that session as a prompt.
- A message that is not a reply goes to the only open session. If more than one is open, the bot asks `Which session?`.
- When the session starts on your message, the bot says `Working on it`, even with Away off.
- If the session is busy, your reply runs after the current turn, and the bot says `Queued: <name> is busy`.
- Messages sent while no session is open are not delivered. The bot says so once.
- Permission cards still time out after 30 seconds. When you are away, turn on Away (see Away mode below).
- For voice, use your phone keyboard's dictation.

Messages pass through Telegram's servers.

Disconnect: right-click the island, choose the **Phone** row, then **Disconnect**. Removing the token in `/plugin` turns the feature off.

## Away mode

Away mode sends everything to Telegram while you are away from the PC. Right-click the island, open **Settings**, and turn on **Away**. It needs a paired phone.

- **Away off.** Telegram stays silent and everything is instant at the PC.
- **Away on.** Every event goes to Telegram, and the pill outline turns amber. Permission cards and Claude's questions wait up to 10 minutes for you, then the normal terminal prompt takes over.
- **Questions from the phone.** Claude's questions arrive with one button per option. Multi-choice questions also get **Done**. Reply with text to give your own answer.
- **Turning Away off** releases everything that is waiting.

## Release

1. Bump `version` in `plugin/.claude-plugin/plugin.json`.
2. Add a `## [X.Y.Z]` section to `CHANGELOG.md`. It becomes the release notes.
3. Commit, then push a tag: `git tag vX.Y.Z && git push origin main vX.Y.Z`.

`.github/workflows/release.yml` checks that the tag matches `plugin.json`, runs the tests, builds the exe, publishes `agent-islands.zip` and the exe as a GitHub release, and commits the new URL and sha256 into `.claude-plugin/marketplace.json` on `main`.

## Local build

Needs Rust (stable). `powershell -File .\build-plugin.ps1` stops any running island, rebuilds, copies the exe into `plugin\bin\` and bumps the patch version. Try that build without the release with `claude --plugin-dir .\plugin`.

## Tech stack

| Layer | Choice |
|---|---|
| Language | Rust, 2024 edition |
| UI | `eframe` / `egui` 0.37 on the `glow` OpenGL backend, with `opengl32` delay-loaded |
| Windows | `windows` 0.62: named pipes, window regions, focus, process tree, DPI |
| Data | `serde` + `serde_json` for hook events and transcripts |
| Fonts | Geist and Geist Mono (OFL), embedded |
| Packaging | Claude Code plugin, released as a zip by GitHub Actions |

## Project layout

```text
src/
  main.rs       entry: island, or `hook` client mode
  hook.rs       hook client: forward event, print the decision
  pipe.rs       named pipe transport
  server.rs     island core: pipe server, timeouts, exit when idle
  state.rs      session state machine, todos, names, colors
  event.rs      hook event parsing, transcript replay
  detail.rs     one-line summary of a permission request
  gate.rs       command gate
  focus.rs      go to terminal
  procs.rs      process tree helpers
  settings.rs   left or right side
  ui/           overlay window, blobs, panel and cards
plugin/         Claude Code plugin (hooks.json, bin/)
tests/          integration test and recorded hook fixtures
docs/           design specs, plans, diagrams
```

## Development

```powershell
cargo test
cargo clippy --all-targets -- -D warnings
```

The manual test list is in [`docs/superpowers/notes/user-checklist.md`](docs/superpowers/notes/user-checklist.md).
