use std::io::{BufRead, BufReader};
use std::path::Path;
use windows::Win32::Foundation::{HWND, LPARAM};
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoUninitialize};
use windows::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
use windows::Win32::UI::Accessibility::{
    CUIAutomation, IUIAutomation, IUIAutomationElement, IUIAutomationSelectionItemPattern, TreeScope_Descendants,
    UIA_SelectionItemPatternId, UIA_TabItemControlTypeId,
};
use windows::Win32::UI::WindowsAndMessaging::{
    BringWindowToTop, EnumWindows, GetClassNameW, GetForegroundWindow, GetWindowThreadProcessId, IsIconic, IsWindowVisible,
    SW_RESTORE, SetForegroundWindow, ShowWindow,
};
use windows::core::BOOL;

/// Brings the session's Windows Terminal window to the front and, when a tab is named after
/// `title_hint`, selects that tab. Blocking (UI Automation can be slow): call it off the UI thread.
pub fn go_to_terminal(terminal_pid: u32, title_hint: Option<&str>) -> bool {
    let init = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
    let ok = focus(terminal_pid, title_hint);
    if init.is_ok() {
        unsafe { CoUninitialize() };
    }
    ok
}

fn focus(terminal_pid: u32, title_hint: Option<&str>) -> bool {
    let mut wins = terminal_windows();
    // Stable sort keeps Z-order, so the pid's most recently active window comes first.
    wins.sort_by_key(|&(_, pid)| pid != terminal_pid);
    if let Some(hint) = title_hint
        && let Some((hwnd, tab)) = find_tab(&wins, hint)
    {
        let ok = activate(hwnd);
        // Selecting after activation; a failed Select still leaves the right window in front.
        let _ = unsafe { tab.GetCurrentPatternAs::<IUIAutomationSelectionItemPattern>(UIA_SelectionItemPatternId).and_then(|p| p.Select()) };
        return ok;
    }
    wins.iter().find(|&&(_, pid)| pid == terminal_pid).is_some_and(|&(hwnd, _)| activate(hwnd))
}

/// The session's AI title: the last `{"type":"ai-title","aiTitle":..}` line of the transcript.
pub fn ai_title(transcript: &Path) -> Option<String> {
    let file = std::fs::File::open(transcript).ok()?;
    let mut title = None;
    for line in BufReader::new(file).split(b'\n').map_while(Result::ok) {
        let Ok(line) = std::str::from_utf8(&line) else { continue };
        if !line.contains("\"ai-title\"") {
            continue;
        }
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(line)
            && v["type"] == "ai-title"
            && let Some(t) = v["aiTitle"].as_str()
        {
            title = Some(t.to_string());
        }
    }
    title
}

/// A Windows Terminal tab is named "<status glyph> <AI title>", so match on the ending.
pub fn tab_matches(name: &str, hint: &str) -> bool {
    !hint.is_empty() && name.trim().ends_with(hint)
}

/// Visible Windows Terminal top-level windows with their owning pids, in Z-order (topmost first).
fn terminal_windows() -> Vec<(HWND, u32)> {
    unsafe extern "system" fn each(hwnd: HWND, lp: LPARAM) -> BOOL {
        unsafe {
            let out = &mut *(lp.0 as *mut Vec<(HWND, u32)>);
            let mut class = [0u16; 64];
            let n = GetClassNameW(hwnd, &mut class) as usize;
            if IsWindowVisible(hwnd).as_bool() && class[..n] == *windows::core::w!("CASCADIA_HOSTING_WINDOW_CLASS").as_wide() {
                let mut pid = 0;
                GetWindowThreadProcessId(hwnd, Some(&mut pid));
                out.push((hwnd, pid));
            }
        }
        true.into()
    }
    let mut out = Vec::new();
    let _ = unsafe { EnumWindows(Some(each), LPARAM(&mut out as *mut _ as isize)) };
    out
}

/// The first tab, across `wins` in order, whose name matches `hint`. Needs COM initialized.
fn find_tab(wins: &[(HWND, u32)], hint: &str) -> Option<(HWND, IUIAutomationElement)> {
    unsafe {
        let uia: IUIAutomation = CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER).ok()?;
        let all = uia.CreateTrueCondition().ok()?;
        for &(hwnd, _) in wins {
            let Ok(found) = uia.ElementFromHandle(hwnd).and_then(|w| w.FindAll(TreeScope_Descendants, &all)) else { continue };
            for i in 0..found.Length().unwrap_or(0) {
                let Ok(t) = found.GetElement(i) else { continue };
                if t.CurrentControlType().is_ok_and(|c| c == UIA_TabItemControlTypeId)
                    && t.CurrentName().is_ok_and(|n| tab_matches(&n.to_string(), hint))
                {
                    return Some((hwnd, t));
                }
            }
        }
        None
    }
}

/// Restores and foregrounds `hwnd`; falls back to attaching to the foreground thread's input when refused.
fn activate(hwnd: HWND) -> bool {
    unsafe {
        if IsIconic(hwnd).as_bool() {
            let _ = ShowWindow(hwnd, SW_RESTORE);
        }
        if SetForegroundWindow(hwnd).as_bool() {
            return true;
        }
        let fg = GetWindowThreadProcessId(GetForegroundWindow(), None);
        let me = GetCurrentThreadId();
        let attached = AttachThreadInput(me, fg, true).as_bool();
        let _ = BringWindowToTop(hwnd);
        let ok = SetForegroundWindow(hwnd).as_bool();
        if attached {
            let _ = AttachThreadInput(me, fg, false);
        }
        ok
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn transcript(name: &str, body: &str) -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!("agent-islands-{}-{name}.jsonl", std::process::id()));
        std::fs::write(&p, body).unwrap();
        p
    }

    #[test]
    fn tab_matches_ignores_leading_glyph() {
        assert!(tab_matches("◐ AskUserQuestion probe with mkdir", "AskUserQuestion probe with mkdir"));
        assert!(tab_matches("✳ Some title ", "Some title"));
        assert!(!tab_matches("✳ Some title", "some title"));
        assert!(!tab_matches("✳ Some title extra", "Some title"));
        assert!(!tab_matches("Windows PowerShell", "Some title"));
    }

    #[test]
    fn ai_title_takes_last_and_skips_malformed() {
        let p = transcript(
            "last",
            "{\"type\":\"ai-title\",\"aiTitle\":\"Old\"}\n{\"type\":\"user\",\"message\":\"x\"}\n{\"type\":\"ai-title\",\"aiTitle\":\"New\"}\n{\"type\":\"ai-title\" broken\n",
        );
        assert_eq!(ai_title(&p).as_deref(), Some("New"));
        std::fs::remove_file(p).unwrap();
    }

    #[test]
    fn ai_title_none_without_title_lines() {
        let p = transcript("none", "{\"type\":\"user\"}\n");
        assert_eq!(ai_title(&p), None);
        std::fs::remove_file(p).unwrap();
        assert_eq!(ai_title(Path::new("Z:/no/such/file.jsonl")), None);
    }

    #[test]
    fn bogus_pid_returns_false() {
        assert!(!go_to_terminal(0xFFFF_FFF0, None));
        assert!(!go_to_terminal(0xFFFF_FFF0, Some("Some title")));
    }
}
