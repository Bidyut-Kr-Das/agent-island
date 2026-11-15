use crate::state::{Todo, TodoStatus};
use serde_json::Value;

#[derive(Debug, Clone, Default)]
pub struct HookEvent {
    pub name: String,
    pub session_id: String,
    pub cwd: String,
    pub transcript_path: String,
    pub tool_name: Option<String>,
    pub tool_input: Value,
    pub tool_response: Value,
    pub notification_type: Option<String>,
    pub session_title: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TodoChange {
    Replace(Vec<Todo>),
    Upsert(Todo),
    // Like Upsert but ignored when the id is unknown.
    Update(Todo),
    SetStatus { id: String, status: TodoStatus },
    Rename { id: String, subject: String },
    Remove(String),
}

pub fn parse(raw: &Value) -> Option<HookEvent> {
    let text = |k: &str| raw.get(k).and_then(Value::as_str).map(str::to_owned);
    Some(HookEvent {
        name: text("hook_event_name")?,
        session_id: text("session_id")?,
        cwd: text("cwd").unwrap_or_default(),
        transcript_path: text("transcript_path").unwrap_or_default(),
        tool_name: text("tool_name"),
        tool_input: raw.get("tool_input").cloned().unwrap_or(Value::Null),
        tool_response: raw.get("tool_response").cloned().unwrap_or(Value::Null),
        notification_type: text("notification_type"),
        session_title: text("session_title"),
    })
}

fn status(s: &str) -> Option<TodoStatus> {
    match s {
        "pending" => Some(TodoStatus::Pending),
        "in_progress" => Some(TodoStatus::InProgress),
        "completed" => Some(TodoStatus::Completed),
        _ => None,
    }
}

pub fn todo_change(ev: &HookEvent) -> Option<TodoChange> {
    let input = &ev.tool_input;
    let str_of = |v: &Value, k: &str| v.get(k).and_then(Value::as_str).map(str::to_owned);
    match ev.tool_name.as_deref()? {
        "TaskCreate" => Some(TodoChange::Upsert(Todo {
            id: str_of(&ev.tool_response["task"], "id")?,
            subject: str_of(input, "subject")?,
            status: TodoStatus::Pending,
        })),
        "TaskUpdate" => {
            let id = str_of(input, "taskId")?;
            let subject = str_of(input, "subject");
            let raw_status = str_of(input, "status");
            match (subject, raw_status) {
                (Some(subject), Some(s)) => {
                    Some(TodoChange::Update(Todo { id, subject, status: status(&s)? }))
                }
                (Some(subject), None) => Some(TodoChange::Rename { id, subject }),
                (None, Some(s)) => Some(match status(&s) {
                    Some(status) => TodoChange::SetStatus { id, status },
                    None => TodoChange::Remove(id),
                }),
                (None, None) => None,
            }
        }
        "TodoWrite" => {
            let list = input.get("todos")?.as_array()?;
            let todos = list
                .iter()
                .enumerate()
                .filter_map(|(i, t)| {
                    Some(Todo {
                        id: i.to_string(),
                        subject: str_of(t, "content")?,
                        status: status(t.get("status")?.as_str()?)?,
                    })
                })
                .collect();
            Some(TodoChange::Replace(todos))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(s: &str) -> HookEvent {
        parse(&serde_json::from_str(s).unwrap()).unwrap()
    }

    fn todo(id: &str, subject: &str, status: TodoStatus) -> Todo {
        Todo { id: id.into(), subject: subject.into(), status }
    }

    #[test]
    fn parse_reads_common_fields() {
        let ev = fixture(include_str!("../tests/fixtures/PostToolUse-TaskCreate.json"));
        assert_eq!(ev.name, "PostToolUse");
        assert_eq!(ev.session_id, "2035275b-3b95-434c-af6f-b77c73649800");
        assert_eq!(ev.cwd, r"D:\projects\agent-islands\spike\sandbox");
        assert_eq!(ev.transcript_path, r"C:\Users\USER\.claude\projects\PROJ\SESSION.jsonl");
        assert_eq!(ev.tool_name.as_deref(), Some("TaskCreate"));
        assert_eq!(ev.tool_input["subject"], "Task A");
        assert_eq!(ev.tool_response["task"]["id"], "1");
        let n = fixture(include_str!("../tests/fixtures/Notification-permission_prompt.json"));
        assert_eq!(n.notification_type.as_deref(), Some("permission_prompt"));
    }

    #[test]
    fn parse_rejects_payload_without_event_name() {
        assert!(parse(&serde_json::json!({"session_id": "x"})).is_none());
    }

    #[test]
    fn todo_change_from_task_create() {
        let ev = fixture(include_str!("../tests/fixtures/PostToolUse-TaskCreate.json"));
        assert_eq!(todo_change(&ev), Some(TodoChange::Upsert(todo("1", "Task A", TodoStatus::Pending))));
    }

    #[test]
    fn todo_change_from_task_update() {
        let ev = fixture(include_str!("../tests/fixtures/PostToolUse-TaskUpdate.json"));
        assert_eq!(
            todo_change(&ev),
            Some(TodoChange::SetStatus { id: "1".into(), status: TodoStatus::InProgress })
        );
    }

    #[test]
    fn task_update_deleted_removes_and_subject_renames() {
        let mut ev = fixture(include_str!("../tests/fixtures/PostToolUse-TaskUpdate.json"));
        ev.tool_input = serde_json::json!({"taskId": "1", "status": "deleted"});
        assert_eq!(todo_change(&ev), Some(TodoChange::Remove("1".into())));
        ev.tool_input = serde_json::json!({"taskId": "1", "subject": "New"});
        assert_eq!(todo_change(&ev), Some(TodoChange::Rename { id: "1".into(), subject: "New".into() }));
        ev.tool_input = serde_json::json!({"taskId": "1", "subject": "New", "status": "completed"});
        assert_eq!(
            todo_change(&ev),
            Some(TodoChange::Update(todo("1", "New", TodoStatus::Completed)))
        );
    }

    #[test]
    fn todo_change_from_todo_write_keeps_order_and_statuses() {
        let ev = fixture(include_str!("../tests/fixtures/PostToolUse-TodoWrite.json"));
        let Some(TodoChange::Replace(list)) = todo_change(&ev) else { panic!("expected Replace") };
        assert_eq!(list[0], todo("0", "A", TodoStatus::InProgress));
        assert_eq!(list[1].id, "1");
        assert_eq!(list[1].status, TodoStatus::Pending);
        assert!(list.len() >= 2);
    }

    #[test]
    fn todo_write_skips_unknown_status() {
        let mut ev = fixture(include_str!("../tests/fixtures/PostToolUse-TodoWrite.json"));
        ev.tool_input = serde_json::json!({"todos": [
            {"content": "A", "status": "deleted"}, {"content": "B", "status": "completed"}]});
        let Some(TodoChange::Replace(list)) = todo_change(&ev) else { panic!() };
        assert_eq!(list, vec![todo("1", "B", TodoStatus::Completed)]);
    }

    #[test]
    fn todo_change_none_for_other_tools() {
        let ev = fixture(include_str!("../tests/fixtures/PostToolUse.json"));
        assert_eq!(todo_change(&ev), None);
    }
}
