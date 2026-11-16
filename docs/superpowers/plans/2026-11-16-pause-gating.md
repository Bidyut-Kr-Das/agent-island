# Pause gating

**Goal:** A Settings toggle that pauses the command gate (spec 9a) so the user can go AFK without gated commands stalling for 30 s and then being denied.

**Decisions (agreed 2026-11-16):**
- Paused = **skip the gate**. The hook prints nothing for a gated command, exactly as if Agent Islands did not gate. Claude Code's own permission rules decide. Agent Islands never auto-allows.
- Duration = **until resumed**. One toggle. In memory only: every island start begins with gating on (safe default, nothing to forget on disk).
- The pill shows a marker while paused, so the user sees the gate is off.

## Design

The island owns the flag; the hook stays stateless. The hook already sends every gated command to the island and waits for a decision, so the island answers a new decision, `"skip"`, at once when paused.

Flow when paused:

1. Hook: `gate::is_gated` is true, sends envelope with `"gated": true` (unchanged).
2. `server::serve`: `gated && paused` → writes `{"decision":"skip"}` at once, then treats the event as ungated (`gated = false` before it is copied into `raw`). The event still reaches Core as normal PreToolUse activity; no permission card queues.
3. Hook `forward`: decision `"skip"` → returns `Some(String::new())`. `run_with` passes it through, `run` prints an empty stdout. Claude Code continues with its normal flow.

Why not let the hook read `settings.json`: two writers of one file, a disk read on every hook call, and a persisted pause that survives restarts. The island answer costs nothing: the gated round trip already exists.

If the island is not running, the hook still denies ("Agent Islands is not running"). Pause is an island state; no island means no pause.

## Changes

### `src/server.rs`
- `spawn(tx, wake, paused: Arc<AtomicBool>)`; clone the `Arc` into each connection thread; `serve(f, tx, wake, &paused)`.
- In `serve`, right after `let gated = v["gated"] == true;`:
  ```rust
  let gated = gated && !(paused.load(Ordering::Relaxed) && (&f).write_all(b"{\"decision\":\"skip\"}\n").is_ok());
  ```
  (or the plain `if` form; the write must happen before the event goes to Core). Rest of `serve` is unchanged: `wants` becomes false for a PreToolUse, so no card and no wait.

### `src/hook.rs`
- `forward`, gated branch: `if d.as_deref() == Some("skip") { return Some(String::new()); }` before `pretool_output`.
- Update the `read_reply` doc comment: decisions are `allow`, `deny`, `timeout`, `skip`.

### `src/ui/mod.rs`
- Create `let paused = Arc::new(AtomicBool::new(false));` before `server::spawn`, pass a clone, store the other in `App` (`paused: Arc<AtomicBool>`).
- Settings card handler: on a gating click, `self.paused.store(v, Ordering::Relaxed)`. No `settings::save_to`.
- Pass `self.paused.load(..)` to `views::pill` for the marker.

### `src/ui/views.rs`
- `settings_card(ui, pill, side, paused) -> (Rect, Option<Side>, Option<bool>)`.
- Add a second row under "Side": label "Gating", buttons **On** / **Paused**, same style and size as Left / Right. Card height `h` grows by `12.0 + 28.0`.
- Pill marker while paused: reuse an existing theme color (amber/warn if present in `theme.rs`) for a small dot or outline on the pill. Exact look picked at build time against the mockups.

### Docs
- Spec `2026-11-15-agent-islands-design.md` section 9a: one paragraph on pause.
- README: one line under the gate section.

## Tests (one per behavior, no new fixtures)
- `server.rs`: `paused_gated_replies_skip_and_does_not_wait`: send a gated envelope with `paused = true`; client reads `{"decision":"skip"}`; Core gets the event with `reply: None` and `raw["gated"] == false`.
- `server.rs`: existing `gated_envelope_waits_for_reply_and_marks_event` passes with `paused = false` (proves unpaused path is unchanged).
- `hook.rs`: `gated_skip_prints_nothing`: fake island replies `skip`; `run_with` returns `Some("")`, not a deny.
- Manual: pause in Settings, run `git status && echo rm` in a session (gated by design), command runs with no card; resume, same command shows a card.

## Skipped
- Timers / auto-resume: user chose "until resumed". Add when someone forgets the pause in practice.
- Persisting the pause across restarts: deliberately not done (safe default).
- Per-session or per-command pause: no ask for it.
