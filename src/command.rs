use std::ffi::OsString;
use std::path::{Path, PathBuf};

#[derive(Debug, PartialEq, Clone)]
pub enum Cmd { Open(Vec<String>), Claude(Vec<String>), Help, Away(bool) }

pub const HELP: &str = "/open <name> - new terminal tab in the zoxide match for <name>\n/claude <name> - same, then start claude\n/away on|off - turn Away on or off\n/help - this list";

/// None: not a command. Err: the exact usage reply.
pub fn parse(text: &str) -> Option<Result<Cmd, String>> {
    let t = text.trim().strip_prefix('/')?;
    let (head, rest) = t.split_once(char::is_whitespace).unwrap_or((t, ""));
    let verb = head.split('@').next().unwrap_or("").to_lowercase();
    let rest = rest.trim();
    let words: Vec<String> = rest.split_whitespace().map(String::from).collect();
    let name_ok = !rest.is_empty() && rest.chars().count() <= 100 && words.len() <= 8 && !rest.chars().any(char::is_control);
    Some(match verb.as_str() {
        "help" => Ok(Cmd::Help),
        "open" if name_ok => Ok(Cmd::Open(words)),
        "open" => Err("Usage: /open <name>".into()),
        "claude" if name_ok => Ok(Cmd::Claude(words)),
        "claude" => Err("Usage: /claude <name>".into()),
        "away" => match words.as_slice() {
            [w] if w.eq_ignore_ascii_case("on") => Ok(Cmd::Away(true)),
            [w] if w.eq_ignore_ascii_case("off") => Ok(Cmd::Away(false)),
            _ => Err("Usage: /away on|off".into()),
        },
        _ => return None,
    })
}

/// The bot a command names, as in `/open@MyBot x`.
pub fn addressee(text: &str) -> Option<&str> {
    let t = text.trim().strip_prefix('/')?;
    t.split(char::is_whitespace).next()?.split_once('@').map(|(_, b)| b)
}

/// zoxide stdout -> existing absolute dir that is safe to hand to wt (no `;` or `"`).
pub fn check_dir(stdout: &str) -> Option<PathBuf> {
    let s = stdout.trim_end_matches(['\r', '\n']);
    if s.is_empty() || s.contains([';', '"']) { return None; }
    // Drive roots like `C:\` keep their slash (wt -d C:\ works).
    let s = if s.len() > 3 { s.strip_suffix('\\').unwrap_or(s) } else { s };
    let p = PathBuf::from(s);
    (p.is_absolute() && p.is_dir()).then_some(p)
}

pub fn wt_args(cmd: &Cmd, dir: &Path) -> Vec<OsString> {
    let mut a: Vec<OsString> = ["-w", "0", "nt", "-d"].map(Into::into).to_vec();
    a.push(dir.into());
    if matches!(cmd, Cmd::Claude(_)) {
        a.extend(["powershell.exe", "-NoLogo", "-NoExit", "-Command", "claude"].map(OsString::from));
    }
    a
}

pub enum QueryError { NotInstalled, NoMatch, Failed }

pub fn run(
    cmd: &Cmd,
    query: impl Fn(&[String]) -> Result<String, QueryError>,
    launch: impl Fn(&[OsString]) -> std::io::Result<()>,
) -> String {
    // Away never reaches run (routing handles it); HELP is the harmless answer.
    let (Cmd::Open(words) | Cmd::Claude(words)) = cmd else { return HELP.into() };
    let name = words.join(" ");
    let out = match query(words) {
        Ok(o) => o,
        Err(QueryError::NotInstalled) => return "zoxide is not installed".into(),
        Err(QueryError::NoMatch) => return format!("No folder matches {name}"),
        Err(QueryError::Failed) => return format!("Could not look up {name}"),
    };
    let Some(dir) = check_dir(&out) else { return format!("No folder matches {name}") };
    if launch(&wt_args(cmd, &dir)).is_err() { return "Could not open Windows Terminal".into(); }
    match cmd {
        Cmd::Claude(_) => format!("Started claude in {} (accept the folder trust prompt on the PC if it is new)", dir.display()),
        _ => format!("Opened {}", dir.display()),
    }
}

fn zoxide(words: &[String]) -> Result<String, QueryError> {
    use std::io::Read;
    use std::os::windows::process::CommandExt;
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};
    let mut cmd = Command::new("zoxide");
    let mut child = clean(&mut cmd).arg("query").arg("--").args(words)
        .creation_flags(crate::hook::CREATE_NO_WINDOW)
        .stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null())
        .spawn()
        .map_err(|e| if e.kind() == std::io::ErrorKind::NotFound { QueryError::NotInstalled } else { QueryError::Failed })?;
    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break s,
            Ok(None) if start.elapsed() < Duration::from_secs(5) => std::thread::sleep(Duration::from_millis(50)),
            _ => { let _ = child.kill(); let _ = child.wait(); return Err(QueryError::Failed); }
        }
    };
    match status.code() {
        Some(0) => {}
        Some(1) => return Err(QueryError::NoMatch),
        _ => return Err(QueryError::Failed),
    }
    let mut s = String::new();
    child.stdout.take().ok_or(QueryError::Failed)?.read_to_string(&mut s).map_err(|_| QueryError::Failed)?;
    Ok(s)
}

/// A hook-started island inherits the bot token and Claude Code's variables; children must not.
fn clean(cmd: &mut std::process::Command) -> &mut std::process::Command {
    for (k, _) in std::env::vars_os() {
        if k.to_string_lossy().to_uppercase().starts_with("CLAUDE") {
            cmd.env_remove(k);
        }
    }
    cmd
}

fn launch(args: &[OsString]) -> std::io::Result<()> {
    clean(&mut std::process::Command::new("wt.exe")).args(args).spawn().map(drop)
}

pub fn spawn(cmd: Cmd, out: std::sync::mpsc::Sender<crate::telegram::Out>) {
    std::thread::spawn(move || {
        let text = run(&cmd, zoxide, launch);
        let _ = out.send(crate::telegram::Out::Send { session_id: None, text, buttons: vec![], tag: None, entities: vec![] });
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn clean_removes_claude_vars() {
        let name = format!("CLAUDE_AI_TEST_{}", std::process::id());
        // SAFETY: unique name; no other test reads or writes it.
        unsafe { std::env::set_var(&name, "secret") };
        let mut cmd = std::process::Command::new("wt.exe");
        clean(&mut cmd);
        let removed: Vec<OsString> = cmd.get_envs().filter(|(_, v)| v.is_none()).map(|(k, _)| k.to_owned()).collect();
        unsafe { std::env::remove_var(&name) };
        assert!(removed.iter().any(|k| k.eq_ignore_ascii_case(&name)), "{removed:?}");
        assert!(removed.iter().all(|k| k.to_string_lossy().to_uppercase().starts_with("CLAUDE")));
        for (k, _) in std::env::vars_os().filter(|(k, _)| k.to_string_lossy().to_uppercase().starts_with("CLAUDE")) {
            assert!(removed.iter().any(|r| r.eq_ignore_ascii_case(&k)), "{k:?} kept");
        }
    }
    #[test] fn addressee_is_the_at_suffix() {
        assert_eq!(addressee("/open@MyBot x"), Some("MyBot"));
        assert_eq!((addressee("/open x@y"), addressee("hi @b"), addressee("/help")), (None, None, None));
    }
    #[test] fn run_passes_words_verbatim() {
        let seen = std::cell::RefCell::new(vec![]);
        let reply = run(&Cmd::Open(vec!["foo\"".into(), "&".into(), "$(x)".into()]),
            |w| { seen.borrow_mut().push(w.to_vec()); Err(QueryError::NoMatch) }, |_| panic!("must not launch"));
        assert_eq!(seen.into_inner(), vec![vec!["foo\"".to_string(), "&".into(), "$(x)".into()]]);
        assert_eq!(reply, "No folder matches foo\" & $(x)");
    }
    #[test] fn run_replies() {
        let tmp = std::env::temp_dir();
        let shown = tmp.display().to_string(); // check_dir drops the trailing `\`
        let shown = shown.trim_end_matches('\\');
        let ok = |_: &[String]| Ok(format!("{}\n", tmp.display()));
        assert_eq!(run(&Cmd::Open(vec!["a".into()]), ok, |_| Ok(())), format!("Opened {shown}"));
        assert_eq!(run(&Cmd::Claude(vec!["a".into()]), ok, |_| Ok(())), format!("Started claude in {shown} (accept the folder trust prompt on the PC if it is new)"));
        assert_eq!(run(&Cmd::Open(vec!["a".into()]), ok, |_| Err(std::io::ErrorKind::NotFound.into())), "Could not open Windows Terminal");
        assert_eq!(run(&Cmd::Open(vec!["a".into()]), |_| Err(QueryError::NotInstalled), |_| Ok(())), "zoxide is not installed");
        assert_eq!(run(&Cmd::Open(vec!["a".into()]), |_| Err(QueryError::Failed), |_| Ok(())), "Could not look up a");
        assert_eq!(run(&Cmd::Open(vec!["a".into()]), |_| Ok("C:\\x;y\n".into()), |_| panic!()), "No folder matches a");
        assert_eq!(run(&Cmd::Help, |_| panic!(), |_| panic!()), HELP);
    }
    #[test] fn parses_three_verbs() {
        assert_eq!(parse("/open agent"), Some(Ok(Cmd::Open(vec!["agent".into()]))));
        assert_eq!(parse("  /CLAUDE@MyBot foo  bar "), Some(Ok(Cmd::Claude(vec!["foo".into(), "bar".into()]))));
        assert_eq!(parse("/help"), Some(Ok(Cmd::Help)));
        assert_eq!(parse("/away ON"), Some(Ok(Cmd::Away(true))));
        assert_eq!(parse("/away off"), Some(Ok(Cmd::Away(false))));
        assert_eq!(parse("/away"), Some(Err("Usage: /away on|off".into())));
    }
    #[test] fn other_text_is_not_a_command() {
        for t in ["/clear", "/compact now", "open agent", "", "/", "/opener x"] { assert_eq!(parse(t), None, "{t}"); }
    }
    #[test] fn bad_names_get_usage() {
        assert_eq!(parse("/open"), Some(Err("Usage: /open <name>".into())));
        assert_eq!(parse("/claude a b c d e f g h i"), Some(Err("Usage: /claude <name>".into())));
        assert_eq!(parse(&format!("/open {}", "x".repeat(101))), Some(Err("Usage: /open <name>".into())));
        assert_eq!(parse("/open a\u{7}b"), Some(Err("Usage: /open <name>".into())));
    }
    #[test] fn check_dir_accepts_existing_absolute_dir() {
        let d = std::env::temp_dir();
        assert_eq!(check_dir(&format!("{}\r\n", d.display())), Some(d.clone()));
        assert_eq!(check_dir(&format!("{}\\\n", d.display())), Some(d));
    }
    #[test] fn check_dir_accepts_drive_root() {
        assert_eq!(check_dir("C:\\\n"), Some(std::path::PathBuf::from("C:\\")));
    }
    #[test] fn check_dir_rejects_wt_separators() {
        for s in ["C:\\a;b\n", "C:\\a\"b\n", "", "relative\\x\n", "C:\\definitely\\missing\\ai\n"] { assert_eq!(check_dir(s), None, "{s}"); }
    }
    #[test] fn wt_args_are_fixed() {
        let d = std::path::Path::new("D:\\p");
        assert_eq!(wt_args(&Cmd::Open(vec![]), d), ["-w", "0", "nt", "-d", "D:\\p"].map(OsString::from).to_vec());
        assert_eq!(wt_args(&Cmd::Claude(vec![]), d), ["-w", "0", "nt", "-d", "D:\\p", "powershell.exe", "-NoLogo", "-NoExit", "-Command", "claude"].map(OsString::from).to_vec());
    }
}
