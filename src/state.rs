use std::collections::VecDeque;

use crate::event::{HookEvent, TodoChange};

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TodoStatus {
    Pending,
    InProgress,
    Completed,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Todo {
    pub id: String,
    pub subject: String,
    pub status: TodoStatus,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum WaitKind {
    Permission,
    Question,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Status {
    Idle,
    Working,
    Waiting(WaitKind),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Finish {
    Ok,
    Error,
}

#[derive(Debug, Clone)]
pub struct Session {
    pub id: String,
    pub project: String,
    pub cwd: String,
    pub transcript: String,
    pub color: usize,
    pub status: Status,
    pub todos: Vec<Todo>,
    pub finished: Option<Finish>,
    pub title: Option<String>,
    pub claude_pid: Option<u32>,
    pub terminal_pid: Option<u32>,
    pub changed_at: u64,
}

#[derive(Debug, Clone)]
pub struct Permission {
    pub req_id: u64,
    pub session_id: String,
    pub tool: String,
    pub verb: String,
    pub detail: String,
    pub full: String,
    pub timed_out: bool,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct Pids {
    pub claude: Option<u32>,
    pub terminal: Option<u32>,
}

#[derive(Debug, Default)]
pub struct State {
    pub sessions: Vec<Session>,
    pub permissions: VecDeque<Permission>,
    next_req: u64,
}

fn project_name(cwd: &str) -> String {
    let parts = [char::from(92), '/'];
    cwd.trim_end_matches(parts).rsplit(parts).next().unwrap_or_default().to_owned()
}

fn apply_todo(todos: &mut Vec<Todo>, change: TodoChange) {
    match change {
        TodoChange::Replace(list) => *todos = list,
        TodoChange::Upsert(t) => match todos.iter_mut().find(|x| x.id == t.id) {
            Some(x) => *x = t,
            None => todos.push(t),
        },
        TodoChange::Update(t) => {
            if let Some(x) = todos.iter_mut().find(|x| x.id == t.id) {
                *x = t;
            }
        }
        TodoChange::SetStatus { id, status } => {
            if let Some(x) = todos.iter_mut().find(|x| x.id == id) {
                x.status = status;
            }
        }
        TodoChange::Rename { id, subject } => {
            if let Some(x) = todos.iter_mut().find(|x| x.id == id) {
                x.subject = subject;
            }
        }
        TodoChange::Remove(id) => todos.retain(|x| x.id != id),
    }
}

/// A pid-less session silent this long (ms) is treated as gone.
const STALE_MS: u64 = 30 * 60 * 1000;

impl State {
    pub fn apply(&mut self, ev: &HookEvent, pids: Pids, now: u64) -> Option<u64> {
        let sid = ev.session_id.as_str();
        if ev.name == "SessionEnd" {
            self.sessions.retain(|s| s.id != sid);
            self.permissions.retain(|p| p.session_id != sid);
            return None;
        }
        let idx = match self.sessions.iter().position(|s| s.id == sid) {
            Some(i) => i,
            None => {
                let color = (0..6)
                    .find(|c| !self.sessions.iter().any(|s| s.color == *c))
                    .unwrap_or(self.sessions.len() % 6);
                self.sessions.push(Session {
                    id: sid.to_owned(),
                    project: project_name(&ev.cwd),
                    cwd: ev.cwd.clone(),
                    transcript: ev.transcript_path.clone(),
                    color,
                    status: Status::Idle,
                    todos: Vec::new(),
                    finished: None,
                    title: None,
                    claude_pid: None,
                    terminal_pid: None,
                    changed_at: now,
                });
                self.sessions.len() - 1
            }
        };
        let has_card = self.permissions.iter().any(|p| p.session_id == sid);
        let has_pending = self.permissions.iter().any(|p| p.session_id == sid && !p.timed_out);
        let s = &mut self.sessions[idx];
        s.changed_at = now;
        s.claude_pid = pids.claude.or(s.claude_pid);
        s.terminal_pid = pids.terminal.or(s.terminal_pid);
        if ev.session_title.is_some() {
            s.title = ev.session_title.clone();
        }
        let tool = ev.tool_name.as_deref().unwrap_or_default();
        let ask = tool == "AskUserQuestion";
        let mut req_id = None;
        match ev.name.as_str() {
            "SessionStart" => s.status = Status::Idle,
            "UserPromptSubmit" => {
                s.status = Status::Working;
                s.todos.clear();
                s.finished = None;
            }
            "Stop" => {
                s.status = Status::Idle;
                s.finished = Some(Finish::Ok);
            }
            "StopFailure" => {
                s.status = Status::Idle;
                s.finished = Some(Finish::Error);
            }
            "PermissionRequest" if ask => s.status = Status::Waiting(WaitKind::Question),
            "PermissionRequest" => {
                self.next_req += 1;
                let mcp = ev.mcp_server.as_deref();
                let (verb, full) = crate::detail::full_line(tool, &ev.tool_input, mcp);
                let (_, detail) = crate::detail::detail_line(tool, &ev.tool_input, mcp);
                self.permissions.push_back(Permission {
                    req_id: self.next_req,
                    session_id: sid.to_owned(),
                    tool: tool.to_owned(),
                    verb,
                    detail,
                    full,
                    timed_out: false,
                });
                s.status = Status::Waiting(WaitKind::Permission);
                req_id = Some(self.next_req);
            }
            "PreToolUse" if ask => s.status = Status::Waiting(WaitKind::Question),
            "PreToolUse" => s.status = Status::Working,
            "PostToolUse" => {
                if let Some(change) = crate::event::todo_change(ev) {
                    apply_todo(&mut s.todos, change);
                }
                s.status = Status::Working;
            }
            "Notification" => match ev.notification_type.as_deref() {
                // permission_prompt duplicates the permission card
                Some("permission_prompt" | "idle_prompt" | "agent_completed") => {}
                _ if has_card || s.status == Status::Waiting(WaitKind::Question) => {}
                _ => s.status = Status::Waiting(WaitKind::Other),
            },
            _ => {
                if matches!(s.status, Status::Waiting(WaitKind::Question | WaitKind::Other)) {
                    s.status = Status::Working;
                }
            }
        }
        if has_pending && s.status != Status::Waiting(WaitKind::Question) {
            s.status = Status::Waiting(WaitKind::Permission);
        }
        req_id
    }

    // Leaves Waiting(Permission) once no un-timed-out card remains for the session.
    fn settle(&mut self, session_id: &str) {
        let pending = self.permissions.iter().any(|p| p.session_id == session_id && !p.timed_out);
        let any = self.permissions.iter().any(|p| p.session_id == session_id);
        if let Some(s) = self.sessions.iter_mut().find(|s| s.id == session_id)
            && s.status == Status::Waiting(WaitKind::Permission)
            && !pending
        {
            s.status = if any { Status::Waiting(WaitKind::Other) } else { Status::Working };
        }
    }

    pub fn resolve(&mut self, req_id: u64) {
        if let Some(pos) = self.permissions.iter().position(|p| p.req_id == req_id) {
            let p = self.permissions.remove(pos).unwrap();
            self.settle(&p.session_id);
        }
    }

    pub fn timeout(&mut self, req_id: u64) {
        if let Some(p) = self.permissions.iter_mut().find(|p| p.req_id == req_id) {
            p.timed_out = true;
            let sid = p.session_id.clone();
            self.settle(&sid);
        }
    }

    pub fn mark_seen(&mut self, session_id: &str) {
        if let Some(s) = self.sessions.iter_mut().find(|s| s.id == session_id) {
            s.finished = None;
        }
    }

    /// Drops sessions whose claude process is gone, and sessions with no known claude pid that have been
    /// silent for STALE_MS and are not waiting on the user (`now` in ms, like `changed_at`).
    pub fn prune(&mut self, alive: impl Fn(u32) -> bool, now: u64) {
        self.sessions.retain(|s| match s.claude_pid {
            Some(pid) => alive(pid),
            None => matches!(s.status, Status::Waiting(_)) || now.saturating_sub(s.changed_at) < STALE_MS,
        });
        let sessions = &self.sessions;
        self.permissions.retain(|p| sessions.iter().any(|s| s.id == p.session_id));
    }

    pub fn is_empty(&self) -> bool {
        self.sessions.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::parse;
    use serde_json::{Value, json};

    const CWD: &str = r"D:\projects\payments-api";

    fn ev(name: &str, session: &str, extra: Value) -> HookEvent {
        let mut raw = json!({"hook_event_name": name, "session_id": session, "cwd": CWD, "transcript_path": "t.jsonl"});
        raw.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        parse(&raw).unwrap()
    }

    fn go(s: &mut State, name: &str, session: &str, extra: Value) -> Option<u64> {
        s.apply(&ev(name, session, extra), Pids::default(), 1)
    }

    fn status(s: &State, id: &str) -> Status {
        s.sessions.iter().find(|x| x.id == id).unwrap().status
    }

    fn perm(s: &mut State, session: &str, tool: &str) -> u64 {
        go(s, "PermissionRequest", session, json!({"tool_name": tool})).unwrap()
    }

    #[test]
    fn session_start_adds_idle_session() {
        let mut s = State::default();
        go(&mut s, "SessionStart", "a", json!({}));
        assert_eq!(s.sessions.len(), 1);
        assert_eq!(s.sessions[0].project, "payments-api");
        assert_eq!(s.sessions[0].status, Status::Idle);
        assert_eq!(s.sessions[0].transcript, "t.jsonl");
    }

    #[test]
    fn project_name_tolerates_trailing_separator_and_forward_slashes() {
        let mut s = State::default();
        go(&mut s, "SessionStart", "a", json!({"cwd": r"D:\projects\x-app\"}));
        go(&mut s, "SessionStart", "b", json!({"cwd": "/home/me/y-app"}));
        assert_eq!(s.sessions[0].project, "x-app");
        assert_eq!(s.sessions[1].project, "y-app");
    }

    #[test]
    fn unknown_session_created_on_any_event() {
        let mut s = State::default();
        go(&mut s, "PreToolUse", "a", json!({"tool_name": "Bash"}));
        assert_eq!(s.sessions.len(), 1);
        assert_eq!(status(&s, "a"), Status::Working);
    }

    #[test]
    fn prompt_sets_working_and_clears_todos_and_finished() {
        let mut s = State::default();
        go(&mut s, "PostToolUse", "a", json!({"tool_name": "TaskCreate",
            "tool_input": {"subject": "X"}, "tool_response": {"task": {"id": "1"}}}));
        go(&mut s, "Stop", "a", json!({}));
        assert_eq!(s.sessions[0].todos.len(), 1);
        assert_eq!(s.sessions[0].finished, Some(Finish::Ok));
        go(&mut s, "UserPromptSubmit", "a", json!({}));
        assert_eq!(s.sessions[0].status, Status::Working);
        assert!(s.sessions[0].todos.is_empty());
        assert_eq!(s.sessions[0].finished, None);
    }

    #[test]
    fn stop_sets_idle_and_finished_ok() {
        let mut s = State::default();
        go(&mut s, "UserPromptSubmit", "a", json!({}));
        go(&mut s, "Stop", "a", json!({}));
        assert_eq!(s.sessions[0].status, Status::Idle);
        assert_eq!(s.sessions[0].finished, Some(Finish::Ok));
    }

    #[test]
    fn stop_failure_sets_finished_error() {
        let mut s = State::default();
        go(&mut s, "StopFailure", "a", json!({}));
        assert_eq!(s.sessions[0].status, Status::Idle);
        assert_eq!(s.sessions[0].finished, Some(Finish::Error));
    }

    #[test]
    fn session_end_removes_session_and_its_permissions() {
        let mut s = State::default();
        perm(&mut s, "a", "Bash");
        perm(&mut s, "b", "Bash");
        go(&mut s, "SessionEnd", "a", json!({}));
        assert_eq!(s.sessions.len(), 1);
        assert_eq!(s.permissions.len(), 1);
        assert_eq!(s.permissions[0].session_id, "b");
    }

    #[test]
    fn permission_request_sets_waiting_and_returns_req_id() {
        let mut s = State::default();
        let id = perm(&mut s, "a", "Bash");
        assert_eq!(status(&s, "a"), Status::Waiting(WaitKind::Permission));
        assert_eq!(s.permissions[0].req_id, id);
        assert_eq!(s.permissions[0].tool, "Bash");
        assert!(!s.permissions[0].timed_out);
    }

    #[test]
    fn permission_request_fills_verb_detail_and_full() {
        let raw = serde_json::from_str(include_str!("../tests/fixtures/PermissionRequest-PowerShell.json")).unwrap();
        let mut s = State::default();
        s.apply(&parse(&raw).unwrap(), Pids::default(), 1);
        let p = &s.permissions[0];
        assert_eq!((p.verb.as_str(), p.detail.as_str(), p.full.as_str()), ("Wants to run a command", "mkdir probe1", "mkdir probe1"));
    }

    #[test]
    fn permission_request_for_ask_user_question_is_a_question() {
        let mut s = State::default();
        let r = go(&mut s, "PermissionRequest", "a", json!({"tool_name": "AskUserQuestion"}));
        assert_eq!(r, None);
        assert!(s.permissions.is_empty());
        assert_eq!(status(&s, "a"), Status::Waiting(WaitKind::Question));
    }

    #[test]
    fn two_permissions_same_session_queue_in_order() {
        let mut s = State::default();
        let first = perm(&mut s, "a", "Bash");
        let second = perm(&mut s, "a", "Write");
        assert!(second > first);
        assert_eq!(s.permissions.front().unwrap().req_id, first);
        s.resolve(first);
        assert_eq!(s.permissions.len(), 1);
        assert_eq!(s.permissions[0].req_id, second);
        assert_eq!(status(&s, "a"), Status::Waiting(WaitKind::Permission));
        s.resolve(second);
        assert_eq!(status(&s, "a"), Status::Working);
    }

    #[test]
    fn timeout_marks_permission_timed_out_and_wait_other() {
        let mut s = State::default();
        let id = perm(&mut s, "a", "Bash");
        s.timeout(id);
        assert!(s.permissions[0].timed_out);
        assert_eq!(status(&s, "a"), Status::Waiting(WaitKind::Other));
    }

    #[test]
    fn pretooluse_ask_user_question_sets_waiting_question() {
        let mut s = State::default();
        go(&mut s, "PreToolUse", "a", json!({"tool_name": "AskUserQuestion"}));
        assert_eq!(status(&s, "a"), Status::Waiting(WaitKind::Question));
    }

    #[test]
    fn any_later_event_clears_waiting() {
        let mut s = State::default();
        go(&mut s, "PreToolUse", "a", json!({"tool_name": "AskUserQuestion"}));
        go(&mut s, "PostToolUse", "a", json!({"tool_name": "AskUserQuestion"}));
        assert_eq!(status(&s, "a"), Status::Working);
        go(&mut s, "Notification", "a", json!({"notification_type": "needs_input"}));
        assert_eq!(status(&s, "a"), Status::Waiting(WaitKind::Other));
        go(&mut s, "Stop", "a", json!({}));
        assert_eq!(status(&s, "a"), Status::Idle);
    }

    #[test]
    fn pending_permission_survives_other_events() {
        let mut s = State::default();
        perm(&mut s, "a", "Bash");
        go(&mut s, "PostToolUse", "a", json!({"tool_name": "Read"}));
        assert_eq!(s.permissions.len(), 1);
        assert_eq!(status(&s, "a"), Status::Waiting(WaitKind::Permission));
    }

    #[test]
    fn notification_needs_input_sets_waiting_other_unless_card_open() {
        let mut s = State::default();
        go(&mut s, "Notification", "a", json!({"notification_type": "needs_input"}));
        assert_eq!(status(&s, "a"), Status::Waiting(WaitKind::Other));
        perm(&mut s, "b", "Bash");
        go(&mut s, "Notification", "b", json!({"notification_type": "needs_input"}));
        assert_eq!(status(&s, "b"), Status::Waiting(WaitKind::Permission));
        go(&mut s, "PreToolUse", "c", json!({"tool_name": "AskUserQuestion"}));
        go(&mut s, "Notification", "c", json!({"notification_type": "needs_input"}));
        assert_eq!(status(&s, "c"), Status::Waiting(WaitKind::Question));
    }

    #[test]
    fn notification_idle_permission_and_agent_completed_ignored() {
        let mut s = State::default();
        go(&mut s, "UserPromptSubmit", "a", json!({}));
        for t in ["idle_prompt", "permission_prompt", "agent_completed"] {
            go(&mut s, "Notification", "a", json!({"notification_type": t}));
            assert_eq!(status(&s, "a"), Status::Working, "{t}");
        }
    }

    #[test]
    fn todos_apply_on_post_tool_use() {
        let mut s = State::default();
        go(&mut s, "PostToolUse", "a", json!({"tool_name": "TaskCreate",
            "tool_input": {"subject": "X"}, "tool_response": {"task": {"id": "1"}}}));
        go(&mut s, "PostToolUse", "a", json!({"tool_name": "TaskCreate",
            "tool_input": {"subject": "Y"}, "tool_response": {"task": {"id": "2"}}}));
        go(&mut s, "PostToolUse", "a", json!({"tool_name": "TaskUpdate",
            "tool_input": {"taskId": "2", "status": "completed"}}));
        go(&mut s, "PostToolUse", "a", json!({"tool_name": "TaskUpdate",
            "tool_input": {"taskId": "9", "status": "completed"}}));
        let t = &s.sessions[0].todos;
        assert_eq!(t.len(), 2);
        assert_eq!(t[1].status, TodoStatus::Completed);
        go(&mut s, "PostToolUse", "a", json!({"tool_name": "TaskUpdate",
            "tool_input": {"taskId": "1", "status": "deleted"}}));
        go(&mut s, "PostToolUse", "a", json!({"tool_name": "TaskUpdate",
            "tool_input": {"taskId": "2", "subject": "Z"}}));
        assert_eq!(s.sessions[0].todos.len(), 1);
        assert_eq!(s.sessions[0].todos[0].subject, "Z");
        go(&mut s, "PostToolUse", "a", json!({"tool_name": "TodoWrite",
            "tool_input": {"todos": [{"content": "A", "status": "pending"}]}}));
        assert_eq!(s.sessions[0].todos[0].subject, "A");
        assert_eq!(s.sessions[0].todos.len(), 1);
    }

    #[test]
    fn task_update_with_subject_for_unknown_id_is_ignored() {
        let mut s = State::default();
        go(&mut s, "PostToolUse", "a", json!({"tool_name": "TaskUpdate",
            "tool_input": {"taskId": "9", "subject": "Ghost", "status": "completed"}}));
        assert!(s.sessions[0].todos.is_empty());
        go(&mut s, "PostToolUse", "a", json!({"tool_name": "TaskCreate",
            "tool_input": {"subject": "X"}, "tool_response": {"task": {"id": "1"}}}));
        go(&mut s, "PostToolUse", "a", json!({"tool_name": "TaskUpdate",
            "tool_input": {"taskId": "1", "subject": "Y", "status": "completed"}}));
        assert_eq!(s.sessions[0].todos, vec![Todo { id: "1".into(), subject: "Y".into(), status: TodoStatus::Completed }]);
    }

    #[test]
    fn seventh_session_reuses_first_color() {
        let mut s = State::default();
        for i in 0..7 {
            go(&mut s, "SessionStart", &i.to_string(), json!({}));
        }
        let colors: Vec<_> = s.sessions.iter().map(|x| x.color).collect();
        assert_eq!(colors, vec![0, 1, 2, 3, 4, 5, 0]);
    }

    #[test]
    fn freed_color_is_reused() {
        let mut s = State::default();
        for i in 0..3 {
            go(&mut s, "SessionStart", &i.to_string(), json!({}));
        }
        go(&mut s, "SessionEnd", "1", json!({}));
        go(&mut s, "SessionStart", "new", json!({}));
        assert_eq!(s.sessions.last().unwrap().color, 1);
    }

    #[test]
    fn cwd_pinned_from_first_event() {
        let mut s = State::default();
        go(&mut s, "SessionStart", "a", json!({}));
        go(&mut s, "PreToolUse", "a", json!({"cwd": r"D:\elsewhere", "tool_name": "Bash"}));
        assert_eq!(s.sessions[0].cwd, CWD);
        assert_eq!(s.sessions[0].project, "payments-api");
    }

    #[test]
    fn empty_chain_keeps_known_pids() {
        let mut s = State::default();
        s.apply(&ev("SessionStart", "a", json!({})), Pids { claude: Some(7), terminal: Some(8) }, 1);
        s.apply(&ev("PostToolUse", "a", json!({})), crate::procs::find_pids(&[]), 2);
        assert_eq!((s.sessions[0].claude_pid, s.sessions[0].terminal_pid), (Some(7), Some(8)));
    }

    #[test]
    fn changed_at_and_pids_and_title_update_per_event() {
        let mut s = State::default();
        s.apply(&ev("SessionStart", "a", json!({})), Pids { claude: Some(7), terminal: Some(8) }, 5);
        s.apply(&ev("Stop", "a", json!({"session_title": "T"})), Pids::default(), 9);
        let x = &s.sessions[0];
        assert_eq!((x.changed_at, x.claude_pid, x.terminal_pid), (9, Some(7), Some(8)));
        assert_eq!(x.title.as_deref(), Some("T"));
    }

    #[test]
    fn mark_seen_clears_finished() {
        let mut s = State::default();
        go(&mut s, "Stop", "a", json!({}));
        s.mark_seen("a");
        assert_eq!(s.sessions[0].finished, None);
    }

    #[test]
    fn prune_removes_dead_claude_pid() {
        let mut s = State::default();
        s.apply(&ev("SessionStart", "a", json!({})), Pids { claude: Some(1), terminal: None }, 1);
        s.apply(&ev("SessionStart", "b", json!({})), Pids { claude: Some(2), terminal: None }, 1);
        go(&mut s, "SessionStart", "c", json!({}));
        perm(&mut s, "a", "Bash");
        s.prune(|pid| pid == 2, 1);
        let ids: Vec<_> = s.sessions.iter().map(|x| x.id.as_str()).collect();
        assert_eq!(ids, vec!["b", "c"]);
        assert!(s.permissions.is_empty());
    }

    #[test]
    fn prune_drops_stale_pidless_sessions_unless_waiting() {
        let mut s = State::default();
        go(&mut s, "SessionStart", "stale", json!({}));
        perm(&mut s, "waiting", "Bash");
        let now = 1 + 30 * 60 * 1000;
        s.apply(&ev("PostToolUse", "fresh", json!({})), Pids::default(), now - 1000);
        s.prune(|_| true, now);
        let ids: Vec<_> = s.sessions.iter().map(|x| x.id.as_str()).collect();
        assert_eq!(ids, vec!["waiting", "fresh"]);
    }

    #[test]
    fn is_empty_true_only_with_no_sessions() {
        let mut s = State::default();
        assert!(s.is_empty());
        go(&mut s, "SessionStart", "a", json!({}));
        assert!(!s.is_empty());
        go(&mut s, "SessionEnd", "a", json!({}));
        assert!(s.is_empty());
    }
}
