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

#[cfg(test)]
mod tests {
    use super::*;

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
