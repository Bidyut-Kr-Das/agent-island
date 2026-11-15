use crate::state::Pids;
use std::collections::HashMap;
use std::path::PathBuf;
use windows::Win32::Foundation::{CloseHandle, FILETIME, HANDLE, STILL_ACTIVE};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::Threading::{
    GetExitCodeProcess, GetProcessTimes, OpenProcess, PROCESS_NAME_WIN32,
    PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
};
use windows::core::PWSTR;

/// Opens `pid` for limited query, runs `f`, always closes the handle.
fn with_process<T>(pid: u32, f: impl FnOnce(HANDLE) -> Option<T>) -> Option<T> {
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let r = f(h);
        let _ = CloseHandle(h);
        r
    }
}

fn creation_time(pid: u32) -> Option<u64> {
    with_process(pid, |h| unsafe {
        let (mut c, mut e, mut k, mut u) = (FILETIME::default(), FILETIME::default(), FILETIME::default(), FILETIME::default());
        GetProcessTimes(h, &mut c, &mut e, &mut k, &mut u).ok()?;
        Some(((c.dwHighDateTime as u64) << 32) | c.dwLowDateTime as u64)
    })
}

/// pid -> (parent pid, exe file name) for every process, from one snapshot.
fn snapshot() -> HashMap<u32, (u32, String)> {
    let mut map = HashMap::new();
    unsafe {
        let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else { return map };
        let mut e = PROCESSENTRY32W { dwSize: size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
        let mut ok = Process32FirstW(snap, &mut e).is_ok();
        while ok {
            let len = e.szExeFile.iter().position(|&c| c == 0).unwrap_or(e.szExeFile.len());
            map.insert(e.th32ProcessID, (e.th32ParentProcessID, String::from_utf16_lossy(&e.szExeFile[..len])));
            ok = Process32NextW(snap, &mut e).is_ok();
        }
        let _ = CloseHandle(snap);
    }
    map
}

/// (pid, exe name), self first. Stops at the root, a cycle, or a PID-reused parent
/// (parent created after its child, or creation time unreadable).
pub fn ancestors(pid: u32) -> Vec<(u32, String)> {
    let procs = snapshot();
    let mut out: Vec<(u32, String)> = Vec::new();
    let mut cur = pid;
    let mut cur_time = None;
    while let Some((ppid, name)) = procs.get(&cur) {
        if out.iter().any(|(p, _)| *p == cur) {
            break;
        }
        let t = creation_time(cur);
        if let (Some(child), Some(this)) = (cur_time, t) {
            if this > child {
                break;
            }
        } else if !out.is_empty() && t.is_none() {
            break;
        }
        out.push((cur, name.clone()));
        cur_time = t;
        cur = *ppid;
    }
    out
}

pub fn find_pids(chain: &[(u32, String)]) -> Pids {
    let find = |n: &str| chain.iter().find(|(_, name)| name.eq_ignore_ascii_case(n)).map(|(p, _)| *p);
    Pids { claude: find("claude.exe"), terminal: find("WindowsTerminal.exe") }
}

pub fn is_alive(pid: u32) -> bool {
    with_process(pid, |h| unsafe {
        let mut code = 0u32;
        GetExitCodeProcess(h, &mut code).ok()?;
        Some(code == STILL_ACTIVE.0 as u32)
    })
    .unwrap_or(false)
}

pub fn exe_path(pid: u32) -> Option<PathBuf> {
    with_process(pid, |h| unsafe {
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len).ok()?;
        Some(PathBuf::from(String::from_utf16_lossy(&buf[..len as usize])))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chain(names: &[&str]) -> Vec<(u32, String)> {
        names.iter().enumerate().map(|(i, n)| (i as u32 + 10, n.to_string())).collect()
    }

    #[test]
    fn ancestors_of_self_starts_with_self_and_includes_parent() {
        let a = ancestors(std::process::id());
        assert_eq!(a[0].0, std::process::id());
        assert!(a.len() >= 2);
    }

    #[test]
    fn find_pids_picks_names() {
        let p = find_pids(&chain(&["hook.exe", "bash.exe", "bash.exe", "Claude.exe", "cmd.exe", "WindowsTerminal.exe"]));
        assert_eq!(p.claude, Some(13));
        assert_eq!(p.terminal, Some(15));
    }

    #[test]
    fn find_pids_returns_none_when_names_absent() {
        let p = find_pids(&chain(&["hook.exe", "bash.exe", "explorer.exe"]));
        assert_eq!(p.claude, None);
        assert_eq!(p.terminal, None);
    }

    #[test]
    fn is_alive_self_true_and_bogus_false() {
        assert!(is_alive(std::process::id()));
        assert!(!is_alive(u32::MAX - 3));
    }

    #[test]
    fn exe_path_of_self_matches_current_exe() {
        let got = exe_path(std::process::id()).unwrap().to_string_lossy().to_lowercase();
        let want = std::env::current_exe().unwrap().to_string_lossy().to_lowercase();
        assert_eq!(got, want);
    }
}
