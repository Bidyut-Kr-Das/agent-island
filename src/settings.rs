use std::path::{Path, PathBuf};

/// Which screen edge the island docks to.
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub enum Side {
    #[default]
    Left,
    Right,
}

/// `%APPDATA%\agent-islands\settings.json`.
pub fn path() -> Option<PathBuf> {
    Some(PathBuf::from(std::env::var_os("APPDATA")?).join("agent-islands").join("settings.json"))
}

/// Anything missing or unreadable is Left.
pub fn load_from(path: &Path) -> Side {
    let v: Option<serde_json::Value> = std::fs::read(path).ok().and_then(|b| serde_json::from_slice(&b).ok());
    match v.as_ref().and_then(|v| v["side"].as_str()) {
        Some("right") => Side::Right,
        _ => Side::Left,
    }
}

/// Anything missing or unreadable is None.
pub fn load_chat(path: &Path) -> Option<i64> {
    let v: serde_json::Value = serde_json::from_slice(&std::fs::read(path).ok()?).ok()?;
    v["phone_chat"].as_i64()
}

/// Anything missing or unreadable is false.
pub fn load_away(path: &Path) -> bool {
    std::fs::read(path).ok().and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok()).is_some_and(|v| v["away"] == true)
}

/// Replaces one key and keeps the rest. Unreadable or non-object content starts from `{}`.
pub fn set(path: &Path, key: &str, value: serde_json::Value) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut v: serde_json::Value = std::fs::read(path)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .filter(|v: &serde_json::Value| v.is_object())
        .unwrap_or_else(|| serde_json::json!({}));
    v[key] = value;
    std::fs::write(path, v.to_string())
}

pub fn save_to(path: &Path, side: Side) -> std::io::Result<()> {
    let s = if side == Side::Right { "right" } else { "left" };
    set(path, "side", serde_json::json!(s))
}

pub const RUN_NAME: &str = "AgentIslands";
const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const ENTROPY: &[u8] = b"agent-islands phone token v1";

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// DPAPI-encrypts for the current user, returns lowercase hex.
pub fn protect(plain: &str) -> Option<String> {
    use windows::Win32::Security::Cryptography::{CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptProtectData};
    let input = CRYPT_INTEGER_BLOB { cbData: plain.len() as u32, pbData: plain.as_ptr() as *mut u8 };
    let ent = CRYPT_INTEGER_BLOB { cbData: ENTROPY.len() as u32, pbData: ENTROPY.as_ptr() as *mut u8 };
    let mut out = CRYPT_INTEGER_BLOB::default();
    unsafe { CryptProtectData(&input, None, Some(&ent), None, None, CRYPTPROTECT_UI_FORBIDDEN, &mut out) }.ok()?;
    blob_take(out).map(|b| b.iter().map(|x| format!("{x:02x}")).collect())
}

/// Copies the DPAPI output and frees it.
fn blob_take(b: windows::Win32::Security::Cryptography::CRYPT_INTEGER_BLOB) -> Option<Vec<u8>> {
    use windows::Win32::Foundation::{HLOCAL, LocalFree};
    if b.pbData.is_null() {
        return None;
    }
    let v = unsafe { std::slice::from_raw_parts(b.pbData, b.cbData as usize) }.to_vec();
    unsafe { LocalFree(Some(HLOCAL(b.pbData as _))) };
    Some(v)
}

/// None on bad hex, tampering, or another user's blob.
pub fn unprotect(hex: &str) -> Option<String> {
    use windows::Win32::Security::Cryptography::{CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptUnprotectData};
    if hex.is_empty() || hex.len() % 2 != 0 || !hex.is_ascii() {
        return None;
    }
    let mut bytes = (0..hex.len()).step_by(2).map(|i| u8::from_str_radix(&hex[i..i + 2], 16).ok()).collect::<Option<Vec<u8>>>()?;
    let input = CRYPT_INTEGER_BLOB { cbData: bytes.len() as u32, pbData: bytes.as_mut_ptr() };
    let ent = CRYPT_INTEGER_BLOB { cbData: ENTROPY.len() as u32, pbData: ENTROPY.as_ptr() as *mut u8 };
    let mut out = CRYPT_INTEGER_BLOB::default();
    unsafe { CryptUnprotectData(&input, None, Some(&ent), None, None, CRYPTPROTECT_UI_FORBIDDEN, &mut out) }.ok()?;
    String::from_utf8(blob_take(out)?).ok()
}

/// Reads `phone_token` and decrypts it.
pub fn load_token(path: &Path) -> Option<String> {
    let v: serde_json::Value = serde_json::from_slice(&std::fs::read(path).ok()?).ok()?;
    unprotect(v["phone_token"].as_str()?)
}

/// `Some` writes the quoted exe path under HKCU Run; `None` removes it (missing is success).
pub fn set_run(name: &str, exe: Option<&Path>) -> bool {
    use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS};
    use windows::Win32::System::Registry::{HKEY_CURRENT_USER, REG_SZ, RegDeleteKeyValueW, RegSetKeyValueW};
    use windows::core::PCWSTR;
    let (key, nm) = (wide(RUN_KEY), wide(name));
    unsafe {
        match exe {
            Some(p) => {
                let data = wide(&format!("\"{}\"", p.display()));
                RegSetKeyValueW(HKEY_CURRENT_USER, PCWSTR(key.as_ptr()), PCWSTR(nm.as_ptr()), REG_SZ.0, Some(data.as_ptr().cast()), (data.len() * 2) as u32) == ERROR_SUCCESS
            }
            None => {
                let r = RegDeleteKeyValueW(HKEY_CURRENT_USER, PCWSTR(key.as_ptr()), PCWSTR(nm.as_ptr()));
                r == ERROR_SUCCESS || r == ERROR_FILE_NOT_FOUND
            }
        }
    }
}

/// Only the tests read the login entry back.
#[cfg(test)]
pub fn get_run(name: &str) -> Option<String> {
    use windows::Win32::Foundation::ERROR_SUCCESS;
    use windows::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_SZ, RegGetValueW};
    use windows::core::PCWSTR;
    let (key, nm) = (wide(RUN_KEY), wide(name));
    let mut size = 0u32;
    unsafe { RegGetValueW(HKEY_CURRENT_USER, PCWSTR(key.as_ptr()), PCWSTR(nm.as_ptr()), RRF_RT_REG_SZ, None, None, Some(&mut size)) }.ok().ok()?;
    let mut buf = vec![0u16; (size as usize).div_ceil(2)];
    let r = unsafe { RegGetValueW(HKEY_CURRENT_USER, PCWSTR(key.as_ptr()), PCWSTR(nm.as_ptr()), RRF_RT_REG_SZ, None, Some(buf.as_mut_ptr().cast()), Some(&mut size)) };
    if r != ERROR_SUCCESS {
        return None;
    }
    let n = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    Some(String::from_utf16_lossy(&buf[..n]))
}

/// `Some(non-empty)` stores the token encrypted and adds the login entry; otherwise clears both.
/// Only an installed plugin exe writes the login entry, so a dev build never takes it.
pub fn autostart(path: &Path, token: Option<&str>) {
    let exe = std::env::current_exe().ok().filter(|e| crate::hook::install(e).is_some());
    autostart_with(path, token, RUN_NAME, exe.as_deref());
}

/// `exe` None: the token is still stored, the login entry is left as it is.
fn autostart_with(path: &Path, token: Option<&str>, run_name: &str, exe: Option<&Path>) {
    match token.filter(|t| !t.is_empty()).and_then(protect) {
        Some(hex) => {
            let _ = set(path, "phone_token", hex.into());
            if let Some(exe) = exe {
                set_run(run_name, Some(exe));
            }
        }
        None => {
            let _ = set(path, "phone_token", serde_json::Value::Null);
            set_run(run_name, None);
        }
    }
}

/// `agent-islands.exe forget`: unpairs fully, so the next hook token cannot re-store anything.
pub fn forget(path: &Path) {
    forget_with(path, RUN_NAME);
}

fn forget_with(path: &Path, run_name: &str) {
    autostart_with(path, None, run_name, None);
    let _ = set(path, "away", false.into());
    let _ = set(path, "phone_chat", serde_json::Value::Null);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Deletes a test login entry even when the test panics.
    struct RunGuard(String);
    impl Drop for RunGuard {
        fn drop(&mut self) {
            set_run(&self.0, None);
        }
    }

    fn exe() -> PathBuf {
        std::env::current_exe().unwrap()
    }

    #[test]
    fn forget_unpairs_and_clears_token() {
        let p = tmp("forget");
        let name = RunGuard(format!("AgentIslandsTestForget{}", std::process::id()));
        let name = &name.0;
        set(&p, "phone_chat", serde_json::json!(42)).unwrap();
        set(&p, "away", serde_json::json!(true)).unwrap();
        autostart_with(&p, Some("t:1"), name, Some(&exe()));
        forget_with(&p, name);
        assert_eq!((load_chat(&p), load_away(&p), load_token(&p), get_run(name)), (None, false, None, None));
    }

    #[test]
    fn not_installed_stores_token_without_login_entry() {
        let p = tmp("noinst");
        let name = RunGuard(format!("AgentIslandsTestNoInst{}", std::process::id()));
        autostart_with(&p, Some("t:1"), &name.0, None);
        assert_eq!((load_token(&p).as_deref(), get_run(&name.0)), (Some("t:1"), None));
        assert!(crate::hook::install(&exe()).is_none(), "test exe is not an install");
    }

    #[test]
    fn dpapi_roundtrip_and_tamper() {
        let h = protect("123:ABC").unwrap();
        assert!(!h.contains("123:ABC"));
        assert_eq!(unprotect(&h).as_deref(), Some("123:ABC"));
        let mut bad = h.clone();
        // tail is the integrity MAC; the brief's bytes 10..12 sit in a header field DPAPI ignores
        let n = h.len();
        bad.replace_range(n - 2..n, if &h[n - 2..] == "00" { "11" } else { "00" });
        assert_eq!(unprotect(&bad), None);
        assert_eq!(unprotect("zz"), None);
    }

    #[test]
    fn autostart_stores_and_clears_token() {
        let p = tmp("auto");
        let guard = RunGuard(format!("AgentIslandsTest{}", std::process::id()));
        let name = &guard.0;
        save_to(&p, Side::Right).unwrap();
        autostart_with(&p, Some("t:1"), name, Some(&exe()));
        let stored = load_token(&p);
        let run = get_run(name);
        let raw = std::fs::read_to_string(&p).unwrap();
        autostart_with(&p, None, name, None);
        assert_eq!(stored.as_deref(), Some("t:1"));
        assert!(!raw.contains("t:1"));
        assert!(run.is_some_and(|v| v.starts_with('"') && v.ends_with('"')));
        assert_eq!((load_token(&p), get_run(&name), load_from(&p)), (None, None, Side::Right));
    }

    fn tmp(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("ai-settings-{}-{name}", std::process::id())).join("settings.json")
    }

    #[test]
    fn roundtrip_right() {
        let p = tmp("rt");
        save_to(&p, Side::Right).unwrap();
        assert_eq!(load_from(&p), Side::Right);
        save_to(&p, Side::Left).unwrap();
        assert_eq!(load_from(&p), Side::Left);
    }

    #[test]
    fn missing_file_is_left() {
        assert_eq!(load_from(&tmp("missing")), Side::Left);
    }

    #[test]
    fn garbage_is_left() {
        let p = tmp("garbage");
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        for g in ["not json", r#"{"side":3}"#, r#"{"side":"top"}"#] {
            std::fs::write(&p, g).unwrap();
            assert_eq!(load_from(&p), Side::Left, "{g}");
        }
    }

    #[test]
    fn set_keeps_other_keys() {
        let p = tmp("rmw");
        save_to(&p, Side::Right).unwrap();
        set(&p, "phone_chat", serde_json::json!(42)).unwrap();
        assert_eq!((load_from(&p), load_chat(&p)), (Side::Right, Some(42)));
        save_to(&p, Side::Left).unwrap();
        assert_eq!(load_chat(&p), Some(42));
        set(&p, "phone_chat", serde_json::Value::Null).unwrap();
        assert_eq!(load_chat(&p), None);
    }

    #[test]
    fn away_roundtrip_keeps_side_and_chat() {
        let p = tmp("away");
        assert!(!load_away(&p));
        save_to(&p, Side::Right).unwrap();
        set(&p, "phone_chat", serde_json::json!(42)).unwrap();
        set(&p, "away", serde_json::json!(true)).unwrap();
        assert_eq!((load_away(&p), load_from(&p), load_chat(&p)), (true, Side::Right, Some(42)));
        set(&p, "away", serde_json::json!(false)).unwrap();
        assert!(!load_away(&p));
        std::fs::write(&p, "not json").unwrap();
        assert!(!load_away(&p));
    }

    #[test]
    fn set_over_garbage_starts_fresh() {
        let p = tmp("rmwg");
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, "not json").unwrap();
        set(&p, "phone_chat", serde_json::json!(7)).unwrap();
        assert_eq!(load_chat(&p), Some(7));
    }
}
