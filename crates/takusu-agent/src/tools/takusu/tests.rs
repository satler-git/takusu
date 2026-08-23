use super::common::{
    TaskContext, TimeZoneCache, entry_in_range, format_datetime_for_display,
    format_display_datetime_args, habit_json, normalize_reference_array, normalize_status,
    overdue_in_range, schedule_entry_value, strip_leading_hash, task_json, task_reference,
    transform_preview, transitive_dependencies,
};
use super::mutation::{
    CreateHabit, CreateHabitArgs, CreateTask, CreateTaskArgs, DeleteHabit, DeleteHabitArgs,
    DeleteTask, DeleteTaskArgs, GenerateSchedule, GenerateScheduleArgs, MoveTaskTool, MutationSpec,
    MutationTool, Reschedule, RescheduleArgs, UpdateHabit, UpdateHabitArgs, UpdateTask,
    UpdateTaskArgs,
};
use super::read_tools::{GetHabit, GetSchedule, GetTask, HabitScheduledSpans, ListTasks};
use crate::{ChangeOperation, InvalidArgsError, Tool, ToolError, TypedTool};
use axum::{
    Json, Router,
    routing::{get, post},
};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::str::FromStr;
use std::sync::Arc;
use takusu_client::{
    Client, CommentRow, HabitDetail, HabitRow, HabitScheduledSpanRow, HabitStepRow, ScheduleEntry,
    ScheduleRow, SettingsResponse, TaskRow,
};
use takusu_types::{CommentAuthor, Quantity, TaskStatus, TaskStatusFilter};

// ── test-only helpers (moved from common.rs and mutation.rs) ─────────────

/// Parse `key` as either a single string or an array of display references.
/// Strips leading `#` characters and deduplicates while preserving order.
fn refs_from_args(
    args: &serde_json::Map<String, Value>,
    key: &str,
) -> Result<Vec<String>, ToolError> {
    fn non_empty_ref(key: &str, raw: &str) -> Result<String, ToolError> {
        let raw = raw.trim();
        if raw.is_empty() {
            return Err(ToolError::InvalidArgs(InvalidArgsError::new(
                key,
                "must not be empty",
            )));
        }
        let r = strip_leading_hash(raw);
        if r.is_empty() {
            return Err(ToolError::InvalidArgs(InvalidArgsError::new(
                key,
                "must not be just '#'",
            )));
        }
        Ok(r.to_string())
    }

    match args.get(key) {
        Some(Value::String(s)) => Ok(vec![non_empty_ref(key, s)?]),
        Some(Value::Array(arr)) => {
            let mut refs = Vec::with_capacity(arr.len());
            for value in arr {
                let s = value.as_str().ok_or_else(|| {
                    ToolError::InvalidArgs(InvalidArgsError::new(key, "must contain only strings"))
                })?;
                refs.push(non_empty_ref(key, s)?);
            }
            let mut seen = HashSet::new();
            refs.retain(|r| seen.insert(r.clone()));
            Ok(refs)
        }
        _ => Err(ToolError::InvalidArgs(InvalidArgsError::new(
            key,
            "must be a string or an array of strings",
        ))),
    }
}

fn task_row(
    id: &str,
    display_id: i64,
    title: &str,
    habit_id: Option<&str>,
    depends: &[&str],
) -> TaskRow {
    TaskRow {
        id: id.to_string(),
        display_id,
        title: title.to_string(),
        description: None,
        start_at: None,
        end_at: "2025-06-05T10:00:00Z".parse().unwrap(),
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
        habit_id: habit_id.map(|s| s.to_string()),
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
        created_at: "2025-06-01T00:00:00Z".parse().unwrap(),
        updated_at: "2025-06-01T00:00:00Z".parse().unwrap(),
    }
}

fn habit_row(id: &str, display_id: i64, title: &str) -> HabitRow {
    HabitRow {
        id: id.to_string(),
        display_id,
        title: title.to_string(),
        description: None,
        recurrence: "FREQ=DAILY".to_string(),
        start_time: "08:00".parse().unwrap(),
        end_time: "09:00".parse().unwrap(),
        avg_minutes: 60,
        sigma_minutes: 10,
        parallelizable: false,
        allows_parallel: false,
        abandonability: 0.5.into(),
        active: true,
        fixed: false,
        window_mode: takusu_types::WindowMode::Day,
        created_at: "2025-06-01T00:00:00Z".parse().unwrap(),
        updated_at: "2025-06-01T00:00:00Z".parse().unwrap(),
    }
}

fn step_row(id: &str, habit_id: &str, position: i64, title: &str) -> HabitStepRow {
    HabitStepRow {
        id: id.to_string(),
        habit_id: habit_id.to_string(),
        position,
        title: title.to_string(),
        description: None,
        start_time: "08:00".parse().unwrap(),
        end_time: "09:00".parse().unwrap(),
        avg_minutes: 30,
        sigma_minutes: 5,
        parallelizable: false,
        allows_parallel: false,
        abandonability: 0.5.into(),
        fixed: false,
        depends_on: Vec::new().into(),
        created_at: "2025-06-01T00:00:00Z".parse().unwrap(),
    }
}

#[test]
fn task_reference_schema_accepts_single_or_array() {
    let client = Client::new("http://localhost", "");
    let tool = GetTask {
        client: client.clone(),
        tz_cache: TimeZoneCache::new(client),
    };
    let schema = tool.parameters_schema();
    assert_eq!(schema["required"], json!(["task_ref"]));
    let alternatives = schema["properties"]["task_ref"]["anyOf"]
        .as_array()
        .expect("anyOf alternatives");
    assert_eq!(alternatives.len(), 2);
    assert!(alternatives.iter().any(|alt| alt["type"] == "string"));
    assert!(alternatives.iter().any(|alt| alt["type"] == "array"));
}

#[test]
fn task_reference_uses_global_or_habit_scoped_display_id() {
    let habit_id = "habit-uuid";
    let mut habit_map = HashMap::new();
    habit_map.insert(habit_id.to_string(), 7);

    let standalone = task_row("task-1", 42, "standalone", None, &[]);
    assert_eq!(task_reference(&standalone, &HashMap::new()), "#42");

    let habit_task = task_row("task-2", 3, "habit task", Some(habit_id), &[]);
    assert_eq!(task_reference(&habit_task, &habit_map), "h7#3");
}

#[test]
fn refs_from_args_parses_single_and_array_and_strips_hashes() {
    let mut single = serde_json::Map::new();
    single.insert("task_ref".into(), Value::String("#42".into()));
    assert_eq!(refs_from_args(&single, "task_ref").unwrap(), vec!["42"]);

    let mut array = serde_json::Map::new();
    array.insert(
        "task_ref".into(),
        Value::Array(vec!["#1".into(), " h2#3 ".into(), "#1".into()]),
    );
    assert_eq!(
        refs_from_args(&array, "task_ref").unwrap(),
        vec!["1", "h2#3"]
    );
}

#[test]
fn refs_from_args_parses_habit_refs() {
    let mut single = serde_json::Map::new();
    single.insert("habit_ref".into(), Value::String("h1".into()));
    assert_eq!(refs_from_args(&single, "habit_ref").unwrap(), vec!["h1"]);

    let mut array = serde_json::Map::new();
    array.insert(
        "habit_ref".into(),
        Value::Array(vec!["h1".into(), " h2 ".into(), "h1".into()]),
    );
    assert_eq!(
        refs_from_args(&array, "habit_ref").unwrap(),
        vec!["h1", "h2"]
    );
}

#[test]
fn refs_from_args_rejects_empty_or_hash_only_refs() {
    for bad in ["", " ", "#", " # "] {
        let mut single = serde_json::Map::new();
        single.insert("task_ref".into(), Value::String(bad.into()));
        assert!(refs_from_args(&single, "task_ref").is_err());
    }

    let mut array = serde_json::Map::new();
    array.insert(
        "task_ref".into(),
        Value::Array(vec!["#42".into(), "#".into()]),
    );
    assert!(refs_from_args(&array, "task_ref").is_err());
}

#[test]
fn get_habit_schema_accepts_single_or_array() {
    let client = Client::new("http://localhost", "");
    let tool = GetHabit { client };
    let schema = tool.parameters_schema();
    assert_eq!(schema["required"], json!(["habit_ref"]));
    let alternatives = schema["properties"]["habit_ref"]["anyOf"]
        .as_array()
        .expect("anyOf alternatives");
    assert_eq!(alternatives.len(), 2);
    assert!(alternatives.iter().any(|alt| alt["type"] == "string"));
    assert!(alternatives.iter().any(|alt| alt["type"] == "array"));
}

#[test]
fn transitive_dependencies_collects_recursive_dependencies_excluding_requested() {
    let a = task_row("a", 1, "a", None, &["b", "c"]);
    let b = task_row("b", 2, "b", None, &["d"]);
    let c = task_row("c", 3, "c", None, &[]);
    let d = task_row("d", 4, "d", None, &[]);

    let all = vec![a.clone(), b.clone(), c.clone(), d.clone()];
    let requested = vec![a];
    let (deps, missing) = transitive_dependencies(&requested, &all);

    assert!(missing.is_empty());
    assert_eq!(deps.len(), 3);
    let ids: Vec<&str> = deps.iter().map(|t| t.id.as_str()).collect();
    assert_eq!(ids, vec!["b", "c", "d"]);
}

#[test]
fn transitive_dependencies_reports_missing_dependency_ids() {
    let a = task_row("a", 1, "a", None, &["missing"]);

    let all = vec![a.clone()];
    let requested = vec![a];
    let (deps, missing) = transitive_dependencies(&requested, &all);

    assert!(deps.is_empty());
    assert_eq!(missing, vec!["missing"]);
}

#[test]
fn task_json_hides_internal_uuids_and_uses_references() {
    let habit = habit_row("habit-uuid", 7, "habit");
    let dep = task_row("dep-uuid", 5, "dep", None, &[]);
    let task = task_row("task-uuid", 3, "task", Some("habit-uuid"), &["dep-uuid"]);
    let ctx = TaskContext::new(&[task.clone(), dep.clone()], &[habit]);

    let value = task_json(&task, &ctx, None);
    assert!(value.get("id").is_none());
    assert!(value.get("habit_id").is_none());
    assert!(value.get("habit_step_id").is_none());
    assert_eq!(value["display_id"], 3);
    assert_eq!(value["reference"], "h7#3");
    assert_eq!(value["depends"], json!(["#5"]));
}

#[test]
fn habit_json_hides_internal_uuids_and_omits_ignored_fields_when_steps_exist() {
    let habit = habit_row("habit-uuid", 7, "habit");
    let step = step_row("step-uuid", "habit-uuid", 1, "step");
    let detail = HabitDetail {
        habit,
        steps: vec![step],
    };

    let value = habit_json(&detail);
    assert!(value.get("id").is_none());
    assert_eq!(value["display_id"], 7);
    assert_eq!(value["reference"], "h7");
    // Habits with steps ignore top-level fields that the scheduler takes
    // from each step instead (#1084).
    assert!(value.get("start_time").is_none());
    assert!(value.get("end_time").is_none());
    assert!(value.get("avg_minutes").is_none());
    assert!(value.get("sigma_minutes").is_none());
    assert!(value.get("parallelizable").is_none());
    assert!(value.get("allows_parallel").is_none());
    assert!(value.get("abandonability").is_none());

    let steps = value["steps"].as_array().unwrap();
    assert_eq!(steps.len(), 1);
    assert!(steps[0].get("id").is_none());
    assert!(steps[0].get("habit_id").is_none());
}

#[test]
fn habit_json_includes_habit_level_fields_when_no_steps() {
    let habit = habit_row("habit-uuid", 7, "habit");
    let detail = HabitDetail {
        habit,
        steps: vec![],
    };

    let value = habit_json(&detail);
    assert!(value.get("start_time").is_some());
    assert!(value.get("end_time").is_some());
    assert!(value.get("avg_minutes").is_some());
    assert!(value.get("sigma_minutes").is_some());
    assert!(value.get("parallelizable").is_some());
    assert!(value.get("allows_parallel").is_some());
    assert!(value.get("abandonability").is_some());
}

#[test]
fn habit_json_maps_step_dependencies_to_display_positions() {
    let habit = habit_row("habit-uuid", 7, "habit");
    let first = step_row("step-1", "habit-uuid", 0, "warmup");
    let mut second = step_row("step-2", "habit-uuid", 1, "run");
    second.depends_on = vec!["step-1".to_string()].into();
    let detail = HabitDetail {
        habit,
        steps: vec![first, second],
    };

    let value = habit_json(&detail);
    let steps = value["steps"].as_array().unwrap();
    assert_eq!(steps.len(), 2);
    assert_eq!(steps[0]["position"], 1);
    assert_eq!(steps[0]["depends_on"], json!([]));
    assert_eq!(steps[1]["position"], 2);
    assert_eq!(steps[1]["depends_on"], json!([1]));
}

#[test]
fn schedule_entry_value_includes_title_display_id_and_reference() {
    let task = task_row("task-uuid", 3, "task title", Some("habit-uuid"), &[]);
    let habit = habit_row("habit-uuid", 7, "habit");
    let ctx = TaskContext::new(&[task], &[habit]);

    let entry = json!({
        "task_id": "task-uuid",
        "start_at": "2025-06-05T10:00:00Z",
        "end_at": "2025-06-05T11:00:00Z",
    });
    let value = schedule_entry_value(&entry, &ctx, None);

    assert!(value.get("task_id").is_none());
    assert_eq!(value["reference"], "h7#3");
    assert_eq!(value["display_id"], 3);
    assert_eq!(value["title"], "task title");
    assert_eq!(value["start_at"], "2025-06-05T10:00:00Z");
    assert_eq!(value["end_at"], "2025-06-05T11:00:00Z");
}

#[test]
fn transform_preview_replaces_internal_task_ids_with_references() {
    let task = task_row("task-uuid", 3, "task", Some("habit-uuid"), &[]);
    let habit = habit_row("habit-uuid", 7, "habit");
    let ctx = TaskContext::new(&[task], &[habit]);

    let preview = json!({
        "entries": [{
            "task_id": "task-uuid",
            "start_at": "2025-06-05T10:00:00Z",
            "end_at": "2025-06-05T11:00:00Z",
        }],
        "unscheduled_task_ids": ["task-uuid"],
        "displaced_task_ids": ["task-uuid"],
    });
    let out = transform_preview(preview, &ctx, None);

    let entries = out["entries"].as_array().unwrap();
    assert_eq!(entries[0]["reference"], "h7#3");
    assert_eq!(out["unscheduled_task_ids"], json!(["h7#3"]));
    assert_eq!(out["displaced_task_ids"], json!(["h7#3"]));
}

#[test]
fn normalize_interprets_naive_datetime_in_server_timezone() {
    let tz = jiff::tz::TimeZone::get("Asia/Tokyo").unwrap();
    let mut args: CreateTaskArgs = serde_json::from_value(json!({
        "title": "test",
        "end_at": "2025-06-05T14:00",
        "avg_minutes": 30,
    }))
    .unwrap();

    <CreateTask as MutationSpec>::normalize(&mut args, &tz).unwrap();

    let value = serde_json::to_value(&args).unwrap();
    let end_at = value["end_at"].as_str().unwrap();
    // 2025-06-05 14:00 JST == 2025-06-05 05:00 UTC
    assert!(end_at.starts_with("2025-06-05T05:00:00"));
    assert!(end_at.ends_with('Z'));
}

#[test]
fn strip_leading_hash_removes_only_leading_hash() {
    assert_eq!(strip_leading_hash("#42"), "42");
    assert_eq!(strip_leading_hash("42"), "42");
    assert_eq!(strip_leading_hash("h1#5"), "h1#5");
    assert_eq!(strip_leading_hash("#h1#5"), "h1#5");
    assert_eq!(strip_leading_hash("uuid-like-string"), "uuid-like-string");
}

#[test]
fn normalize_reference_array_trims_and_strips_leading_hash() {
    let mut args = serde_json::Map::new();
    args.insert(
        "depends".to_string(),
        json!(["#5", " h1#3", "#42 ", "  uuid  "]),
    );
    normalize_reference_array(&mut args, "depends").unwrap();

    assert_eq!(args["depends"], json!(["5", "h1#3", "42", "uuid"]));
}

#[test]
fn normalize_reference_array_rejects_non_string_entries() {
    let mut args = serde_json::Map::new();
    args.insert("task_ids".to_string(), json!(["#5", 42]));

    assert!(normalize_reference_array(&mut args, "task_ids").is_err());
}

#[test]
fn normalize_refs_strips_hashes_for_backend() {
    let mut execution_args = serde_json::Map::new();
    execution_args.insert("task_ref".to_string(), Value::String("#42".to_string()));
    execution_args.insert("depends".to_string(), json!(["#1", "h2#3"]));

    <UpdateTask as MutationSpec>::normalize_refs(&mut execution_args).unwrap();

    assert_eq!(execution_args["task_ref"], "42");
    assert_eq!(execution_args["depends"], json!(["1", "h2#3"]));
}

#[test]
fn normalize_status_maps_common_synonyms() {
    assert_eq!(
        normalize_status("done").unwrap(),
        TaskStatusFilter::Completed
    );
    assert_eq!(
        normalize_status("Done").unwrap(),
        TaskStatusFilter::Completed
    );
    assert_eq!(
        normalize_status("  DONE  ").unwrap(),
        TaskStatusFilter::Completed
    );
    assert_eq!(
        normalize_status("complete").unwrap(),
        TaskStatusFilter::Completed
    );
    assert_eq!(
        normalize_status("in-progress").unwrap(),
        TaskStatusFilter::InProgress
    );
    assert_eq!(
        normalize_status("in progress").unwrap(),
        TaskStatusFilter::InProgress
    );
    assert_eq!(normalize_status("todo").unwrap(), TaskStatusFilter::Pending);
    assert_eq!(normalize_status("skip").unwrap(), TaskStatusFilter::Skipped);
    assert_eq!(
        normalize_status("completed").unwrap(),
        TaskStatusFilter::Completed
    );
    assert_eq!(
        normalize_status("pending").unwrap(),
        TaskStatusFilter::Pending
    );
    assert_eq!(
        normalize_status("overdue").unwrap(),
        TaskStatusFilter::Overdue
    );
}

#[test]
fn normalize_status_rejects_unknown_values() {
    assert!(normalize_status("deleted").is_err());
    assert!(normalize_status("foo").is_err());
}

#[test]
fn normalize_normalizes_status_for_update_task() {
    let tz = jiff::tz::TimeZone::get("UTC").unwrap();
    let mut args: UpdateTaskArgs = serde_json::from_value(json!({
        "task_ref": "#1",
        "status": "done",
    }))
    .unwrap();

    <UpdateTask as MutationSpec>::normalize(&mut args, &tz).unwrap();

    let value = serde_json::to_value(&args).unwrap();
    assert_eq!(value["status"], "completed");
}

#[test]
fn list_tasks_status_schema_has_enum() {
    let client = Client::new("http://localhost", "");
    let tool = ListTasks {
        client: client.clone(),
        tz_cache: TimeZoneCache::new(client),
    };
    let schema = tool.parameters_schema();
    let values: Vec<String> =
        serde_json::from_value(schema["properties"]["status"]["enum"].clone()).unwrap();
    assert!(values.contains(&"completed".to_string()));
    assert!(values.contains(&"pending".to_string()));
    assert!(values.contains(&"overdue".to_string()));
}

#[test]
fn update_task_status_schema_has_enum() {
    let client = Client::new("http://localhost", "");
    let tool = MutationTool::<UpdateTask>::new(client.clone(), TimeZoneCache::new(client));
    let schema = tool.parameters_schema();
    let values: Vec<String> =
        serde_json::from_value(schema["properties"]["status"]["enum"].clone()).unwrap();
    assert!(values.contains(&"completed".to_string()));
}

#[test]
fn create_task_description_mentions_one_utterance_capture() {
    let client = Client::new("http://localhost", "");
    let tool = MutationTool::<CreateTask>::new(client.clone(), TimeZoneCache::new(client));
    let desc = tool.description();
    assert!(
        desc.contains("quantity"),
        "description should mention quantity: {desc}"
    );
    assert!(
        desc.contains("avg_minutes"),
        "description should mention estimate: {desc}"
    );
    assert!(
        desc.contains("inferred_fields"),
        "description should mention inferred_fields: {desc}"
    );
    assert!(
        desc.contains("演習30題追加。金曜まで"),
        "description should include capture example: {desc}"
    );
}

#[test]
fn change_summary_covers_all_kinds() {
    let create_task: CreateTaskArgs = serde_json::from_value(json!({
        "title": "演習30題追加", "end_at": "2026-07-30", "avg_minutes": 30,
    }))
    .unwrap();
    assert_eq!(
        <CreateTask as MutationSpec>::change_summary(&create_task),
        (
            "演習30題追加".to_owned(),
            "「演習30題追加」を作成".to_owned()
        ),
    );

    let update_task_titled: UpdateTaskArgs =
        serde_json::from_value(json!({ "task_ref": "#42", "title": "予習" })).unwrap();
    assert_eq!(
        <UpdateTask as MutationSpec>::change_summary(&update_task_titled),
        ("#42".to_owned(), "「予習」を更新".to_owned()),
    );

    let update_task_ref_only: UpdateTaskArgs =
        serde_json::from_value(json!({ "task_ref": "#42" })).unwrap();
    assert_eq!(
        <UpdateTask as MutationSpec>::change_summary(&update_task_ref_only),
        ("#42".to_owned(), "#42を更新".to_owned()),
    );

    let delete_task: DeleteTaskArgs = serde_json::from_value(json!({ "task_ref": "#7" })).unwrap();
    assert_eq!(
        <DeleteTask as MutationSpec>::change_summary(&delete_task),
        ("#7".to_owned(), "#7を削除".to_owned()),
    );

    let create_habit: CreateHabitArgs = serde_json::from_value(json!({
        "title": "毎朝ジョギング", "recurrence": "FREQ=DAILY",
        "start_time": "06:00", "end_time": "07:00", "avg_minutes": 60,
    }))
    .unwrap();
    assert_eq!(
        <CreateHabit as MutationSpec>::change_summary(&create_habit),
        (
            "毎朝ジョギング".to_owned(),
            "「毎朝ジョギング」を作成".to_owned()
        ),
    );

    let update_habit: UpdateHabitArgs =
        serde_json::from_value(json!({ "habit_ref": "h3", "title": "夜ジョギング" })).unwrap();
    assert_eq!(
        <UpdateHabit as MutationSpec>::change_summary(&update_habit),
        ("h3".to_owned(), "「夜ジョギング」を更新".to_owned()),
    );

    let delete_habit: DeleteHabitArgs =
        serde_json::from_value(json!({ "habit_ref": "h1" })).unwrap();
    assert_eq!(
        <DeleteHabit as MutationSpec>::change_summary(&delete_habit),
        ("h1".to_owned(), "h1を削除".to_owned()),
    );

    let generate: GenerateScheduleArgs = serde_json::from_value(json!({})).unwrap();
    assert_eq!(
        <GenerateSchedule as MutationSpec>::change_summary(&generate),
        (String::new(), "スケジュールを生成".to_owned()),
    );

    let reschedule: RescheduleArgs = serde_json::from_value(json!({ "mode": "full" })).unwrap();
    assert_eq!(
        <Reschedule as MutationSpec>::change_summary(&reschedule),
        (String::new(), "スケジュールを再調整".to_owned()),
    );
}

#[test]
fn format_datetime_for_display_converts_utc_to_zoned() {
    let tz = jiff::tz::TimeZone::get("Asia/Tokyo").unwrap();
    let out = format_datetime_for_display("2025-06-05T10:00:00Z", &tz);
    assert!(out.contains("2025-06-05T19:00:00"));
    assert!(out.contains("+09:00"));
    assert!(out.contains("[Asia/Tokyo]"));
}

#[test]
fn format_datetime_for_display_handles_sqlite_datetime() {
    let tz = jiff::tz::TimeZone::get("Asia/Tokyo").unwrap();
    let out = format_datetime_for_display("2025-06-05 10:00:00", &tz);
    assert!(out.contains("2025-06-05T19:00:00"));
    assert!(out.contains("+09:00"));
    assert!(out.contains("[Asia/Tokyo]"));
}

#[test]
fn format_datetime_for_display_handles_offset_string() {
    let tz = jiff::tz::TimeZone::get("Asia/Tokyo").unwrap();
    let out = format_datetime_for_display("2025-06-05T10:00:00+00:00", &tz);
    assert!(out.contains("2025-06-05T19:00:00"));
    assert!(out.contains("+09:00"));
    assert!(out.contains("[Asia/Tokyo]"));
}

#[test]
fn format_datetime_for_display_handles_naive_with_t_separator() {
    let tz = jiff::tz::TimeZone::get("Asia/Tokyo").unwrap();
    let out = format_datetime_for_display("2025-06-05T10:00:00", &tz);
    assert!(out.contains("2025-06-05T19:00:00"));
    assert!(out.contains("+09:00"));
    assert!(out.contains("[Asia/Tokyo]"));
}

#[test]
fn format_datetime_for_display_returns_unknown_strings_unchanged() {
    let tz = jiff::tz::TimeZone::get("Asia/Tokyo").unwrap();
    assert_eq!(format_datetime_for_display("not-a-date", &tz), "not-a-date");
    assert_eq!(format_datetime_for_display("", &tz), "");
}

#[test]
fn task_json_converts_datetimes_to_zoned() {
    let habit = habit_row("habit-uuid", 7, "habit");
    let task = task_row("task-uuid", 3, "task", Some("habit-uuid"), &[]);
    let ctx = TaskContext::new(std::slice::from_ref(&task), &[habit]);
    let tz = jiff::tz::TimeZone::get("Asia/Tokyo").unwrap();

    let value = task_json(&task, &ctx, Some(&tz));

    assert!(
        value["end_at"]
            .as_str()
            .unwrap()
            .contains("2025-06-05T19:00:00")
    );
    assert!(
        value["created_at"]
            .as_str()
            .unwrap()
            .contains("2025-06-01T09:00:00")
    );
}

#[test]
fn schedule_entry_value_converts_datetimes_to_zoned() {
    let task = task_row("task-uuid", 3, "task title", Some("habit-uuid"), &[]);
    let habit = habit_row("habit-uuid", 7, "habit");
    let ctx = TaskContext::new(&[task], &[habit]);
    let entry = json!({
        "task_id": "task-uuid",
        "start_at": "2025-06-05T10:00:00Z",
        "end_at": "2025-06-05T11:00:00Z",
    });
    let tz = jiff::tz::TimeZone::get("Asia/Tokyo").unwrap();

    let value = schedule_entry_value(&entry, &ctx, Some(&tz));

    assert!(
        value["start_at"]
            .as_str()
            .unwrap()
            .contains("2025-06-05T19:00:00")
    );
    assert!(
        value["end_at"]
            .as_str()
            .unwrap()
            .contains("2025-06-05T20:00:00")
    );
}

#[test]
fn format_display_datetime_args_converts_utc_fields_to_zoned() {
    let tz = jiff::tz::TimeZone::get("Asia/Tokyo").unwrap();
    let mut args = serde_json::Map::new();
    args.insert(
        "start_at".into(),
        Value::String("2025-06-05T10:00:00Z".into()),
    );
    args.insert(
        "end_at".into(),
        Value::String("2025-06-05T11:00:00Z".into()),
    );
    args.insert("title".into(), Value::String("task".into()));
    format_display_datetime_args(&mut args, &tz);
    assert!(
        args["start_at"]
            .as_str()
            .unwrap()
            .contains("2025-06-05T19:00:00")
    );
    assert!(
        args["end_at"]
            .as_str()
            .unwrap()
            .contains("2025-06-05T20:00:00")
    );
    assert_eq!(args["title"], "task");
}

// ── get_schedule range helpers ───────────────────────────────────────

#[test]
fn get_schedule_schema_has_from_to_and_no_overdue() {
    let client = Client::new("http://localhost", "");
    let tool = GetSchedule {
        client: client.clone(),
        tz_cache: TimeZoneCache::new(client),
    };
    let schema = tool.parameters_schema();
    assert!(schema["properties"]["from"].is_object());
    assert!(schema["properties"]["to"].is_object());
    assert!(schema["properties"]["no_overdue"].is_object());
    assert!(schema["required"].as_array().is_none_or(|v| v.is_empty()));
}

#[test]
fn entry_in_range_keeps_overlapping_entries() {
    let tz = jiff::tz::TimeZone::UTC;
    let entry = json!({
        "task_id": "t1",
        "start_at": "2026-07-20T10:00:00Z",
        "end_at": "2026-07-20T11:00:00Z",
    });

    let from = jiff::Timestamp::from_str("2026-07-20T09:00:00Z").unwrap();
    let to = jiff::Timestamp::from_str("2026-07-20T12:00:00Z").unwrap();
    assert!(entry_in_range(&entry, Some(from), Some(to), &tz));
    assert!(entry_in_range(&entry, None, None, &tz));

    // Entry ends before the range starts.
    assert!(!entry_in_range(
        &entry,
        Some(jiff::Timestamp::from_str("2026-07-20T12:00:00Z").unwrap()),
        None,
        &tz
    ));

    // Entry starts after the range ends.
    assert!(!entry_in_range(
        &entry,
        None,
        Some(jiff::Timestamp::from_str("2026-07-20T09:00:00Z").unwrap()),
        &tz
    ));

    // Reversed range excludes everything.
    assert!(!entry_in_range(&entry, Some(to), Some(from), &tz));
}

#[test]
fn entry_in_range_handles_missing_or_invalid_timestamps() {
    let tz = jiff::tz::TimeZone::UTC;
    let from = jiff::Timestamp::from_str("2026-07-20T09:00:00Z").unwrap();
    let to = jiff::Timestamp::from_str("2026-07-20T12:00:00Z").unwrap();

    // Missing start_at: decision is based on end_at.
    let no_start_within = json!({
        "task_id": "t1",
        "end_at": "2026-07-20T10:00:00Z",
    });
    assert!(entry_in_range(&no_start_within, Some(from), Some(to), &tz));
    let no_start_after = json!({
        "task_id": "t1",
        "end_at": "2026-07-20T13:00:00Z",
    });
    assert!(!entry_in_range(&no_start_after, Some(from), Some(to), &tz));

    // Missing end_at: decision is based on start_at.
    let no_end_within = json!({
        "task_id": "t1",
        "start_at": "2026-07-20T10:00:00Z",
    });
    assert!(entry_in_range(&no_end_within, Some(from), Some(to), &tz));
    let no_end_before = json!({
        "task_id": "t1",
        "start_at": "2026-07-20T08:00:00Z",
    });
    assert!(!entry_in_range(&no_end_before, Some(from), Some(to), &tz));

    // Both timestamps missing: fail closed when a range is supplied.
    let no_times = json!({"task_id": "t1"});
    assert!(!entry_in_range(&no_times, Some(from), Some(to), &tz));
    assert!(entry_in_range(&no_times, None, None, &tz));

    // Unparseable timestamps: fail closed when a range is supplied.
    let invalid = json!({
        "task_id": "t1",
        "start_at": "not-a-date",
        "end_at": "also-not",
    });
    assert!(!entry_in_range(&invalid, Some(from), Some(to), &tz));
    assert!(entry_in_range(&invalid, None, None, &tz));
}

#[test]
fn overdue_in_range_filters_by_deadline() {
    let tz = jiff::tz::TimeZone::UTC;
    let mut task = task_row("t1", 1, "task", None, &[]);
    task.end_at = "2026-07-20T10:00:00Z".parse().unwrap();

    let before = jiff::Timestamp::from_str("2026-07-20T09:00:00Z").unwrap();
    let after = jiff::Timestamp::from_str("2026-07-20T11:00:00Z").unwrap();

    assert!(overdue_in_range(&task, Some(before), Some(after), &tz));
    assert!(!overdue_in_range(&task, Some(after), None, &tz));
    assert!(!overdue_in_range(&task, None, Some(before), &tz));

    // Reversed range excludes everything.
    assert!(!overdue_in_range(&task, Some(after), Some(before), &tz));
}

#[tokio::test]
async fn move_task_tool_proposes_move_with_existing_entry() {
    let task = Arc::new(task_row("task-uuid", 42, "買い物", None, &[]));
    let task_for_get = task.as_ref().clone();
    let task_for_list = vec![task.as_ref().clone()];
    let habits = vec![habit_row("habit-uuid", 1, "朝のランニング")];
    let schedule = vec![ScheduleEntry {
        task_id: "task-uuid".to_string(),
        start_at: "2025-06-05T18:00:00Z".parse().unwrap(),
        end_at: "2025-06-05T18:30:00Z".parse().unwrap(),
    }]
    .into();
    let schedule_row = ScheduleRow {
        id: "sched-1".into(),
        created_at: "2025-06-01T00:00:00Z".parse().unwrap(),
        updated_at: "2025-06-01T00:00:00Z".parse().unwrap(),
        schedule,
        horizon_task_ids: Vec::new().into(),
    };

    let app = Router::new()
        .route("/api/tasks/{id}", get(move || async { Json(task_for_get) }))
        .route("/api/tasks", get(move || async { Json(task_for_list) }))
        .route("/api/habits", get(move || async { Json(habits) }))
        .route("/api/schedule", get(move || async { Json(schedule_row) }))
        .route(
            "/api/settings",
            get(|| async {
                Json(SettingsResponse {
                    tz: "Asia/Tokyo".into(),
                    sleep_start: "23:00".parse().unwrap(),
                    sleep_end: "07:00".parse().unwrap(),
                    comfortable_minutes: None,
                    maximum_minutes: None,
                    solver: takusu_types::Solver::Auto,
                    time_budget_ms: None,
                    seed: None,
                    warm_start: false,
                    plan_length_days: 14,
                    device_priority: takusu_types::JsonString::new(vec![
                        "desktop".to_string(),
                        "android".to_string(),
                    ]),
                })
            }),
        );

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    let client = Client::new(&format!("http://{addr}"), "");
    let tz_cache = TimeZoneCache::new(client.clone());
    let tool = crate::tool::Typed(MoveTaskTool { client, tz_cache });
    let args = json!({"task_ref": "#42", "start_at": "2025-06-05T19:00:00+09:00"});
    let output = tool.call(args).await.unwrap();

    assert_eq!(output.proposed_changes.len(), 1);
    let change = &output.proposed_changes[0];
    assert_eq!(change.operation, ChangeOperation::Move);
    assert_eq!(change.target.to_string(), "task #42");

    let before = change.before.as_ref().unwrap();
    assert_eq!(before["schedule_start_at"], "2025-06-05T18:00:00Z");
    assert_eq!(before["schedule_end_at"], "2025-06-05T18:30:00Z");

    let after = change.after.as_ref().unwrap().as_object().unwrap();
    assert_eq!(after["task_ref"], "#42");
    assert!(after.get("end_at").is_some());

    let execution = change.arguments.as_ref().unwrap().as_object().unwrap();
    assert_eq!(execution["task_ref"], "42");
}

fn settings_response() -> SettingsResponse {
    SettingsResponse {
        tz: "Asia/Tokyo".into(),
        sleep_start: "23:00".parse().unwrap(),
        sleep_end: "07:00".parse().unwrap(),
        comfortable_minutes: None,
        maximum_minutes: None,
        solver: takusu_types::Solver::Auto,
        time_budget_ms: None,
        seed: None,
        warm_start: false,
        plan_length_days: 14,
        device_priority: takusu_types::JsonString::new(vec![
            "desktop".to_string(),
            "android".to_string(),
        ]),
    }
}

#[tokio::test]
async fn update_task_tool_fetches_before_and_normalizes() {
    let task = Arc::new(task_row("task-uuid", 42, "買い物", None, &[]));
    let task_for_get = task.as_ref().clone();
    let task_for_list = vec![task.as_ref().clone()];
    let habits: Vec<HabitRow> = vec![];

    let app = Router::new()
        .route("/api/tasks/{id}", get(move || async { Json(task_for_get) }))
        .route("/api/tasks", get(move || async { Json(task_for_list) }))
        .route("/api/habits", get(move || async { Json(habits) }))
        .route("/api/settings", get(|| async { Json(settings_response()) }));

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    let client = Client::new(&format!("http://{addr}"), "");
    let tz_cache = TimeZoneCache::new(client.clone());
    let tool = crate::tool::Typed(MutationTool::<UpdateTask>::new(client, tz_cache));
    let args = json!({
        "task_ref": "#42",
        "title": "予習",
        "end_at": "2026-07-30T10:00",
        "status": "done",
    });
    let output = tool.call(args).await.unwrap();

    assert_eq!(output.proposed_changes.len(), 1);
    let change = &output.proposed_changes[0];
    assert_eq!(change.operation, ChangeOperation::Update);
    assert_eq!(change.target.to_string(), "task #42");
    assert_eq!(change.description, "「予習」を更新");
    assert!(change.observed_updated_at.is_some());

    // "before" state is fetched from the server.
    let before = change.before.as_ref().unwrap();
    assert_eq!(before["title"], "買い物");

    // Display args keep the user-facing reference; status is normalized.
    let after = change.after.as_ref().unwrap().as_object().unwrap();
    assert_eq!(after["task_ref"], "#42");
    assert_eq!(after["status"], "completed");

    // Execution args strip the leading `#`.
    let execution = change.arguments.as_ref().unwrap().as_object().unwrap();
    assert_eq!(execution["task_ref"], "42");
    assert_eq!(execution["status"], "completed");
}

#[tokio::test]
async fn reschedule_tool_runs_preview_and_proposes() {
    let tasks: Vec<TaskRow> = vec![];
    let habits: Vec<HabitRow> = vec![];

    let app = Router::new()
        .route(
            "/api/schedule/preview",
            post(|| async { Json(json!({ "entries": [] })) }),
        )
        .route("/api/tasks", get(move || async { Json(tasks) }))
        .route("/api/habits", get(move || async { Json(habits) }))
        .route("/api/settings", get(|| async { Json(settings_response()) }));

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    let client = Client::new(&format!("http://{addr}"), "");
    let tz_cache = TimeZoneCache::new(client.clone());
    let tool = crate::tool::Typed(MutationTool::<Reschedule>::new(client, tz_cache));
    let args = json!({ "mode": "full", "from": "2026-07-30T09:00" });
    let output = tool.call(args).await.unwrap();

    assert_eq!(output.proposed_changes.len(), 1);
    let change = &output.proposed_changes[0];
    assert_eq!(change.operation, ChangeOperation::Reschedule);
    assert_eq!(change.description, "スケジュールを再調整");

    // Execution args normalize the naive datetime to UTC and carry the preview.
    let execution = change.arguments.as_ref().unwrap().as_object().unwrap();
    assert_eq!(execution["mode"], "full");
    assert!(execution["from"].as_str().unwrap().ends_with('Z'));
    assert!(execution.get("_preview_entries").is_some());

    // Display args carry the transformed preview.
    let after = change.after.as_ref().unwrap().as_object().unwrap();
    assert!(after.get("_preview").is_some());
}

#[tokio::test]
async fn habit_scheduled_spans_tool_lists_and_proposes() {
    let habit = habit_row("habit-uuid", 1, "朝のランニング");
    let habit_detail = HabitDetail {
        habit: habit.clone(),
        steps: vec![],
    };
    let span = HabitScheduledSpanRow {
        id: "span-uuid".into(),
        habit_id: "habit-uuid".into(),
        start_date: "2025-08-01".parse().unwrap(),
        end_date: "2025-08-07".parse().unwrap(),
        reason: Some("旅行".into()),
        created_at: "2025-06-01T00:00:00Z".parse().unwrap(),
    };
    let spans_for_list = vec![span.clone()];

    let app = Router::new()
        .route(
            "/api/habits/{id}",
            get(move || async move { Json(habit_detail.clone()) }),
        )
        .route(
            "/api/habits/{id}/scheduled-spans",
            get(move || async move { Json(spans_for_list.clone()) }),
        )
        .route(
            "/api/settings",
            get(|| async {
                Json(SettingsResponse {
                    tz: "Asia/Tokyo".into(),
                    sleep_start: "23:00".parse().unwrap(),
                    sleep_end: "07:00".parse().unwrap(),
                    comfortable_minutes: None,
                    maximum_minutes: None,
                    solver: takusu_types::Solver::Auto,
                    time_budget_ms: None,
                    seed: None,
                    warm_start: false,
                    plan_length_days: 14,
                    device_priority: takusu_types::JsonString::new(vec![
                        "desktop".to_string(),
                        "android".to_string(),
                    ]),
                })
            }),
        );

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    let client = Client::new(&format!("http://{addr}"), "");
    let tz_cache = TimeZoneCache::new(client.clone());
    let tool = crate::tool::Typed(HabitScheduledSpans { client, tz_cache });

    // list
    let output = tool
        .call(json!({"habit_ref": "h1", "action": "list"}))
        .await
        .unwrap();
    assert!(output.proposed_changes.is_empty());
    let content: Value = serde_json::from_str(&output.content).unwrap();
    assert_eq!(content["habit_ref"], "h1");
    assert_eq!(content["active"], true);
    assert_eq!(content["kind"], "pause");
    assert_eq!(content["spans"].as_array().unwrap().len(), 1);

    // create proposal
    let output = tool
        .call(json!({
            "habit_ref": "h1",
            "action": "create",
            "start_date": "2025-09-01",
            "end_date": "2025-09-07",
            "reason": "出張"
        }))
        .await
        .unwrap();
    assert_eq!(output.proposed_changes.len(), 1);
    let change = &output.proposed_changes[0];
    assert_eq!(change.operation, ChangeOperation::CreateScheduledSpan);
    assert_eq!(change.target.to_string(), "habit h1");
    assert!(change.before.is_none());
    let args = change.arguments.as_ref().unwrap().as_object().unwrap();
    assert_eq!(args["start_date"], "2025-09-01");
    assert_eq!(args["end_date"], "2025-09-07");
    assert_eq!(args["habit_ref"], "h1");

    // delete proposal
    let output = tool
        .call(json!({
            "habit_ref": "h1",
            "action": "delete",
            "span_id": "span-uuid"
        }))
        .await
        .unwrap();
    assert_eq!(output.proposed_changes.len(), 1);
    let change = &output.proposed_changes[0];
    assert_eq!(change.operation, ChangeOperation::DeleteScheduledSpan);
    assert_eq!(change.target.to_string(), "habit h1");
    let before = change.before.as_ref().unwrap();
    assert_eq!(before["start_date"], "2025-08-01");
    let args = change.arguments.as_ref().unwrap().as_object().unwrap();
    assert_eq!(args["span_id"], "span-uuid");
    assert_eq!(args["habit_ref"], "h1");
}

// ── foreign-field rejection tests ───────────────────────────────────────
//
// Each mutation kind has its own args struct with `deny_unknown_fields`, so a
// field that belongs to another kind is rejected at deserialization time
// (surfaced with a field name by `serde_path_to_error` in the `Typed` wrapper).

#[test]
fn create_task_rejects_habit_ref() {
    let err = serde_json::from_value::<CreateTaskArgs>(json!({
        "title": "test",
        "end_at": "2026-07-30",
        "avg_minutes": 30,
        "habit_ref": "h1",
    }))
    .unwrap_err();
    assert!(err.to_string().contains("habit_ref"));
}

#[test]
fn update_task_rejects_steps() {
    let err = serde_json::from_value::<UpdateTaskArgs>(json!({
        "task_ref": "#1",
        "steps": [{"position": 1, "title": "s", "start_time": "09:00", "end_time": "10:00", "avg_minutes": 60}],
    }))
    .unwrap_err();
    assert!(err.to_string().contains("steps"));
}

#[test]
fn create_task_rejects_status() {
    let err = serde_json::from_value::<CreateTaskArgs>(json!({
        "title": "test",
        "end_at": "2026-07-30",
        "avg_minutes": 30,
        "status": "completed",
    }))
    .unwrap_err();
    assert!(err.to_string().contains("status"));
}

#[test]
fn create_habit_rejects_task_fields() {
    let err = serde_json::from_value::<CreateHabitArgs>(json!({
        "title": "jogging",
        "recurrence": "FREQ=DAILY",
        "start_time": "06:00",
        "end_time": "07:00",
        "avg_minutes": 60,
        "task_ref": "#1",
    }))
    .unwrap_err();
    assert!(err.to_string().contains("task_ref"));
}

#[test]
fn delete_task_rejects_schedule_fields() {
    let err = serde_json::from_value::<DeleteTaskArgs>(json!({
        "task_ref": "#1",
        "mode": "full",
    }))
    .unwrap_err();
    assert!(err.to_string().contains("mode"));
}

#[test]
fn create_task_accepts_relevant_fields() {
    let args = serde_json::from_value::<CreateTaskArgs>(json!({
        "title": "test",
        "end_at": "2026-07-30",
        "avg_minutes": 30,
        "description": "desc",
        "start_at": "2026-07-28T09:00",
        "sigma_minutes": 5,
        "depends": ["#2"],
        "parallelizable": true,
        "fixed": false,
        "quantity_total": 10,
        "quantity_unit": "pages",
        "why": "reason",
        "warnings": ["watch out"],
    }));
    assert!(args.is_ok());
}

#[test]
fn delete_task_accepts_only_task_ref() {
    let args = serde_json::from_value::<DeleteTaskArgs>(json!({
        "task_ref": "#1",
        "why": "done",
    }));
    assert!(args.is_ok());
}

#[test]
fn create_task_requires_mandatory_fields() {
    let err = serde_json::from_value::<CreateTaskArgs>(json!({
        "title": "test",
    }))
    .unwrap_err();
    assert!(err.to_string().contains("end_at"));
}

#[tokio::test]
async fn get_task_attaches_comment_timeline() {
    let task = Arc::new(task_row("task-uuid", 42, "買い物", None, &[]));
    let task_for_get = task.as_ref().clone();
    let task_for_list = vec![task.as_ref().clone()];
    let habits: Vec<HabitRow> = vec![];
    let comments = [
        CommentRow {
            id: "c1".into(),
            task_id: "task-uuid".into(),
            author: CommentAuthor::Agent,
            content: "思ったより手間取った".into(),
            seq: 1,
            created_at: "2025-06-01T00:00:00Z".parse().unwrap(),
        },
        CommentRow {
            id: "c2".into(),
            task_id: "task-uuid".into(),
            author: CommentAuthor::User,
            content: "次回は30分で".into(),
            seq: 2,
            created_at: "2025-06-02T00:00:00Z".parse().unwrap(),
        },
    ];

    let app = Router::new()
        .route("/api/tasks/{id}", get(move || async { Json(task_for_get) }))
        .route("/api/tasks", get(move || async { Json(task_for_list) }))
        .route("/api/habits", get(move || async { Json(habits) }))
        .route(
            "/api/tasks/{id}/comments",
            get(move || async { Json(comments) }),
        )
        .route("/api/settings", get(|| async { Json(settings_response()) }));

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    let client = Client::new(&format!("http://{addr}"), "");
    let tz_cache = TimeZoneCache::new(client.clone());
    let tool = crate::tool::Typed(GetTask { client, tz_cache });
    let output = tool.call(json!({ "task_ref": ["#42"] })).await.unwrap();
    let parsed: Value = serde_json::from_str(&output.content).unwrap();
    let task_view = &parsed["tasks"][0];
    assert_eq!(task_view["display_id"], 42);
    assert_eq!(task_view["comment_count"], 2);
    let attached = task_view["comments"].as_array().unwrap();
    assert_eq!(attached.len(), 2);
    assert_eq!(attached[0]["seq"], 1);
    assert_eq!(attached[1]["author"], "user");
}
