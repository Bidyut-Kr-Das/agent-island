# Changelog

Each `## [x.y.z]` section is the release note for tag `vx.y.z`. The release workflow publishes the section that matches the pushed tag, so write the section before you tag.

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
