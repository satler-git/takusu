use crossterm::event::{KeyCode, KeyEvent};
use takusu_storage::{HabitRow, TaskRow};
use takusu_util::{Abandonability, Quantity, TaskStatus};

use crate::app::{App, Modal};

pub async fn handle_key(app: &mut App, key: KeyEvent, terminal: &mut ratatui::DefaultTerminal) {
    match key.code {
        KeyCode::Char('j') | KeyCode::Down => app.task_list.next(),
        KeyCode::Char('k') | KeyCode::Up => app.task_list.prev(),
        KeyCode::Char('f') => cycle_filter(app).await,
        KeyCode::Char('n') => {
            app.create_fields = vec![String::new(); 3];
            app.modal = Modal::CreateTask { field: 0 };
        }
        KeyCode::Char('s') => change_status(app).await,
        KeyCode::Char('d') if app.selected_task().is_some() => {
            app.modal = Modal::ConfirmDelete;
        }
        KeyCode::Char('w') => work_session(app).await,
        KeyCode::Char('e') => edit_task(app, terminal).await,
        KeyCode::Char('g') => app.do_generate().await,
        KeyCode::Char('r') => app.do_reschedule().await,
        _ => {}
    }
}

async fn cycle_filter(app: &mut App) {
    let filters = [
        None,
        Some("pending"),
        Some("scheduled"),
        Some("in_progress"),
        Some("completed"),
    ];
    let current = filters
        .iter()
        .position(|f| *f == app.task_filter.as_deref());
    let next = match current {
        Some(i) => filters[(i + 1) % filters.len()],
        None => filters[0],
    };
    app.task_filter = next.map(|s| s.to_string());
    app.task_list.index = 0;
    app.task_list.scroll = 0;
    app.reload_tasks().await;
}

async fn change_status(app: &mut App) {
    let task = match app.selected_task() {
        Some(t) => t.clone(),
        None => return,
    };
    let next = match task.status {
        TaskStatus::Pending => TaskStatus::Scheduled,
        TaskStatus::Scheduled => TaskStatus::InProgress,
        TaskStatus::InProgress => TaskStatus::Completed,
        TaskStatus::Completed => TaskStatus::Skipped,
        _ => TaskStatus::Pending,
    };
    let body = takusu_storage::UpdateTask {
        status: Some(next),
        ..Default::default()
    };
    match app.app.update_task(&task.id, &body).await {
        Ok(_) => {
            app.status_msg = Some(format!("#{} → {next}", task.display_id));
            app.reload_tasks().await;
        }
        Err(e) => app.status_msg = Some(format!("Error: {e}")),
    }
}

async fn work_session(app: &mut App) {
    let task = match app.selected_task() {
        Some(t) => t.clone(),
        None => return,
    };
    let result = match task.status {
        TaskStatus::InProgress => app.app.pause_task_work(&task.id, None).await,
        _ => app.app.start_task_work(&task.id, None).await,
    };
    match result {
        Ok(t) => {
            app.status_msg = Some(format!("#{} → {}", t.display_id, t.status));
            app.reload_tasks().await;
        }
        Err(e) => app.status_msg = Some(format!("Error: {e}")),
    }
}

async fn edit_task(app: &mut App, terminal: &mut ratatui::DefaultTerminal) {
    let task = match app.selected_task() {
        Some(t) => t.clone(),
        None => return,
    };

    let content = format_edit_text(&task, &app.all_tasks, &app.habits, &app.tz);

    ratatui::restore();
    let edited = open_editor(&content);
    *terminal = ratatui::init();

    let edited = match edited {
        Ok(e) => e,
        Err(e) => {
            app.status_msg = Some(format!("Editor error: {e}"));
            return;
        }
    };

    let update = match parse_edit_text(&edited, &app.tz) {
        Ok(u) => u,
        Err(e) => {
            app.status_msg = Some(format!("Parse error: {e}"));
            return;
        }
    };

    match app.app.update_task(&task.id, &update).await {
        Ok(t) => {
            app.status_msg = Some(format!("Updated #{}", t.display_id));
            app.reload_tasks().await;
            app.reload_schedule().await;
        }
        Err(e) => app.status_msg = Some(format!("Error: {e}")),
    }
}

fn format_edit_text(
    task: &TaskRow,
    all_tasks: &[TaskRow],
    habits: &[HabitRow],
    tz: &jiff::tz::TimeZone,
) -> String {
    let depends = format_task_depends(task, all_tasks, habits);
    format!(
        "# Edit task. Lines starting with '#' are comments.
# Empty fields are not updated; '-' clears description, start_at,
# quantity_unit, quantity_total and depends (use '0' for quantity_done).
title: {title}
description: {desc}
start_at: {start}
end_at: {end}
status: {status}
avg_minutes: {avg}
sigma_minutes: {sigma}
abandonability: {abandon}
fixed: {fixed}
depends: {depends}
quantity_total: {total}
quantity_done: {done}
quantity_unit: {unit}
parallelizable: {parallel}
allows_parallel: {allows}",
        title = task.title,
        desc = task.description.as_deref().unwrap_or(""),
        start = task
            .start_at
            .as_ref()
            .map(|t| (*t).to_zoned(tz.clone()).to_string())
            .unwrap_or_default(),
        end = task.end_at.to_zoned(tz.clone()),
        status = task.status,
        avg = task.avg_minutes,
        sigma = task.sigma_minutes,
        abandon = task.abandonability,
        fixed = task.fixed,
        depends = depends,
        total = task.quantity_total.map_or(String::new(), |v| v.to_string()),
        done = task.quantity_done,
        unit = task.quantity_unit.as_deref().unwrap_or(""),
        parallel = task.parallelizable,
        allows = task.allows_parallel,
    )
}

fn format_task_depends(task: &TaskRow, all_tasks: &[TaskRow], habits: &[HabitRow]) -> String {
    serde_json::from_str::<Vec<String>>(&task.depends)
        .unwrap_or_default()
        .iter()
        .map(|id| task_ref(id, all_tasks, habits).unwrap_or_else(|| "?".to_string()))
        .collect::<Vec<_>>()
        .join(", ")
}

fn task_ref(id: &str, all_tasks: &[TaskRow], habits: &[HabitRow]) -> Option<String> {
    let t = all_tasks.iter().find(|t| t.id == id)?;
    if let Some(habit_id) = t.habit_id.as_ref() {
        let habit = habits.iter().find(|h| &h.id == habit_id)?;
        Some(format!("h{}#{}", habit.display_id, t.display_id))
    } else {
        Some(format!("#{}", t.display_id))
    }
}

fn parse_edit_text(
    content: &str,
    tz: &jiff::tz::TimeZone,
) -> Result<takusu_storage::UpdateTask, String> {
    let mut update = takusu_storage::UpdateTask::default();
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (key, value) = match line.split_once(':') {
            Some(kv) => kv,
            None => continue,
        };
        let key = key.trim();
        let value = value.trim();
        if value.is_empty() {
            continue;
        }
        match key {
            "title" => update.title = Some(value.to_string()),
            "description" => update.description = parse_clear_string(value),
            "start_at" => {
                update.start_at = parse_clear_timestamp(value, tz)?;
            }
            "end_at" => {
                if value != "-" {
                    update.end_at = Some(
                        takusu_util::Timestamp::parse_with_tz(value, tz)
                            .map_err(|e| format!("invalid end_at: {e}"))?,
                    );
                }
            }
            "status" => {
                update.status = Some(value.parse::<TaskStatus>().map_err(|e| e.to_string())?)
            }
            "avg_minutes" => update.avg_minutes = parse_i64(value)?,
            "sigma_minutes" => update.sigma_minutes = parse_i64(value)?,
            "abandonability" => update.abandonability = parse_f64(value)?.map(Abandonability::new),
            "fixed" => update.fixed = Some(parse_bool(value)?),
            "depends" => update.depends = parse_depends(value)?,
            "quantity_total" => {
                update.quantity_total = parse_clear_i64(value)?
                    .map(Quantity::new)
                    .transpose()
                    .map_err(|e| e.to_string())?;
            }
            "quantity_done" => {
                update.quantity_done = parse_i64(value)?
                    .map(Quantity::new)
                    .transpose()
                    .map_err(|e| e.to_string())?;
            }
            "quantity_unit" => update.quantity_unit = parse_clear_string(value),
            "parallelizable" => update.parallelizable = Some(parse_bool(value)?),
            "allows_parallel" => update.allows_parallel = Some(parse_bool(value)?),
            _ => {}
        }
    }
    Ok(update)
}

/// '-' clears the field (Some("")); any other non-empty value is kept as-is.
fn parse_clear_string(value: &str) -> Option<String> {
    Some(String::new())
        .filter(|_| value == "-")
        .or(Some(value.to_string()))
}

/// '-' clears the field (None); any other non-empty value is parsed with the
/// given timezone. Returns `Option<Option<Timestamp>>`:
/// `None` = no change (shouldn't happen since empty is skipped by caller),
/// `Some(None)` = clear, `Some(Some(ts))` = set.
fn parse_clear_timestamp(
    value: &str,
    tz: &jiff::tz::TimeZone,
) -> Result<Option<Option<takusu_util::Timestamp>>, String> {
    if value == "-" {
        Ok(Some(None))
    } else {
        let ts = takusu_util::Timestamp::parse_with_tz(value, tz)
            .map_err(|e| format!("invalid start_at: {e}"))?;
        Ok(Some(Some(ts)))
    }
}

/// '-' clears the field (Some(0)); any other non-empty value is parsed.
fn parse_clear_i64(value: &str) -> Result<Option<i64>, String> {
    if value == "-" {
        Ok(Some(0))
    } else {
        value
            .parse()
            .map(Some)
            .map_err(|_| format!("invalid integer: {value}"))
    }
}

fn parse_i64(value: &str) -> Result<Option<i64>, String> {
    if value == "-" {
        // '-' is not meaningful for required or non-nullable numeric fields.
        Ok(None)
    } else {
        value
            .parse()
            .map(Some)
            .map_err(|_| format!("invalid integer: {value}"))
    }
}

fn parse_f64(value: &str) -> Result<Option<f64>, String> {
    if value == "-" {
        Ok(None)
    } else {
        value
            .parse()
            .map(Some)
            .map_err(|_| format!("invalid float: {value}"))
    }
}

fn parse_bool(value: &str) -> Result<bool, String> {
    match value.to_lowercase().as_str() {
        "true" | "yes" | "1" => Ok(true),
        "false" | "no" | "0" => Ok(false),
        _ => Err(format!("invalid bool: {value}")),
    }
}

fn parse_depends(value: &str) -> Result<Option<Vec<String>>, String> {
    if value == "-" {
        // Clear dependencies: resolve_depends converts an empty list to '[]'.
        return Ok(Some(Vec::new()));
    }
    if value.starts_with('[') {
        serde_json::from_str(value).map_err(|e| format!("invalid depends JSON: {e}"))
    } else {
        let ids: Vec<String> = value
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        Ok(Some(ids))
    }
}

fn open_editor(content: &str) -> Result<String, String> {
    use std::io::{Read, Write};

    let editor = std::env::var("EDITOR").unwrap_or_else(|_| "vi".to_string());
    let mut tmp = tempfile::NamedTempFile::with_prefix("takusu-edit-")
        .map_err(|e| format!("failed to create temp file: {e}"))?;
    tmp.write_all(content.as_bytes())
        .map_err(|e| format!("failed to write temp file: {e}"))?;
    let path = tmp.path().to_path_buf();

    // Parse EDITOR safely, respecting quotes and spaces (e.g. "code --wait").
    let words = shell_words::split(&editor).map_err(|e| format!("invalid EDITOR: {e}"))?;
    let (program, args) = match words.as_slice() {
        [prog, rest @ ..] => (prog.as_str(), rest),
        [] => ("vi", &[][..]),
    };

    let status = std::process::Command::new(program)
        .args(args)
        .arg(&path)
        .status()
        .map_err(|e| format!("failed to launch {editor}: {e}"))?;

    if !status.success() {
        return Err("editor exited with error".to_string());
    }

    let mut result = String::new();
    std::fs::File::open(&path)
        .and_then(|mut f| f.read_to_string(&mut result))
        .map_err(|e| e.to_string())?;
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_task() -> TaskRow {
        TaskRow {
            id: "task-uuid".to_string(),
            display_id: 1,
            title: "Read book".to_string(),
            description: Some("Important".to_string()),
            start_at: Some("2025-06-15T08:00:00Z".parse().unwrap()),
            end_at: "2025-06-15T10:00:00Z".parse().unwrap(),
            avg_minutes: 30,
            sigma_minutes: 5,
            depends: serde_json::to_string(&["dep-uuid".to_string()]).unwrap(),
            parallelizable: true,
            allows_parallel: false,
            abandonability: 0.25.into(),
            status: TaskStatus::Pending,
            habit_id: None,
            ical_uid: None,
            user_edited: false,
            fixed: true,
            habit_step_id: None,
            quantity_total: Some(Quantity::new(100).unwrap()),
            quantity_done: Quantity::new(50).unwrap(),
            quantity_unit: Some("pages".to_string()),
            completed_at: None,
            split_from_task_id: None,
            original_quantity_total: None,
            actual_minutes: None,
            created_at: "2025-06-14T00:00:00Z".parse().unwrap(),
            updated_at: "2025-06-14T00:00:00Z".parse().unwrap(),
        }
    }

    fn sample_dep_task() -> TaskRow {
        TaskRow {
            id: "dep-uuid".to_string(),
            display_id: 42,
            title: "Prerequisite".to_string(),
            description: None,
            start_at: None,
            end_at: "2025-06-15T12:00:00Z".parse().unwrap(),
            avg_minutes: 15,
            sigma_minutes: 3,
            depends: "[]".to_string(),
            parallelizable: false,
            allows_parallel: false,
            abandonability: 0.5.into(),
            status: TaskStatus::Completed,
            habit_id: None,
            ical_uid: None,
            user_edited: false,
            fixed: false,
            habit_step_id: None,
            quantity_total: None,
            quantity_done: Quantity::default(),
            quantity_unit: None,
            completed_at: None,
            split_from_task_id: None,
            original_quantity_total: None,
            actual_minutes: None,
            created_at: "2025-06-14T00:00:00Z".parse().unwrap(),
            updated_at: "2025-06-14T00:00:00Z".parse().unwrap(),
        }
    }

    fn sample_habit_task() -> TaskRow {
        TaskRow {
            id: "habit-task-uuid".to_string(),
            display_id: 7,
            title: "Habit task".to_string(),
            description: None,
            start_at: None,
            end_at: "2025-06-15T12:00:00Z".parse().unwrap(),
            avg_minutes: 15,
            sigma_minutes: 3,
            depends: "[]".to_string(),
            parallelizable: false,
            allows_parallel: false,
            abandonability: 0.5.into(),
            status: TaskStatus::Pending,
            habit_id: Some("habit-uuid".to_string()),
            ical_uid: None,
            user_edited: false,
            fixed: false,
            habit_step_id: None,
            quantity_total: None,
            quantity_done: Quantity::default(),
            quantity_unit: None,
            completed_at: None,
            split_from_task_id: None,
            original_quantity_total: None,
            actual_minutes: None,
            created_at: "2025-06-14T00:00:00Z".parse().unwrap(),
            updated_at: "2025-06-14T00:00:00Z".parse().unwrap(),
        }
    }

    fn sample_habit() -> HabitRow {
        HabitRow {
            id: "habit-uuid".to_string(),
            display_id: 3,
            title: "Daily habit".to_string(),
            description: None,
            active: true,
            start_time: "08:00".parse().unwrap(),
            end_time: "09:00".parse().unwrap(),
            window_mode: takusu_util::WindowMode::Day,
            avg_minutes: 10,
            sigma_minutes: 2,
            abandonability: 0.5.into(),
            parallelizable: false,
            allows_parallel: false,
            fixed: false,
            recurrence: r#"{"freq":"daily"}"#.to_string(),
            created_at: "2025-06-14T00:00:00Z".parse().unwrap(),
            updated_at: "2025-06-14T00:00:00Z".parse().unwrap(),
        }
    }

    #[test]
    fn format_edit_text_includes_all_fields() {
        let text = format_edit_text(
            &sample_task(),
            &[sample_dep_task()],
            &[],
            &jiff::tz::TimeZone::UTC,
        );
        assert!(text.contains("title: Read book"));
        assert!(text.contains("description: Important"));
        assert!(text.contains("start_at: 2025-06-15T08:00:00+00:00"));
        assert!(text.contains("end_at: 2025-06-15T10:00:00+00:00"));
        assert!(text.contains("depends: #42"));
        assert!(!text.contains("dep-uuid"));
        assert!(text.contains("quantity_total: 100"));
        assert!(text.contains("quantity_done: 50"));
        assert!(text.contains("quantity_unit: pages"));
        assert!(text.contains("parallelizable: true"));
        assert!(text.contains("allows_parallel: false"));
    }

    #[test]
    fn format_edit_text_uses_habit_ref_for_habit_tasks() {
        let text = format_edit_text(
            &sample_task_with_dep("habit-task-uuid"),
            &[sample_habit_task()],
            &[sample_habit()],
            &jiff::tz::TimeZone::UTC,
        );
        assert!(text.contains("depends: h3#7"));
    }

    fn sample_task_with_dep(dep_id: &str) -> TaskRow {
        let mut task = sample_task();
        task.depends = serde_json::to_string(&[dep_id.to_string()]).unwrap();
        task
    }

    #[test]
    fn parse_edit_text_updates_all_fields() {
        let text = r#"title: New title
status: in_progress
avg_minutes: 45
sigma_minutes: 10
abandonability: 0.5
fixed: false
parallelizable: false
allows_parallel: true
quantity_total: 200
quantity_done: 75
quantity_unit: chapters
"#;
        let update = parse_edit_text(text, &jiff::tz::TimeZone::UTC).unwrap();
        assert_eq!(update.title, Some("New title".to_string()));
        assert_eq!(update.status, Some(TaskStatus::InProgress));
        assert_eq!(update.avg_minutes, Some(45));
        assert_eq!(update.sigma_minutes, Some(10));
        assert_eq!(update.abandonability, Some(0.5.into()));
        assert_eq!(update.fixed, Some(false));
        assert_eq!(update.parallelizable, Some(false));
        assert_eq!(update.allows_parallel, Some(true));
        assert_eq!(update.quantity_total, Some(Quantity::new(200).unwrap()));
        assert_eq!(update.quantity_done, Some(Quantity::new(75).unwrap()));
        assert_eq!(update.quantity_unit, Some("chapters".to_string()));
    }

    #[test]
    fn parse_edit_text_dash_clears_supported_fields() {
        let text = r#"description: -
start_at: -
quantity_unit: -
quantity_total: -
depends: -
sigma_minutes: -
"#;
        let update = parse_edit_text(text, &jiff::tz::TimeZone::UTC).unwrap();
        assert_eq!(update.description, Some(String::new()));
        assert_eq!(update.start_at, Some(None));
        assert_eq!(update.quantity_unit, Some(String::new()));
        assert_eq!(update.quantity_total, Some(Quantity::default()));
        assert_eq!(update.depends, Some(Vec::new()));
        // sigma_minutes is not nullable; '-' should be a no-op.
        assert_eq!(update.sigma_minutes, None);
    }

    #[test]
    fn parse_edit_text_dash_does_not_clear_required_string() {
        let text = "end_at: -\n";
        let update = parse_edit_text(text, &jiff::tz::TimeZone::UTC).unwrap();
        assert_eq!(update.end_at, None);
    }

    #[test]
    fn parse_edit_text_skips_empty_lines() {
        let text = "title: New title\ndescription:\nstatus: scheduled\n";
        let update = parse_edit_text(text, &jiff::tz::TimeZone::UTC).unwrap();
        assert_eq!(update.title, Some("New title".to_string()));
        assert_eq!(update.description, None);
        assert_eq!(update.status, Some(TaskStatus::Scheduled));
    }

    #[test]
    fn parse_edit_text_parses_comma_separated_depends() {
        let text = "depends: a, b, c\n";
        let update = parse_edit_text(text, &jiff::tz::TimeZone::UTC).unwrap();
        assert_eq!(
            update.depends,
            Some(vec!["a".to_string(), "b".to_string(), "c".to_string()])
        );
    }

    #[test]
    fn parse_edit_text_parses_json_depends() {
        let text = r#"depends: ["x", "y"]"#;
        let update = parse_edit_text(text, &jiff::tz::TimeZone::UTC).unwrap();
        assert_eq!(update.depends, Some(vec!["x".to_string(), "y".to_string()]));
    }

    #[test]
    fn parse_edit_text_rejects_invalid_number() {
        let text = "avg_minutes: not-a-number\n";
        assert!(parse_edit_text(text, &jiff::tz::TimeZone::UTC).is_err());
    }
}
