# Island Takeover Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** After a plugin update, the first hook from the newer version replaces the running older island, so nobody has to kill it by hand.

**Architecture:** The hook compares the island's exe path with its own. Installs live at `<cache>\<version>\bin\agent-islands.exe`, so the version folder tells which is newer. A newer hook terminates an older island and starts its own. An older hook keeps talking to a newer island. Shipped islands (0.1.9, 0.2.0) need no cooperation, because the hook terminates them from outside.

**Tech Stack:** Rust 2024, `windows` 0.62 (`Win32_System_Threading` is already enabled), named pipes.

**Spec:** None. This plan is the design. The problem it fixes:

- `src/hook.rs:170` drops every event when the island's exe differs from the hook's exe, and nothing ever replaces that island.
- `run_with` turns that drop into a deny for gated commands, with the reason "Agent Islands is not running". After an update, `git commit`, `git push`, `rm` and the rest of the gate list are denied until the island is killed by hand.

## Global Constraints

- The version comes from the install path only. `Cargo.toml` stays at 0.1.0 and is not the plugin version.
- Versions are exactly three dot-separated `u32` parts, compared numerically. Anything else is not a version.
- Only a strictly newer hook replaces an island. Equal versions with different bytes behave as today (dropped).
- Only a process whose image is `agent-islands.exe` in a sibling version folder may be terminated.
- From this release on, pipe messages may only change additively, because older hooks now talk to newer islands.
- Hooks have a 5 s timeout, so the whole takeover must finish inside 4 s.
- No new dependencies.

## Accepted Losses

These happen once per update, at the moment of the swap:

- Cards pending on the old island are lost. A permission request falls back to the terminal dialog, a held question opens its dialog, and a gated command is denied with "Agent Islands is not running", so Claude can retry it.
- Idle sessions reappear on the new island at their next hook event or when their `listen` helper reconnects.
- Sessions still running 0.2.0 or older hooks cannot talk to the new island until `/reload-plugins` or a restart. Their gated commands stay denied until then. Sessions on the first release with this fix and later are not affected.

## Review Focus

1. Version `0.1.10` against `0.1.9`: must count as newer. A string compare gets this wrong. Test in Task 1.
2. The old version folder was already deleted by plugin cleanup: the island is still classified `Older`, because classification of siblings reads only the path. Test in Task 1.
3. The same install reported with different letter case: still siblings. Test in Task 1.
4. Another program owns the pipe: never terminated, events dropped as today. Test in Task 1.
5. An older hook arrives after the newer island is up: it must not replace it, and its events must reach it. Test in Task 2.

---

### Task 1: Classify the running island

**Files:**
- Modify: `src/hook.rs` (next to `same_exe`, tests in the existing `tests` module)

**Interfaces:**
- Produces: `pub(crate) enum Island { Usable, Older, Foreign }` (derive `Debug, PartialEq`) and `pub(crate) fn classify(server: &Path, me: &Path) -> Island`

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn classify_compares_sibling_versions() {
    let p = |v: &str| std::path::PathBuf::from(format!(r"C:\c\agent-islands\{v}\bin\agent-islands.exe"));
    assert_eq!(classify(&p("0.1.9"), &p("0.1.10")), Island::Older);
    assert_eq!(classify(&p("0.1.10"), &p("0.1.9")), Island::Usable);
    assert_eq!(classify(&p("0.2.0"), &p("0.2.0")), Island::Usable, "same path");
    assert_eq!(classify(&p("0.2.1-beta"), &p("0.2.2")), Island::Foreign);
    assert_eq!(classify(&p("0.2"), &p("0.2.2")), Island::Foreign);
    let upper = std::path::PathBuf::from(r"C:\C\AGENT-ISLANDS\0.1.9\BIN\AGENT-ISLANDS.EXE");
    assert_eq!(classify(&upper, &p("0.2.0")), Island::Older);
}

#[test]
fn classify_rejects_anything_outside_the_install_folder() {
    let me = Path::new(r"C:\c\agent-islands\0.2.1\bin\agent-islands.exe");
    for other in [
        r"C:\Windows\notepad.exe",
        r"C:\c\agent-islands\0.1.0\bin\other.exe",
        r"C:\c\agent-islands\0.1.0\agent-islands.exe",
        r"C:\elsewhere\agent-islands\0.1.0\bin\agent-islands.exe",
    ] {
        assert_eq!(classify(Path::new(other), me), Island::Foreign, "{other}");
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --bin agent-islands classify`
Expected: compile error, `classify` not found.

- [ ] **Step 3: Implement `classify`**

`same_exe(server, me)` true gives `Usable`. Otherwise read each path as `<root>\<version>\bin\agent-islands.exe`, without touching the disk and ignoring case. If either path does not fit, or the roots differ, or a version does not parse, return `Foreign`. Then server newer gives `Usable`, server older gives `Older`, equal gives `Foreign`.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test --bin agent-islands classify`
Expected: 2 passed.

- [ ] **Step 5: Commit**

```bash
git add src/hook.rs
git commit -m "feat: classify the running island by install version"
```

### Task 2: Replace an older island

**Files:**
- Modify: `src/procs.rs` (after `exe_path`)
- Modify: `src/hook.rs:164-172` (`forward_with_token`), plus the new connect function
- Modify: `src/listen.rs:1,10-17`
- Modify: `CHANGELOG.md` (`[Unreleased]`)
- Test: `tests/island.rs`

**Interfaces:**
- Consumes: `classify`, `Island` from Task 1
- Produces: `pub fn terminate_exact(pid: u32, image: &Path) -> bool` in `procs.rs`; `pub(crate) fn connect_island(name: &str, spawn: bool, replace: bool) -> Option<(std::fs::File, bool)>` in `hook.rs`, where the bool means this call started the island

- [ ] **Step 1: Write the failing integration test**

Add `takeover_goes_only_forward` to `tests/island.rs`. Generalize the existing `hook` helper into `hook_with(exe, pipe, fixture)`, keeping `hook` as the call with `EXE`.

1. Under a fresh temp folder, copy `EXE` to `cache\0.0.1\bin\agent-islands.exe` and `cache\0.0.2\bin\agent-islands.exe`. Append one byte to the 0.0.2 copy so `same_exe` sees different files.
2. Run the session-start fixture through 0.0.1 on a unique pipe. Assert the running island's path is the 0.0.1 copy.
3. Run it through 0.0.2. Assert within 5 s that the island's path is the 0.0.2 copy and that no 0.0.1 island is left.
4. Run it through 0.0.1 again. Assert the island is still the same 0.0.2 process id.
5. Run the session-end fixture through 0.0.2 and wait for the pipe to go away, as the existing test does.

Read the island's path and pid with `powershell -NoProfile -Command "Get-Process agent-islands | ForEach-Object { \"$($_.Id) $($_.Path)\" }"`, and keep only lines under the temp folder so a real island is never touched.

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test --test island takeover_goes_only_forward`
Expected: FAIL at step 3, the island is still the 0.0.1 copy.

- [ ] **Step 3: Implement `procs::terminate_exact`**

Open one handle with `PROCESS_TERMINATE | SYNCHRONIZE | PROCESS_QUERY_LIMITED_INFORMATION`. Read the image path from that handle and compare it to `image` ignoring case. Only on a match call `TerminateProcess`, then `WaitForSingleObject` for up to 500 ms. Return true when the process has exited. Share the path query with `exe_path` rather than copying it.

- [ ] **Step 4: Implement `hook::connect_island`**

Loop: `connect_retry(name, spawn)`, read `procs::exe_path(pid)` and `current_exe`, then `classify`.

- `Usable`: return the file and `spawned`, where `spawned` is also true if this call replaced an island, so the new island gets the process chain.
- `Older` with `replace` true, fewer than 2 replacements so far, and under 1500 ms since the call began: drop the file, call `terminate_exact`, go round again. The second round covers an old `listen` helper restarting the old island in the gap.
- Anything else, including a failed path lookup: return `None`.

Worst case is 1500 ms + 500 ms + the 2 s in `connect_retry`, which is 4 s.

- [ ] **Step 5: Use it from both callers**

`forward_with_token` calls `connect_island(name, spawn, spawn)?` and drops its own check. Its `decision_output`, `pretool_output` and `ask_output` calls pass `true`. `serve_lines` calls `connect_island(name, spawn, false)` and drops its own check, because the session's hooks do the replacing. Remove the imports this leaves unused.

- [ ] **Step 6: Run all tests**

Run: `cargo test`
Expected: all pass, including `takeover_goes_only_forward`.

- [ ] **Step 7: Add the changelog line and commit**

Under `[Unreleased]`, in `### Fixed`: "A plugin update now replaces the running island on the next hook. Restarting it by hand is no longer needed."

```bash
git add src/procs.rs src/hook.rs src/listen.rs tests/island.rs CHANGELOG.md
git commit -m "fix: newer hook replaces an older running island"
```

### Task 3: Verify on a real update

**Files:** none, unless a defect turns up.

- [ ] **Step 1: Sync with the remote**

Run: `git pull --ff-only origin main`
Expected: the local `.claude-plugin/marketplace.json` now points at v0.2.0, as the published one does.

- [ ] **Step 2: Release as usual, then update Claude Code**

Install the new version and run `/reload-plugins` in one session, leaving the old island running.

- [ ] **Step 3: Confirm the takeover**

Send a prompt, then run: `Get-Process agent-islands | Select-Object Id, Path, StartTime`
Expected: one process, its path in the new version folder, started after the prompt. The session shows on the island, and a gated command such as `git commit` shows a card rather than being denied.
