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

pub fn save_to(path: &Path, side: Side) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let s = if side == Side::Right { "right" } else { "left" };
    std::fs::write(path, serde_json::json!({ "side": s }).to_string())
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
}
