use serde_json::json;
use takusu_client::*;
use takusu_types::{ScheduleMode, TaskStatus};

#[test]
fn create_task_serialization() {
    let ct = CreateTask {
        title: "Test task".to_string(),
        description: Some("A test".to_string()),
        start_at: None,
        end_at: "2025-06-05T18:00:00Z".parse().unwrap(),
        avg_minutes: 60,
        sigma_minutes: Some(10),
        depends: Some(vec!["dep1".to_string()]),
        parallelizable: Some(true),
        allows_parallel: Some(false),
        abandonability: Some(0.3.into()),
        ical_uid: Some("ical-123@example.com".to_string()),
        habit_id: Some("habit-1".to_string()),
        fixed: None,
        habit_step_id: None,
        quantity_total: None,
        quantity_done: None,
        quantity_unit: None,
        original_quantity_total: None,
    };

    let json = serde_json::to_value(&ct).unwrap();
    assert_eq!(json["title"], "Test task");
    assert_eq!(json["avg_minutes"], 60);
    assert_eq!(json["sigma_minutes"], 10);
    assert_eq!(json["abandonability"], 0.3);
    assert_eq!(json["ical_uid"], "ical-123@example.com");
    assert_eq!(json["habit_id"], "habit-1");
}

#[test]
fn create_task_defaults_are_skipped() {
    let ct = CreateTask {
        title: "Minimal".to_string(),
        description: None,
        start_at: None,
        end_at: "2025-06-05T18:00:00Z".parse().unwrap(),
        avg_minutes: 30,
        sigma_minutes: None,
        depends: None,
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
    };

    let json = serde_json::to_value(&ct).unwrap();
    assert_eq!(json["title"], "Minimal");
    assert_eq!(json["avg_minutes"], 30);
    assert!(!json.as_object().unwrap().contains_key("sigma_minutes"));
    assert!(!json.as_object().unwrap().contains_key("parallelizable"));
    assert!(!json.as_object().unwrap().contains_key("ical_uid"));
    assert!(!json.as_object().unwrap().contains_key("habit_id"));
}

#[test]
fn update_task_serialization() {
    let ut = UpdateTask {
        title: Some("Updated".to_string()),
        status: Some(TaskStatus::InProgress),
        parallelizable: Some(true),
        habit_id: Some("habit-2".to_string()),
        ..Default::default()
    };

    let json = serde_json::to_value(&ut).unwrap();
    assert_eq!(json["title"], "Updated");
    assert_eq!(json["status"], "in_progress");
    assert_eq!(json["parallelizable"], true);
    assert_eq!(json["habit_id"], "habit-2");
}

#[test]
fn update_task_default_is_empty() {
    let ut = UpdateTask::default();
    let json = serde_json::to_value(&ut).unwrap();
    assert!(json.as_object().unwrap().is_empty());
}

#[test]
fn generate_schedule_serialization() {
    let gs = GenerateSchedule {
        task_ids: None,
        sleep: SleepInput::Recommended,
    };

    let json = serde_json::to_value(&gs).unwrap();
    assert_eq!(json["sleep"], "recommended");
    assert!(!json.as_object().unwrap().contains_key("task_ids"));
    assert!(!json.as_object().unwrap().contains_key("until"));
}

#[test]
fn reschedule_serialization() {
    let rs = Reschedule {
        mode: ScheduleMode::Range,
        from: Some("2025-06-05T08:00:00Z".to_string()),
        until: Some("2025-06-05T18:00:00Z".to_string()),
        task_ids: None,
        pinned: vec![],
        sleep: SleepInput::Recommended,
    };

    let json = serde_json::to_value(&rs).unwrap();
    assert_eq!(json["mode"], "range");
    assert_eq!(json["sleep"], "recommended");
}

#[test]
fn move_entry_serialization() {
    let me = MoveEntry {
        start_at: "2025-06-05T12:00:00Z".parse().unwrap(),
        force: true,
    };

    let json = serde_json::to_value(&me).unwrap();
    assert_eq!(json["start_at"], "2025-06-05T12:00:00Z");
    assert_eq!(json["force"], true);
}

#[test]
fn move_entry_default_force_omitted() {
    let me = MoveEntry {
        start_at: "2025-06-05T12:00:00Z".parse().unwrap(),
        force: false,
    };

    let json = serde_json::to_value(&me).unwrap();
    assert_eq!(json["start_at"], "2025-06-05T12:00:00Z");
    assert_eq!(json["force"], false);
}

#[test]
fn move_entry_response_deserialization() {
    let json = serde_json::json!({
        "task_id": "task-1",
        "start_at": "2025-06-05T12:00:00Z",
        "end_at": "2025-06-05T13:00:00Z",
        "warnings": ["overlap ignored"]
    });
    let resp: MoveEntryResponse = serde_json::from_value(json).unwrap();
    assert_eq!(resp.task_id, "task-1");
    assert_eq!(resp.start_at, "2025-06-05T12:00:00Z".parse().unwrap());
    assert_eq!(resp.end_at, "2025-06-05T13:00:00Z".parse().unwrap());
    assert_eq!(resp.warnings, vec!["overlap ignored"]);
}

#[test]
fn move_entry_response_warnings_default_empty() {
    let json = serde_json::json!({
        "task_id": "task-1",
        "start_at": "2025-06-05T12:00:00Z",
        "end_at": "2025-06-05T13:00:00Z"
    });
    let resp: MoveEntryResponse = serde_json::from_value(json).unwrap();
    assert!(resp.warnings.is_empty());
}

#[test]
fn create_habit_serialization() {
    let ch = CreateHabit {
        title: "Daily standup".to_string(),
        description: None,
        recurrence: "daily".to_string(),
        start_time: "09:00".parse().unwrap(),
        end_time: "09:30".parse().unwrap(),
        avg_minutes: 30,
        sigma_minutes: None,
        parallelizable: None,
        allows_parallel: None,
        abandonability: None,
        fixed: None,
        window_mode: None,
    };

    let json = serde_json::to_value(&ch).unwrap();
    assert_eq!(json["title"], "Daily standup");
    assert_eq!(json["recurrence"], "daily");
}

#[test]
fn update_settings_serialization() {
    let us = UpdateSettings {
        tz: Some("Asia/Tokyo".to_string()),
        sleep_start: Some("23:00".parse().unwrap()),
        sleep_end: None,
        comfortable_minutes: None,
        maximum_minutes: None,
        ..UpdateSettings::default()
    };

    let json = serde_json::to_value(&us).unwrap();
    assert_eq!(json["tz"], "Asia/Tokyo");
    assert_eq!(json["sleep_start"], "23:00");
    assert!(!json.as_object().unwrap().contains_key("sleep_end"));
    assert!(
        !json
            .as_object()
            .unwrap()
            .contains_key("comfortable_minutes")
    );
    assert!(!json.as_object().unwrap().contains_key("maximum_minutes"));
}

#[test]
fn update_settings_new_fields_serialization() {
    let us = UpdateSettings {
        tz: None,
        sleep_start: None,
        sleep_end: None,
        comfortable_minutes: None,
        maximum_minutes: None,
        solver: Some(takusu_types::Solver::Sa),
        time_budget_ms: Some(1500),
        seed: Some(42),
        warm_start: Some(true),
        plan_length_days: None,
    };

    let json = serde_json::to_value(&us).unwrap();
    assert_eq!(json["solver"], "sa");
    assert_eq!(json["time_budget_ms"], 1500);
    assert_eq!(json["seed"], 42);
    assert_eq!(json["warm_start"], true);
}

#[test]
fn update_settings_default_skips_new_fields() {
    let us = UpdateSettings::default();
    let json = serde_json::to_value(&us).unwrap();
    assert!(!json.as_object().unwrap().contains_key("solver"));
    assert!(!json.as_object().unwrap().contains_key("time_budget_ms"));
    assert!(!json.as_object().unwrap().contains_key("seed"));
    assert!(!json.as_object().unwrap().contains_key("warm_start"));
}

#[test]
fn task_query_default_is_empty() {
    let tq = TaskQuery::default();
    assert!(tq.status.is_none());
    assert!(tq.from.is_none());
    assert!(tq.until.is_none());
    assert!(tq.habit_id.is_none());
    assert!(tq.ical_uid.is_none());
}

#[test]
fn client_error_display() {
    let api_err = ClientError::Api {
        status: 404,
        body: "not found".to_string(),
    };
    assert!(!format!("{api_err}").is_empty());
}

#[test]
fn task_row_deserialization() {
    let json = json!({
        "id": "task-123",
        "display_id": 42,
        "title": "Write tests",
        "description": "Add tests",
        "start_at": "2025-06-05T09:00:00Z",
        "end_at": "2025-06-05T10:00:00Z",
        "avg_minutes": 60,
        "sigma_minutes": 5,
        "depends": "[]",
        "parallelizable": false,
        "allows_parallel": false,
        "abandonability": 0.5,
        "status": "pending",
        "habit_id": null,
        "ical_uid": null,
        "created_at": "2025-06-01T00:00:00Z",
        "updated_at": "2025-06-01T00:00:00Z"
    });
    let tr: TaskRow = serde_json::from_value(json).unwrap();
    assert_eq!(tr.id, "task-123");
    assert_eq!(tr.display_id, 42);
    assert_eq!(tr.status, TaskStatus::Pending);
    assert!(tr.depends.is_empty());
}

#[test]
fn schedule_entry_roundtrip() {
    let original = json!({
        "task_id": "t1",
        "start_at": "2025-06-05T10:00:00Z",
        "end_at": "2025-06-05T11:00:00Z"
    });

    let se: ScheduleEntry = serde_json::from_value(original.clone()).unwrap();
    assert_eq!(se.task_id, "t1");
    let json = serde_json::to_value(&se).unwrap();
    assert_eq!(json["task_id"], "t1");
}

#[test]
fn token_row_deserialization() {
    let json = json!({
        "id": 1,
        "jti": "tsk_abc123",
        "scope": "read-write",
        "label": "test",
        "created_by": "root",
        "created_at": "2025-06-01T00:00:00Z",
        "revoked_at": null,
        "expires_at": null
    });
    let tr: TokenRow = serde_json::from_value(json).unwrap();
    assert_eq!(tr.id, 1);
    assert!(tr.revoked_at.is_none());
}

#[test]
fn token_create_response_deserialization() {
    let json = json!({
        "id": 1,
        "token": "tsk_new_token_value",
        "scope": "read-write",
        "label": "cli-token",
        "created_at": "2025-06-01T00:00:00Z",
        "expires_at": "2026-06-01T00:00:00Z"
    });
    let tcr: TokenCreateResponse = serde_json::from_value(json).unwrap();
    assert_eq!(tcr.token, "tsk_new_token_value");
    assert_eq!(tcr.id, 1);
    assert_eq!(
        tcr.expires_at,
        Some("2026-06-01T00:00:00Z".parse().unwrap())
    );
}

#[test]
fn settings_response_deserialization() {
    let json = json!({
        "tz": "UTC",
        "sleep_start": "22:00",
        "sleep_end": "06:00"
    });
    let sr: SettingsResponse = serde_json::from_value(json).unwrap();
    assert_eq!(sr.tz, "UTC");
    assert_eq!(sr.sleep_start, "22:00".parse().unwrap());
}

#[test]
fn schedule_row_deserialization() {
    let json = json!({
        "id": "active",
        "created_at": "2025-06-01T00:00:00Z",
        "updated_at": "2025-06-01T00:00:00Z",
        "schedule": "[{\"task_id\":\"t1\",\"start_at\":\"2025-06-01T10:00:00Z\",\"end_at\":\"2025-06-01T11:00:00Z\"}]"
    });
    let sr: ScheduleRow = serde_json::from_value(json).unwrap();
    assert_eq!(sr.id, "active");
    assert!(sr.schedule.as_inner().iter().any(|e| e.task_id == "t1"));
}

#[test]
fn sync_settings_response_deserialization() {
    let json = json!({
        "enabled": true,
        "calendar_id": "primary",
        "client_id": "client-123",
        "has_client_secret": true,
        "has_refresh_token": false
    });
    let resp: SyncSettingsResponse = serde_json::from_value(json).unwrap();
    assert!(resp.enabled);
    assert!(!resp.has_refresh_token);
}

#[test]
fn update_sync_settings_serialization() {
    let uss = UpdateSyncSettings {
        enabled: Some(true),
        calendar_id: Some("primary".to_string()),
        client_id: None,
        client_secret: None,
        refresh_token: None,
    };

    let json = serde_json::to_value(&uss).unwrap();
    assert_eq!(json["enabled"], true);
    assert!(!json.as_object().unwrap().contains_key("client_id"));
}

#[test]
fn habit_row_deserialization() {
    let json = json!({
        "id": "h1",
        "title": "Exercise",
        "description": null,
        "recurrence": "weekly",
        "start_time": "07:00",
        "end_time": "08:00",
        "avg_minutes": 60,
        "sigma_minutes": 5,
        "parallelizable": false,
        "allows_parallel": false,
        "abandonability": 0.3,
        "active": true,
        "created_at": "2025-06-01T00:00:00Z",
        "updated_at": "2025-06-01T00:00:00Z"
    });
    let hr: HabitRow = serde_json::from_value(json).unwrap();
    assert_eq!(hr.id, "h1");
    assert!(hr.active);
}

#[test]
fn client_error_is_send_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<ClientError>();
}

#[test]
fn dependency_analysis_response_deserialization() {
    let json = json!({
        "redundant": [
            {
                "from": "task-1", "from_title": "レポート提出",
                "to": "task-3", "to_title": "資料集め",
                "via": [
                    {"id": "task-1", "title": "レポート提出"},
                    {"id": "task-2", "title": "下書き"},
                    {"id": "task-3", "title": "資料集め"}
                ]
            }
        ]
    });
    let resp: DependencyAnalysisResponse = serde_json::from_value(json).unwrap();
    assert_eq!(resp.redundant.len(), 1);
    let r = &resp.redundant[0];
    assert_eq!(r.from, "task-1");
    assert_eq!(r.to, "task-3");
    assert_eq!(r.via.len(), 3);
    assert_eq!(r.via[1].title, "下書き");
}

#[test]
fn dependency_analysis_empty_response() {
    let json = json!({ "redundant": [] });
    let resp: DependencyAnalysisResponse = serde_json::from_value(json).unwrap();
    assert!(resp.redundant.is_empty());
}

#[test]
fn schedule_preview_response_deserialization() {
    let json = json!({
        "entries": [{
            "task_id": "t1",
            "start_at": "2025-06-05T10:00:00Z",
            "end_at": "2025-06-05T11:00:00Z"
        }],
        "unscheduled_task_ids": ["t2"],
        "displaced_task_ids": ["t3"],
        "sleep_minutes_before": 480,
        "sleep_minutes_after": 240,
        "warnings": ["could not place t2"]
    });
    let resp: SchedulePreviewResponse = serde_json::from_value(json).unwrap();
    assert_eq!(resp.entries.len(), 1);
    assert_eq!(resp.entries[0].task_id, "t1");
    assert_eq!(resp.unscheduled_task_ids, vec!["t2"]);
    assert_eq!(resp.displaced_task_ids, vec!["t3"]);
    assert_eq!(resp.sleep_minutes_before, 480);
    assert_eq!(resp.sleep_minutes_after, 240);
    assert_eq!(resp.warnings, vec!["could not place t2"]);
}

#[test]
fn schedule_preview_response_defaults_when_optional_fields_missing() {
    let json = json!({
        "entries": []
    });
    let resp: SchedulePreviewResponse = serde_json::from_value(json).unwrap();
    assert!(resp.entries.is_empty());
    assert!(resp.unscheduled_task_ids.is_empty());
    assert!(resp.displaced_task_ids.is_empty());
    assert_eq!(resp.sleep_minutes_before, 0);
    assert_eq!(resp.sleep_minutes_after, 0);
    assert!(resp.warnings.is_empty());
}

#[test]
fn oauth_url_response_deserialization() {
    let json = json!({ "url": "https://accounts.google.com/o/oauth2/auth?scope=calendar" });
    let resp: OAuthUrlResponse = serde_json::from_value(json).unwrap();
    assert_eq!(
        resp.url,
        "https://accounts.google.com/o/oauth2/auth?scope=calendar"
    );
}

#[test]
fn oauth_callback_response_deserialization() {
    let json = json!({ "refresh_token_set": true });
    let resp: OAuthCallbackResponse = serde_json::from_value(json).unwrap();
    assert!(resp.refresh_token_set);
}

#[test]
fn trigger_sync_response_deserialization() {
    let json = json!({ "status": "sync_triggered" });
    let resp: TriggerSyncResponse = serde_json::from_value(json).unwrap();
    assert_eq!(resp.status, "sync_triggered");
}
