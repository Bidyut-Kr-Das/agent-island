//! Spec 9a: risky shell commands always come to the island for Allow/Deny.

//!
//! No shell parsing: the whole command becomes a flat list of words, so wrappers (`sudo`, `bash -c`,
//! `iex`, `$(...)`, script blocks, paths, `.exe`) cannot hide a listed command. It over-gates on
//! purpose (`echo rm` is gated); over-gating is the safe direction.

/// Gated wherever they appear as a word.
const COMMANDS: &[&str] = &["rm", "rmdir", "del", "erase", "rd", "remove-item", "ri"];

/// Gated when one of the two words after the tool (flags skipped) is a listed subcommand. Two words,
/// not one, so a flag value (`git -C dir push`, `npm --prefix x install`) cannot hide it.
/// `python -m pip install` and `uv pip install` are caught by the pip entry.
const SUBCOMMANDS: &[(&str, &[&str])] = &[
    ("git", &["commit", "push", "reset"]),
    ("npm", &["install", "i", "add"]),
    ("pnpm", &["add", "install", "i"]),
    ("yarn", &["add", "install"]),
    ("bun", &["add", "install"]),
    ("pip", &["install"]),
    ("pip3", &["install"]),
    ("uv", &["add"]),
    ("cargo", &["add", "install"]),
];

/// `\rm`, `/usr/bin/rm`, `C:\...\git.exe`, `GIT` and `git \` (a continuation) all become their bare name.
fn normalize(word: &str) -> String {
    let w = word.trim_matches('\\');
    let w = w.rsplit(['/', '\\']).next().unwrap_or_default().to_ascii_lowercase();
    for ext in [".exe", ".cmd", ".bat", ".ps1"] {
        if let Some(stem) = w.strip_suffix(ext) {
            return stem.to_owned();
        }
    }
    w
}

pub fn is_gated(tool: &str, command: &str) -> bool {
    if tool != "Bash" && tool != "PowerShell" {
        return false;
    }
    let words: Vec<String> = command
        .split(|c: char| c.is_whitespace() || ";&|(){}<>`$\"',".contains(c))
        .map(normalize)
        .filter(|w| !w.is_empty())
        .collect();
    words.iter().enumerate().any(|(i, w)| {
        COMMANDS.contains(&w.as_str())
            || SUBCOMMANDS.iter().any(|(t, subs)| {
                t == w
                    && words[i + 1..]
                        .iter()
                        .filter(|x| !x.starts_with(['-', '+']))
                        .take(2)
                        .any(|x| subs.contains(&x.as_str()))
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
        for c in ["cargo build", "cargo test", "npm run build", "npx tsc --noEmit", "pytest", "git status", "git log", "rmx", "git committee", "npm installer", "python -m pip list", "cargo build && git log"] {
            assert!(!g(c), "{c}");
        }
    }

    #[test]
    fn wrapped_and_prefixed_forms_gated() {
        for c in [
            // git global options
            "git -C dir push", "git --no-pager commit", "git -c k=v commit", "git --git-dir=x/.git push",
            // env prefix, options before the subcommand
            "FOO=1 npm install", "npm --prefix x install", "cargo +nightly install x",
            // shell wrappers
            "cmd /c del x", "powershell -c \"Remove-Item x\"", "pwsh -Command Remove-Item x", "bash -c \"rm -rf x\"",
            "sh -c 'rm x'", "iex \"rm x\"", "Invoke-Expression 'git push'",
            // prefix commands
            "sudo rm x", "env rm x", "find . | xargs rm", "time rm x", "nohup rm x", "exec rm x", "command rm x", "\\rm x",
            // path-qualified, extensions
            "/usr/bin/rm x", "rm.exe x", "git.exe push", "npm.cmd install", "GIT.EXE PUSH",
            r"C:\Program Files\Git\cmd\git.exe push", r#""C:\Program Files\Git\cmd\git.exe" push"#,
            r"Microsoft.PowerShell.Management\Remove-Item x",
            // grouping, substitution, script blocks
            "(rm x)", "{ rm x; }", "echo $(rm x)", "echo `rm x`", "ls | % { rm $_ }", "if (1) { git push }",
            // line continuations
            "git \\\npush", "git `\npush", "git\\\npush",
            // other forms
            "uv pip install x", "python3 -m pip install x",
        ] {
            assert!(g(c), "{c:?}");
        }
    }

    #[test]
    fn accepted_false_positives_are_gated_on_purpose() {
        for c in ["echo rm", "git log --grep rm", "git commit -m \"x\" && echo \"del\"", "echo \"please rm this\""] {
            assert!(g(c), "{c}");
        }
    }

    #[test]
    fn other_tools_never_gated() {
        assert!(!is_gated("Edit", "rm x"));
        assert!(!is_gated("Write", "git push"));
        assert!(is_gated("PowerShell", "git push"));
    }
}
