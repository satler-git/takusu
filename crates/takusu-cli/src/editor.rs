use std::collections::HashMap;
use std::env;
use std::fs;
use std::io;
use std::process::Command;
use takusu_storage::{HabitRow, HabitStepInput, HabitStepRow, TaskRow, UpdateHabit, UpdateTask};
use takusu_types::{TaskStatus, TimeOfDay, Timestamp, WindowMode, parse_datetime_to_timestamp};

use crate::task_ref::task_reference;

/// Format a `Timestamp` in the given timezone for display in the editor.
/// Returns an empty string for `None`.
fn fmt_ts(ts: Option<&Timestamp>, tz: &jiff::tz::TimeZone) -> String {
    ts.map(|t| {
        let zdt = t.to_zoned(tz.clone());
        zdt.to_string()
    })
    .unwrap_or_default()
}

pub fn format_task_for_editing(
    task: &TaskRow,
    all_tasks: &[TaskRow],
    habit_map: &HashMap<String, i64>,
    tz: &jiff::tz::TimeZone,
) -> String {
    let depends_uuids: Vec<String> = task.depends.to_vec();
    // Show display_ids when the dependency task is known, otherwise fall back to UUID.
    let depends: String = depends_uuids
        .iter()
        .map(|uuid| {
            all_tasks
                .iter()
                .find(|t| &t.id == uuid)
                .map(|t| task_reference(t, habit_map))
                .unwrap_or_else(|| uuid.clone())
        })
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        r#"# Edit task. Lines starting with '#' are comments.
# Empty fields will not be updated. Save and quit to apply changes.
# depends: comma-separated display IDs (e.g. #3, h1#5, #17) or full UUIDs
title: {title}
description: {desc}
start_at: {start}
end_at: {end}
status: {status}
avg_minutes: {avg}
sigma_minutes: {sigma}
parallelizable: {par}
allows_parallel: {apar}
abandonability: {abandon}
fixed: {fixed}
quantity_total: {qtotal}
quantity_done: {qdone}
quantity_unit: {qunit}
original_quantity_total: {oqtotal}
depends: {depends}"#,
        title = task.title,
        desc = task.description.as_deref().unwrap_or(""),
        start = fmt_ts(task.start_at.as_ref(), tz),
        end = fmt_ts(Some(&task.end_at), tz),
        status = task.status,
        avg = task.avg_minutes,
        sigma = task.sigma_minutes,
        par = task.parallelizable,
        apar = task.allows_parallel,
        abandon = task.abandonability,
        fixed = task.fixed,
        qtotal = task
            .quantity_total
            .map(|n| n.to_string())
            .unwrap_or_default(),
        qdone = task.quantity_done,
        qunit = task.quantity_unit.as_deref().unwrap_or(""),
        oqtotal = task
            .original_quantity_total
            .map(|n| n.to_string())
            .unwrap_or_default(),
    )
}

pub fn parse_edited_task(content: &str, tz: &jiff::tz::TimeZone) -> Result<UpdateTask, String> {
    let mut title = None;
    let mut description = None;
    let mut start_at = None;
    let mut end_at = None;
    let mut status = None;
    let mut avg_minutes = None;
    let mut sigma_minutes = None;
    let mut parallelizable = None;
    let mut allows_parallel = None;
    let mut abandonability = None;
    let mut fixed = None;
    let mut quantity_total = None;
    let mut quantity_done = None;
    let mut quantity_unit = None;
    let mut original_quantity_total = None;
    let mut depends = None;

    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (key, value) = match line.split_once(':') {
            Some(kv) => kv,
            None => {
                eprintln!("warning: skipping malformed line (no ':'): {line}");
                continue;
            }
        };
        let key = key.trim();
        let value = value.trim();
        match key {
            "title" => title = Some(value.to_string()),
            "description" => {
                description = if value.is_empty() {
                    Some(None)
                } else {
                    Some(Some(value.to_string()))
                }
            }
            "start_at" => {
                start_at = if value.is_empty() {
                    None
                } else {
                    Some(Some(
                        parse_datetime_to_timestamp(value, tz)
                            .map(Timestamp::from)
                            .map_err(|e| format!("invalid start_at '{value}': {e}"))?,
                    ))
                }
            }
            "end_at" => {
                end_at = if value.is_empty() {
                    None
                } else {
                    Some(
                        parse_datetime_to_timestamp(value, tz)
                            .map(Timestamp::from)
                            .map_err(|e| format!("invalid end_at '{value}': {e}"))?,
                    )
                }
            }
            "status" => {
                status = Some(
                    value
                        .parse::<TaskStatus>()
                        .map_err(|e| format!("invalid status '{value}': {e}"))?,
                )
            }
            "avg_minutes" => {
                avg_minutes = Some(
                    value
                        .parse()
                        .map_err(|e| format!("invalid avg_minutes '{value}': {e}"))?,
                )
            }
            "sigma_minutes" => {
                sigma_minutes = Some(
                    value
                        .parse()
                        .map_err(|e| format!("invalid sigma_minutes '{value}': {e}"))?,
                )
            }
            "parallelizable" => {
                parallelizable = Some(
                    value
                        .parse()
                        .map_err(|e| format!("invalid parallelizable '{value}': {e}"))?,
                )
            }
            "allows_parallel" => {
                allows_parallel = Some(
                    value
                        .parse()
                        .map_err(|e| format!("invalid allows_parallel '{value}': {e}"))?,
                )
            }
            "abandonability" => {
                abandonability = Some(
                    value
                        .parse()
                        .map_err(|e| format!("invalid abandonability '{value}': {e}"))?,
                )
            }
            "fixed" => {
                fixed = Some(
                    value
                        .parse()
                        .map_err(|e| format!("invalid fixed '{value}': {e}"))?,
                )
            }
            "quantity_total" => {
                quantity_total = if value.is_empty() {
                    None
                } else {
                    Some(
                        value
                            .parse()
                            .map_err(|e| format!("invalid quantity_total '{value}': {e}"))?,
                    )
                }
            }
            "quantity_done" => {
                quantity_done = if value.is_empty() {
                    None
                } else {
                    Some(
                        value
                            .parse()
                            .map_err(|e| format!("invalid quantity_done '{value}': {e}"))?,
                    )
                }
            }
            "quantity_unit" => {
                quantity_unit = if value.is_empty() {
                    Some(None)
                } else {
                    Some(Some(value.to_string()))
                }
            }
            "original_quantity_total" => {
                original_quantity_total =
                    if value.is_empty() {
                        None
                    } else {
                        Some(value.parse().map_err(|e| {
                            format!("invalid original_quantity_total '{value}': {e}")
                        })?)
                    }
            }
            "depends" => {
                let items: Vec<String> = if value.is_empty() {
                    vec![]
                } else {
                    value
                        .split(',')
                        .map(|s| s.trim().trim_start_matches('#').to_string())
                        .filter(|s| !s.is_empty())
                        .collect()
                };
                depends = Some(items);
            }
            _ => {}
        }
    }

    Ok(UpdateTask {
        title,
        description: description.flatten(),
        start_at,
        end_at,
        avg_minutes,
        sigma_minutes,
        depends,
        parallelizable,
        allows_parallel,
        abandonability,
        status,
        habit_id: None,
        user_edited: None,
        fixed,
        habit_step_id: None,
        quantity_total,
        quantity_done,
        quantity_unit: quantity_unit.flatten(),
        original_quantity_total,
    })
}

pub fn format_habit_for_editing(habit: &HabitRow) -> String {
    format!(
        r#"# Edit habit. Lines starting with '#' are comments.
# Empty fields will not be updated. Save and quit to apply changes.
# window_mode: 'day' (occurrence day) or 'period' (until next occurrence)
title: {title}
description: {desc}
recurrence: {recurrence}
start_time: {start}
end_time: {end}
avg_minutes: {avg}
sigma_minutes: {sigma}
parallelizable: {par}
allows_parallel: {apar}
abandonability: {abandon}
fixed: {fixed}
active: {active}
window_mode: {window_mode}"#,
        title = habit.title,
        desc = habit.description.as_deref().unwrap_or(""),
        recurrence = habit.recurrence,
        start = habit.start_time,
        end = habit.end_time,
        avg = habit.avg_minutes,
        sigma = habit.sigma_minutes,
        par = habit.parallelizable,
        apar = habit.allows_parallel,
        abandon = habit.abandonability,
        fixed = habit.fixed,
        active = habit.active,
        window_mode = habit.window_mode,
    )
}

pub fn parse_edited_habit(content: &str) -> Result<UpdateHabit, String> {
    let mut title = None;
    let mut description = None;
    let mut recurrence = None;
    let mut start_time = None;
    let mut end_time = None;
    let mut avg_minutes = None;
    let mut sigma_minutes = None;
    let mut parallelizable = None;
    let mut allows_parallel = None;
    let mut abandonability = None;
    let mut fixed = None;
    let mut active = None;
    let mut window_mode = None;

    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (key, value) = match line.split_once(':') {
            Some(kv) => kv,
            None => {
                eprintln!("warning: skipping malformed line (no ':'): {line}");
                continue;
            }
        };
        let key = key.trim();
        let value = value.trim();
        match key {
            "title" => title = Some(value.to_string()),
            "description" => {
                description = if value.is_empty() {
                    Some(None)
                } else {
                    Some(Some(value.to_string()))
                }
            }
            "recurrence" => recurrence = Some(value.to_string()),
            "start_time" => {
                start_time = if value.is_empty() {
                    Some(None)
                } else {
                    Some(Some(
                        value
                            .parse::<TimeOfDay>()
                            .map_err(|e| format!("invalid start_time '{value}': {e}"))?,
                    ))
                }
            }
            "end_time" => {
                end_time = if value.is_empty() {
                    Some(None)
                } else {
                    Some(Some(
                        value
                            .parse::<TimeOfDay>()
                            .map_err(|e| format!("invalid end_time '{value}': {e}"))?,
                    ))
                }
            }
            "avg_minutes" => {
                avg_minutes = Some(
                    value
                        .parse()
                        .map_err(|e| format!("invalid avg_minutes '{value}': {e}"))?,
                )
            }
            "sigma_minutes" => {
                sigma_minutes = Some(
                    value
                        .parse()
                        .map_err(|e| format!("invalid sigma_minutes '{value}': {e}"))?,
                )
            }
            "parallelizable" => {
                parallelizable = Some(
                    value
                        .parse()
                        .map_err(|e| format!("invalid parallelizable '{value}': {e}"))?,
                )
            }
            "allows_parallel" => {
                allows_parallel = Some(
                    value
                        .parse()
                        .map_err(|e| format!("invalid allows_parallel '{value}': {e}"))?,
                )
            }
            "abandonability" => {
                abandonability = Some(
                    value
                        .parse()
                        .map_err(|e| format!("invalid abandonability '{value}': {e}"))?,
                )
            }
            "fixed" => {
                fixed = Some(
                    value
                        .parse()
                        .map_err(|e| format!("invalid fixed '{value}': {e}"))?,
                )
            }
            "active" => {
                active = Some(
                    value
                        .parse()
                        .map_err(|e| format!("invalid active '{value}': {e}"))?,
                )
            }
            "window_mode" if !value.is_empty() => {
                window_mode = Some(
                    value
                        .parse::<WindowMode>()
                        .map_err(|e| format!("invalid window_mode '{value}': {e}"))?,
                );
            }
            _ => {}
        }
    }

    Ok(UpdateHabit {
        title,
        description: description.flatten(),
        recurrence,
        start_time: start_time.flatten(),
        end_time: end_time.flatten(),
        avg_minutes,
        sigma_minutes,
        parallelizable,
        allows_parallel,
        abandonability,
        active,
        fixed,
        window_mode,
    })
}

fn habit_step_row_to_input(step: &HabitStepRow) -> Result<HabitStepInput, String> {
    Ok(HabitStepInput {
        id: Some(step.id.clone()),
        position: step.position,
        title: step.title.clone(),
        description: step.description.clone(),
        start_time: step.start_time,
        end_time: step.end_time,
        avg_minutes: step.avg_minutes,
        sigma_minutes: Some(step.sigma_minutes),
        parallelizable: Some(step.parallelizable),
        allows_parallel: Some(step.allows_parallel),
        abandonability: Some(step.abandonability),
        fixed: Some(step.fixed),
        depends_on: step.depends_on.to_vec(),
    })
}

pub fn format_steps_for_editing(steps: &[HabitStepRow]) -> Result<String, String> {
    let inputs: Vec<HabitStepInput> = steps
        .iter()
        .map(habit_step_row_to_input)
        .collect::<Result<Vec<_>, _>>()?;
    let json = serde_json::to_string_pretty(&inputs)
        .map_err(|e| format!("failed to serialize steps: {e}"))?;
    let mut output = String::from(
        "// Edit habit steps. Lines starting with // are ignored.\n// Each element is a step object; omit or null 'id' to create a new step.\n",
    );
    output.push_str(&json);
    Ok(output)
}

pub fn parse_edited_steps(content: &str) -> Result<Vec<HabitStepInput>, String> {
    let json: String = content
        .lines()
        .filter(|line| {
            let trimmed = line.trim_start();
            !trimmed.is_empty() && !trimmed.starts_with("//")
        })
        .collect::<Vec<_>>()
        .join("\n");
    serde_json::from_str(&json).map_err(|e| format!("invalid steps JSON: {e}"))
}

pub fn open_editor(content: &str, suffix: &str) -> io::Result<String> {
    let dir = env::temp_dir();
    let path = dir.join(format!("takusu-edit-{suffix}.txt"));
    fs::write(&path, content)?;

    let editor = env::var("EDITOR").unwrap_or_else(|_| "vi".to_string());
    let status = Command::new("sh")
        .arg("-c")
        .arg(format!("{editor} '{}'", path.display()))
        .status()?;

    if !status.success() {
        fs::remove_file(&path).ok();
        return Err(io::Error::other("editor exited with non-zero status"));
    }

    let edited = fs::read_to_string(&path)?;
    fs::remove_file(&path).ok();
    Ok(edited)
}

#[cfg(test)]
mod tests {
    use super::*;
    use takusu_types::Quantity;

    fn task_row(
        id: &str,
        display_id: i64,
        title: &str,
        habit_id: Option<&str>,
        depends: &[&str],
    ) -> TaskRow {
        TaskRow {
            id: id.into(),
            display_id,
            title: title.into(),
            description: None,
            start_at: None,
            end_at: "2026-07-23T23:59:00Z".parse().unwrap(),
            avg_minutes: 30,
            sigma_minutes: 5,
            depends: depends
                .iter()
                .map(|s| s.to_string())
                .collect::<Vec<_>>()
                .into(),
            parallelizable: false,
            allows_parallel: false,
            abandonability: 0.5.into(),
            status: TaskStatus::Pending,
            habit_id: habit_id.map(|s| s.into()),
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
            created_at: "2026-07-23T00:00:00Z".parse().unwrap(),
            updated_at: "2026-07-23T00:00:00Z".parse().unwrap(),
        }
    }

    #[test]
    fn parse_edited_task_empty_end_at_is_skipped() {
        let input = "title: t\nend_at:\n";
        let update = parse_edited_task(input, &jiff::tz::TimeZone::UTC).unwrap();
        assert_eq!(update.title.as_deref(), Some("t"));
        assert_eq!(update.end_at, None, "empty end_at should skip update");
    }

    #[test]
    fn parse_edited_task_nonempty_end_at_is_set() {
        let input = "title: t\nend_at: 2026-07-06T18:00:00\n";
        let update = parse_edited_task(input, &jiff::tz::TimeZone::UTC).unwrap();
        assert_eq!(
            update.end_at.map(|t| t.to_string()),
            Some("2026-07-06T18:00:00Z".to_string()),
            "non-empty end_at should be set"
        );
    }

    #[test]
    fn parse_edited_task_empty_start_at_is_skipped() {
        let input = "title: t\nstart_at:\n";
        let update = parse_edited_task(input, &jiff::tz::TimeZone::UTC).unwrap();
        assert_eq!(update.start_at, None, "empty start_at should skip update");
    }

    #[test]
    fn parse_edited_habit_empty_times_are_skipped() {
        let input = "title: h\nstart_time:\nend_time:\n";
        let update = parse_edited_habit(input).unwrap();
        assert_eq!(
            update.start_time, None,
            "empty start_time should skip update"
        );
        assert_eq!(update.end_time, None, "empty end_time should skip update");
    }

    #[test]
    fn parse_edited_habit_nonempty_times_are_set() {
        let input = "title: h\nstart_time: 09:00\nend_time: 10:00\n";
        let update = parse_edited_habit(input).unwrap();
        assert_eq!(
            update.start_time.map(|t| t.to_string()),
            Some("09:00".to_string())
        );
        assert_eq!(
            update.end_time.map(|t| t.to_string()),
            Some("10:00".to_string())
        );
    }

    // ── Per-line error reporting (#347) ─────────────────────────────────

    #[test]
    fn parse_edited_task_line_without_colon_is_skipped() {
        // A malformed line should NOT discard the whole edit.
        let input = "title: t\nthis line has no colon\navg_minutes: 30\n";
        let update = parse_edited_task(input, &jiff::tz::TimeZone::UTC).unwrap();
        assert_eq!(update.title.as_deref(), Some("t"));
        assert_eq!(update.avg_minutes, Some(30));
    }

    #[test]
    fn parse_edited_task_bad_numeric_field_reports_error() {
        let input = "title: t\navg_minutes: abc\n";
        let err = parse_edited_task(input, &jiff::tz::TimeZone::UTC).unwrap_err();
        assert!(
            err.contains("avg_minutes"),
            "error should mention the field: {err}"
        );
        assert!(
            err.contains("abc"),
            "error should mention the bad value: {err}"
        );
    }

    #[test]
    fn parse_edited_task_bad_field_does_not_discard_valid_fields() {
        // Even when one numeric field is bad, the error should be returned
        // (we do not silently drop the valid `title`). The caller can show
        // the error so the user fixes the one bad line and re-edits.
        let input = "title: t\nsigma_minutes: xyz\nfixed: true\n";
        let err = parse_edited_task(input, &jiff::tz::TimeZone::UTC).unwrap_err();
        assert!(err.contains("sigma_minutes"), "error: {err}");
    }

    #[test]
    fn parse_edited_habit_line_without_colon_is_skipped() {
        let input = "title: h\nno colon here\nactive: true\n";
        let update = parse_edited_habit(input).unwrap();
        assert_eq!(update.title.as_deref(), Some("h"));
        assert_eq!(update.active, Some(true));
    }

    #[test]
    fn parse_edited_habit_bad_bool_field_reports_error() {
        let input = "title: h\nactive: maybe\n";
        let err = parse_edited_habit(input).unwrap_err();
        assert!(err.contains("active"), "error: {err}");
        assert!(err.contains("maybe"), "error: {err}");
    }

    // ── Habit steps editor (#95) ───────────────────────────────────────

    #[test]
    fn parse_edited_steps_ignores_comments_and_empty_lines() {
        let input = "// header\n[{\"position\": 1, \"title\": \"s\", \"start_time\": \"09:00\", \"end_time\": \"09:30\", \"avg_minutes\": 15}]\n// footer";
        let steps = parse_edited_steps(input).unwrap();
        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].position, 1);
        assert_eq!(steps[0].title, "s");
    }

    #[test]
    fn parse_edited_steps_round_trips_id_and_optional_fields() {
        let row = HabitStepRow {
            id: "step-1".into(),
            habit_id: "habit-1".into(),
            position: 2,
            title: "Prepare".into(),
            description: Some("get ready".into()),
            start_time: "09:00".parse().unwrap(),
            end_time: "09:30".parse().unwrap(),
            avg_minutes: 15,
            sigma_minutes: 3,
            parallelizable: false,
            allows_parallel: true,
            abandonability: 0.25.into(),
            fixed: true,
            depends_on: vec!["step-0".to_string()].into(),
            created_at: "2026-07-16T00:00:00Z".parse().unwrap(),
        };
        let formatted = format_steps_for_editing(&[row]).unwrap();
        let parsed = parse_edited_steps(&formatted).unwrap();
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].id.as_deref(), Some("step-1"));
        assert_eq!(parsed[0].position, 2);
        assert_eq!(parsed[0].title, "Prepare");
        assert_eq!(parsed[0].description.as_deref(), Some("get ready"));
        assert_eq!(parsed[0].start_time.to_string(), "09:00");
        assert_eq!(parsed[0].end_time.to_string(), "09:30");
        assert_eq!(parsed[0].avg_minutes, 15);
        assert_eq!(parsed[0].sigma_minutes, Some(3));
        assert_eq!(parsed[0].parallelizable, Some(false));
        assert_eq!(parsed[0].allows_parallel, Some(true));
        assert_eq!(parsed[0].abandonability, Some(0.25.into()));
        assert_eq!(parsed[0].fixed, Some(true));
        assert_eq!(parsed[0].depends_on, vec!["step-0"]);
    }

    #[test]
    fn parse_edited_steps_preserves_zero_sigma() {
        let row = HabitStepRow {
            id: "step-0".into(),
            habit_id: "habit-1".into(),
            position: 1,
            title: "No variance".into(),
            description: None,
            start_time: "09:00".parse().unwrap(),
            end_time: "09:15".parse().unwrap(),
            avg_minutes: 15,
            sigma_minutes: 0,
            parallelizable: false,
            allows_parallel: false,
            abandonability: 0.5.into(),
            fixed: false,
            depends_on: Vec::new().into(),
            created_at: "2026-07-16T00:00:00Z".parse().unwrap(),
        };
        let formatted = format_steps_for_editing(&[row]).unwrap();
        let parsed = parse_edited_steps(&formatted).unwrap();
        assert_eq!(parsed[0].sigma_minutes, Some(0));
    }

    // ── Habit task dependency references (#933) ──────────────────────────

    #[test]
    fn format_task_for_editing_uses_habit_scoped_reference() {
        let mut habit_map = HashMap::new();
        habit_map.insert("habit-1".into(), 7);

        let standalone = task_row("task-a", 3, "standalone", None, &[]);
        let habit_task = task_row("task-b", 3, "habit task", Some("habit-1"), &[]);
        let edited = task_row("edited", 1, "edited", None, &["task-a", "task-b"]);

        let formatted = format_task_for_editing(
            &edited,
            &[standalone, habit_task],
            &habit_map,
            &jiff::tz::TimeZone::UTC,
        );
        assert!(
            formatted.contains("depends: #3, h7#3"),
            "expected '#3, h7#3' in:\n{formatted}"
        );
    }

    #[test]
    fn format_task_for_editing_falls_back_when_habit_map_is_empty() {
        let habit_task = task_row("task-b", 3, "habit task", Some("habit-1"), &[]);
        let edited = task_row("edited", 1, "edited", None, &["task-b"]);

        let formatted = format_task_for_editing(
            &edited,
            &[habit_task],
            &HashMap::new(),
            &jiff::tz::TimeZone::UTC,
        );
        assert!(
            formatted.contains("depends: #3"),
            "expected '#3' fallback in:\n{formatted}"
        );
    }

    #[test]
    fn format_task_for_editing_renders_dependency_as_h1_hash_5() {
        let mut habit_map = HashMap::new();
        habit_map.insert("habit-1".into(), 1);

        let habit_task = task_row("task-b", 5, "habit task", Some("habit-1"), &[]);
        let edited = task_row("edited", 1, "edited", None, &["task-b"]);

        let formatted =
            format_task_for_editing(&edited, &[habit_task], &habit_map, &jiff::tz::TimeZone::UTC);
        assert!(
            formatted.contains("depends: h1#5"),
            "expected 'h1#5' in:\n{formatted}"
        );
    }

    #[test]
    fn parse_edited_task_preserves_habit_dependency_reference() {
        let input = "depends: h1#5, #3, task-uuid\n";
        let update = parse_edited_task(input, &jiff::tz::TimeZone::UTC).unwrap();
        assert_eq!(
            update.depends,
            Some(vec![
                "h1#5".to_string(),
                "3".to_string(),
                "task-uuid".to_string(),
            ])
        );
    }

    #[test]
    fn parse_edited_task_strips_only_leading_hash_from_habit_dependency() {
        // A leading `#` on a habit-scoped reference is acceptable input and
        // must be stripped without removing the `#` separator inside `h1#5`.
        let input = "depends: #h1#5, #3\n";
        let update = parse_edited_task(input, &jiff::tz::TimeZone::UTC).unwrap();
        assert_eq!(
            update.depends,
            Some(vec!["h1#5".to_string(), "3".to_string()])
        );
    }

    #[tokio::test]
    async fn parse_and_resolve_habit_dependency_reference_round_trip() {
        use takusu_local_lib::config::LocalConfig;
        use takusu_local_lib::storage_sqlite::SqliteStorage;
        use takusu_storage::{CreateHabit, CreateTask, Storage};

        let cfg = LocalConfig {
            db: "sqlite::memory:".into(),
            jwt_secret: "test-secret".into(),
            ..Default::default()
        };
        let storage = SqliteStorage::init(&cfg).await.unwrap();

        let habit = storage
            .create_habit(&CreateHabit {
                title: "h".into(),
                description: None,
                recurrence: "RRULE:FREQ=DAILY".into(),
                start_time: "09:00".parse().unwrap(),
                end_time: "09:30".parse().unwrap(),
                avg_minutes: 10,
                sigma_minutes: None,
                parallelizable: None,
                allows_parallel: None,
                abandonability: None,
                fixed: None,
                window_mode: None,
            })
            .await
            .unwrap();
        assert_eq!(habit.display_id, 1, "first habit should have display_id 1");

        let mut habit_task_id = String::new();
        for i in 0..5 {
            let task = storage
                .create_task(&CreateTask {
                    title: format!("h{i}"),
                    description: None,
                    start_at: None,
                    end_at: "2026-07-24T23:59:00Z".parse().unwrap(),
                    avg_minutes: 10,
                    sigma_minutes: None,
                    depends: None,
                    parallelizable: None,
                    allows_parallel: None,
                    abandonability: None,
                    ical_uid: None,
                    habit_id: Some(habit.id.clone()),
                    fixed: None,
                    habit_step_id: None,
                    quantity_total: None,
                    quantity_done: None,
                    quantity_unit: None,
                    original_quantity_total: None,
                })
                .await
                .unwrap();
            habit_task_id = task.id;
        }

        // Parse the editor output for a task that depends on h1#5.
        let input = "depends: h1#5\n";
        let update = parse_edited_task(input, &jiff::tz::TimeZone::UTC).unwrap();
        assert_eq!(update.depends, Some(vec!["h1#5".to_string()]));

        // Create the dependent task and verify that h1#5 resolves to the habit task.
        let target = storage
            .create_task(&CreateTask {
                title: "target".into(),
                description: None,
                start_at: None,
                end_at: "2026-07-25T23:59:00Z".parse().unwrap(),
                avg_minutes: 10,
                sigma_minutes: None,
                depends: update.depends,
                parallelizable: None,
                allows_parallel: None,
                abandonability: None,
                ical_uid: None,
                habit_id: None,
                fixed: None,
                habit_step_id: None,
                quantity_total: None,
                quantity_done: None,
                quantity_unit: None,
                original_quantity_total: None,
            })
            .await
            .unwrap();

        let resolved: Vec<String> = target.depends.to_vec();
        assert_eq!(resolved, vec![habit_task_id]);
    }
}
