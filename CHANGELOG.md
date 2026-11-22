# Changelog

Each `## [x.y.z]` section is the release note for tag `vx.y.z`. The release workflow publishes the section that matches the pushed tag, so write the section before you tag.

## [Unreleased]

## [0.2.7] - 2026-11-22

### Fixed
- Telegram: the bot's `/` menu now lists `/open`, `/claude`, `/away` and `/help`, replacing commands another app left on the bot.

## [0.2.6] - 2026-11-21

### Added
- Telegram: phone commands `/open`, `/claude`, `/away` and `/help`. The bot listens while paired.
- Telegram: while Away is on, agents answer in short Simplified Technical English.
- Telegram: long answers show their first lines; the rest is in a tap-to-expand quote.
- Telegram: the todo list shows in a code font below the answer.
- Telegram: a typed message to an idle session gets `Sent to [repo]`.

### Changed
- Telegram: message headers read `[repo]` next to the colored circle, plus `· permission`, `· question` or `· error` when the agent needs you.
- Telegram: prompts typed on the PC are no longer echoed to the phone.

### Fixed
- Telegram: buttons go away after one tap, and the message shows the outcome.

## [0.2.5] - 2026-11-18

### Added
- Telegram: the bot says `Working on it` when a session starts on your message.
- Telegram: Claude's final answer shows Markdown as Telegram formatting. Tables show as code blocks, and links to local files show as their label.

### Fixed
- The pairing panel shows why the Telegram link is failing.

## [0.2.2] - 2026-11-18

### Fixed
- A plugin update now replaces the running island on the next hook. Restarting it by hand is no longer needed.

## [0.2.0] - 2026-11-18

### Added
- Telegram phone link. Optional and off until a bot token is set.
- Away switch in Settings (right-click the island). With a paired phone, Away on sends everything to Telegram and waits up to 10 minutes before the terminal takes over. Away off keeps Telegram silent and everything instant at the PC.
- Answer Claude's questions from the phone with buttons, **Done** for multi-choice, or a text reply for your own answer.
- Redesigned settings card.

### Removed
- Gating: Paused. Away replaces it.

### Fixed
- The jagged pill outline.
- The island no longer drops a hook event when the hook closed its connection before the island accepted it.

## [0.1.9] - 2026-11-16

### Added
- Pause gating from Settings (right-click the pill, **Gating: Paused**). While paused, gated commands follow Claude Code's own permission rules, and the pill outline turns amber. Every island start gates again.

## [0.1.8] - 2026-11-16

### Fixed
- Folder names keep their text; only the first letter is capitalized.

## [0.1.7] - 2026-11-16

### Fixed
- Fixes from the final review.

## [0.1.6] - 2026-11-16

### Added
- Left or right side setting.
- Session todos are rebuilt from the transcript the first time a session is seen.
- Each new session gets a random color among the unused ones.

### Changed
- Lighter island tone, smaller island, 24 px blobs, color instead of bold text.
- Title with icon, capitalized names, neutral colors.

### Fixed
- The hook verifies the island by exe content, not path.
- The panel grows from the pill on the right side.
- The session name follows the cwd, repo root and last two folders.
- A new todo set replaces a finished list.
- Chevron size and title row geometry.

## [0.1.5] - 2026-11-15

### Added
- Idle panels keep their last todos; terminal button.
