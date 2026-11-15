# Agent Islands: names, look, size, side, restart safety

Short design. Approved in chat on 2026-11-15.

## Why

- Session names were wrong. A session started in `D:\Qtanea\bots` and worked in `os-bots-worker`, but the island labeled it `openscholar-bots-v2`. The name was pinned to the folder of the first event the island saw, and the island had restarted while Claude was in that folder.
- Full paths in the panel are too long.
- The near-black island (`#0c0c0e`) disappears against Windows Terminal.
- Blobs (22 px) are too small.
- The left edge is in the way for some layouts. The user wants to choose left or right. Free dragging is not wanted.
- Two bugs reported earlier:
  - Sessions running a different copy of the exe (the plugin cache vs the repo `plugin\bin`) fail the hook's exe-path check, so their blobs never appear.
  - Todos live only in memory, so an island restart loses them.

## Design

1. **Name.**
   - Recomputed from the session's `cwd` on every event, not pinned.
   - If the folder is inside a git repo (walk up to the first folder holding `.git`), use the repo's top folder; otherwise use `cwd` itself.
   - Display the last two folders joined with ` > `, for example `bots > os-bots-worker`.
   - The panel path line uses the same short form.
   - Known limit: Claude Code only reports a new `cwd` after Claude runs `cd`. Edits done by absolute path without a `cd` do not change the name.
2. **Background.**
   - BG becomes `#1c1d22`.
   - The outline (`LINE`) becomes 14% white instead of 7%.
3. **Size.**
   - Everything scales by 1.25 with one UI zoom factor: blobs go from 22 to 27.5 px, and text, panels, cards, window size and window region scale with them.
   - The proportions are unchanged.
4. **Side setting.**
   - Right-click the pill to open a small settings card with a Left/Right switch.
   - The choice is saved to `%APPDATA%\agent-islands\settings.json` as `{"side":"left"|"right"}`. A missing or unreadable file means left.
   - Right side: the pill is docked 8 px from the right edge of the primary monitor's work area, 96 px from the top, and panels, cards and labels open to the left (mirrored).
   - The choice applies immediately.
5. **Server check by content.**
   - The hook verifies the island by comparing a hash of the island exe's bytes with a hash of its own exe, instead of comparing paths.
   - Same build means a match from any install location. A different build is rejected, as before.
6. **Todo replay.**
   - When the island first sees a session, it reads that session's transcript (`transcript_path`) and replays the TaskCreate, TaskUpdate and TodoWrite calls to rebuild the todo list.
   - Live events continue from there.

7. **Random session colors.**
   - Today a new session takes the first unused color in palette order, so the first session is always orange.
   - New rule: pick at random among the colors no live session uses. If all six are in use, pick at random among all six.
   - Randomness comes from std (`RandomState` or clock nanos); no new dependency.

## Approaches considered

- **Settings entry:**
  - A gear in each detail panel was rejected: clutter, and it is only reachable through a session.
  - A tray icon was rejected: a second always-on icon and more code.
- **Size:**
  - Changing every pixel constant by hand was rejected; one zoom factor keeps the proportions with a one-line change.
- **Server check:**
  - A list of allowed install paths was rejected: it breaks whenever the install location changes.

## Non-goals

- Free dragging.
- Multi-monitor choice.
- Settings beyond the side.

## Left to the builder

- The exact settings card layout, following the existing card style (8 px corners, 4 px buttons, Geist).
- The transcript replay must skip lines it cannot parse, and must not block the UI thread.
