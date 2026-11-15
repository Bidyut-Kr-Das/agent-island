//! Spec 9a: risky shell commands always come to the island for Allow/Deny.

const GATED: &[&str] = &[
    "rm", "rmdir", "del", "erase", "rd", "Remove-Item", "ri",
    "git commit", "git push", "git reset",
    "npm install", "npm i", "npm add", "pnpm add", "pnpm install", "pnpm i", "yarn add", "yarn install",
    "bun add", "bun install", "pip install", "pip3 install", "python -m pip install", "uv add",
    "uv pip install", "cargo add", "cargo install",
];

/// Splitting on every `&` (not only `&&`) also covers a background `&` and the PowerShell `&` call
/// operator; it can only gate more, never less.
pub fn is_gated(tool: &str, command: &str) -> bool {
    if tool != "Bash" && tool != "PowerShell" {
        return false;
    }
    command.split([';', '|', '&', '\n', '\r']).any(|seg| {
        let mut words: Vec<&str> = seg.split_whitespace().map(|w| w.trim_matches(['"', '\''])).collect();
        if words.first() == Some(&".") {
            words.remove(0);
        }
        GATED.iter().any(|entry| {
            let want: Vec<&str> = entry.split(' ').collect();
            words.len() >= want.len() && want.iter().zip(&words).all(|(a, b)| a.eq_ignore_ascii_case(b))
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn g(cmd: &str) -> bool {
        is_gated("Bash", cmd)
    }

    #[test]
    fn gates_each_listed_prefix() {
        assert!(g("rm x"));
        assert!(g("rmdir x"));
        assert!(g("del x"));
        assert!(g("erase x"));
        assert!(g("rd x"));
        assert!(g("Remove-Item x"));
        assert!(g("ri x"));
        assert!(g("git commit -m x"));
        assert!(g("git push"));
        assert!(g("git reset --hard"));
        assert!(g("npm install"));
        assert!(g("npm i x"));
        assert!(g("npm add x"));
        assert!(g("pnpm add x"));
        assert!(g("pnpm install"));
        assert!(g("pnpm i"));
        assert!(g("yarn add x"));
        assert!(g("yarn install"));
        assert!(g("bun add x"));
        assert!(g("bun install"));
        assert!(g("pip install x"));
        assert!(g("pip3 install x"));
        assert!(g("python -m pip install x"));
        assert!(g("uv add x"));
        assert!(g("uv pip install x"));
        assert!(g("cargo add x"));
        assert!(g("cargo install x"));
    }

    #[test]
    fn matches_any_segment() {
        assert!(g("cargo build && git push"));
        assert!(g("echo a | rm x"));
        assert!(g("echo a; rm x"));
        assert!(g("false || rm x"));
        assert!(g("echo a\nrm x"));
    }

    #[test]
    fn strips_call_operator_and_quotes() {
        assert!(is_gated("PowerShell", "& \"Remove-Item\" x"));
        assert!(is_gated("PowerShell", ". 'rm' x"));
        assert!(g("  git push  "));
    }

    #[test]
    fn case_insensitive_whole_words() {
        assert!(g("Remove-Item x"));
        assert!(g("remove-item x"));
        assert!(g("GIT PUSH"));
        assert!(!g("rmx"));
        assert!(!g("git committee"));
        assert!(!g("npm installer"));
        assert!(g("npm i -D x"));
        assert!(!g("python -m pip list"));
    }

    #[test]
    fn builds_tests_typechecks_not_gated() {
        for c in ["cargo build", "cargo test", "npm run build", "npx tsc --noEmit", "pytest", "git status", "git log", "echo rm"] {
            assert!(!g(c), "{c}");
        }
    }

    #[test]
    fn other_tools_never_gated() {
        assert!(!is_gated("Edit", "rm x"));
        assert!(!is_gated("Write", "git push"));
        assert!(is_gated("PowerShell", "git push"));
    }
}
