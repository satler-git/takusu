//! Parameterized storage-suite tests: the same `Storage` trait scenarios run
//! against both backends —
//!
//! - `sqlite`: `SqliteStorage` backed by an in-memory SQLite database.
//! - `workers`: the real `takusu-worker` running under `wrangler dev --local`
//!   (workerd + D1), accessed through `WorkersStorage`.
//!
//! Every test is `#[ignore]`d so the default `cargo nextest run --all` does
//! not try to spin up wrangler. CI runs them via
//! `cargo test -p takusu-local --test storage_suite -- --ignored --test-threads=1`
//! in the dedicated `worker-test` job (which pre-builds the worker with
//! `worker-build --release` and provides wrangler).

mod common;

use std::sync::Arc;
use std::sync::LazyLock;

use rstest::rstest;
use takusu_contracts::{
    CreateHabit, CreateHabitScheduledSpan, CreateMemory, CreateTask, MemoryQuery, StartWorkSession,
    Storage, StorageError, TaskQuery, UpdateMemory, UpdateTask,
};
use takusu_local_lib::config::LocalConfig;
use takusu_local_lib::storage_sqlite::SqliteStorage;
use takusu_local_lib::storage_workers::WorkersStorage;
use takusu_types::{CommentAuthor, EnumLabel, MemoryKind, Quantity, TaskStatus, TaskStatusFilter};

use common::{JWT_SECRET, WRANGLER_PORT, root_token, spawn_wrangler};

/// Single shared wrangler instance for the whole test process.
///
/// Spawned lazily on first `setup_workers()` call. If `wrangler` is not on
/// `PATH` (e.g. running only the sqlite cases locally without the nix worker
/// shell), `spawn_wrangler` panics with a clear message — the `Option` wrapper
/// keeps the static initialiser total so a panic in `LazyLock::new` does not
/// poison later sqlite-only runs in the same process.
static WRANGLER: LazyLock<Option<common::WranglerGuard>> = LazyLock::new(|| {
    if std::process::Command::new("wrangler")
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok()
    {
        Some(spawn_wrangler())
    } else {
        None
    }
});

async fn setup_sqlite() -> Arc<dyn Storage> {
    let cfg = LocalConfig {
        db: "sqlite::memory:".into(),
        jwt_secret: JWT_SECRET.into(),
        ..Default::default()
    };
    Arc::new(SqliteStorage::init(&cfg).await.expect("sqlite init"))
}

async fn setup_workers() -> Arc<dyn Storage> {
    // Touch the lazy guard so wrangler is running before we build the client.
    if WRANGLER.is_none() {
        panic!(
            "wrangler is not on PATH; run via `nix develop .#worker` or install wrangler to run the workers backend"
        );
    }
    Arc::new(WorkersStorage::new_with(
        format!("http://127.0.0.1:{WRANGLER_PORT}"),
        root_token().to_string(),
    ))
}

/// Wipe all mutable state so each test starts from a clean slate.
async fn cleanup(storage: &dyn Storage) {
    // tasks
    let tasks = storage
        .list_tasks(&TaskQuery::default())
        .await
        .expect("list_tasks");
    for t in &tasks {
        let _ = storage.delete_task(&t.id).await;
    }
    // habits (+ scheduled spans + steps are cascaded by habit delete)
    let habits = storage.list_habits().await.expect("list_habits");
    for h in &habits {
        let _ = storage.delete_habit(&h.id).await;
    }
    // tokens
    let tokens = storage.list_tokens().await.expect("list_tokens");
    for t in &tokens {
        let _ = storage.revoke_token(t.id).await;
    }
    // skills
    let skills = storage.list_skills().await.expect("list_skills");
    for s in &skills {
        let _ = storage.delete_skill(&s.slug).await;
    }
    // memories: search_memories requires a non-empty `q`, so we cannot list
    // all memories here. Memory tests clean up after themselves; the workers
    // backend uses a fresh temp D1 per process so there is no cross-run leak.
    // schedule
    let _ = storage.clear_schedule().await;
    // gcal mappings
    let _ = storage.clear_gcal_mappings().await;
}

/// Build a minimal `CreateTask` with sensible defaults for suite tests.
fn sample_task(title: &str) -> CreateTask {
    CreateTask {
        title: title.into(),
        description: None,
        start_at: None,
        end_at: "2030-01-01T00:00:00+00:00".parse().unwrap(),
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
    }
}

#[rstest]
#[case::sqlite("sqlite")]
#[case::workers("workers")]
#[tokio::test]
#[ignore]
async fn create_and_list_task(#[case] backend: &str) {
    let storage = match backend {
        "sqlite" => setup_sqlite().await,
        "workers" => setup_workers().await,
        _ => unreachable!(),
    };
    cleanup(&*storage).await;

    let created = storage
        .create_task(&sample_task("suite task"))
        .await
        .expect("create_task");
    assert_eq!(created.title, "suite task");
    assert_eq!(created.status.as_str(), "pending");
    assert_eq!(created.avg_minutes, 30);

    let rows = storage
        .list_tasks(&TaskQuery::default())
        .await
        .expect("list_tasks");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, created.id);
    assert_eq!(rows[0].title, "suite task");
}

#[rstest]
#[case::sqlite("sqlite")]
#[case::workers("workers")]
#[tokio::test]
#[ignore]
async fn update_task_title(#[case] backend: &str) {
    let storage = match backend {
        "sqlite" => setup_sqlite().await,
        "workers" => setup_workers().await,
        _ => unreachable!(),
    };
    cleanup(&*storage).await;

    let created = storage
        .create_task(&sample_task("original"))
        .await
        .expect("create_task");
    let updated = storage
        .update_task(
            &created.id,
            &UpdateTask {
                title: Some("renamed".into()),
                description: None,
                start_at: None,
                end_at: None,
                avg_minutes: None,
                sigma_minutes: None,
                depends: None,
                parallelizable: None,
                allows_parallel: None,
                abandonability: None,
                status: None,
                habit_id: None,
                user_edited: None,
                fixed: None,
                habit_step_id: None,
                quantity_total: None,
                quantity_done: None,
                quantity_unit: None,
                original_quantity_total: None,
            },
        )
        .await
        .expect("update_task");
    assert_eq!(updated.title, "renamed");
    assert_eq!(updated.avg_minutes, 30); // unchanged
}

#[rstest]
#[case::sqlite("sqlite")]
#[case::workers("workers")]
#[tokio::test]
#[ignore]
async fn delete_task(#[case] backend: &str) {
    let storage = match backend {
        "sqlite" => setup_sqlite().await,
        "workers" => setup_workers().await,
        _ => unreachable!(),
    };
    cleanup(&*storage).await;

    let created = storage
        .create_task(&sample_task("doomed"))
        .await
        .expect("create_task");
    storage.delete_task(&created.id).await.expect("delete_task");
    let rows = storage
        .list_tasks(&TaskQuery::default())
        .await
        .expect("list_tasks");
    assert!(rows.is_empty());
}

#[rstest]
#[case::sqlite("sqlite")]
#[case::workers("workers")]
#[tokio::test]
#[ignore]
async fn delete_task_closes_open_work_session(#[case] backend: &str) {
    let storage = match backend {
        "sqlite" => setup_sqlite().await,
        "workers" => setup_workers().await,
        _ => unreachable!(),
    };
    cleanup(&*storage).await;

    let created = storage
        .create_task(&sample_task("task with session"))
        .await
        .expect("create_task");
    let session = storage
        .start_work_session(
            &StartWorkSession {
                task_id: Some(created.id.clone()),
                ..Default::default()
            },
            None,
        )
        .await
        .expect("start_work_session");

    storage
        .delete_task(&created.id)
        .await
        .expect("delete_task");

    let stopped = storage
        .get_work_session(&session.id)
        .await
        .expect("get_work_session");
    assert!(stopped.task_id.is_none());
    assert!(stopped.ended_at.is_some());
}

#[rstest]
#[case::sqlite("sqlite")]
#[case::workers("workers")]
#[tokio::test]
#[ignore]
async fn delete_habit_closes_open_work_session(#[case] backend: &str) {
    let storage = match backend {
        "sqlite" => setup_sqlite().await,
        "workers" => setup_workers().await,
        _ => unreachable!(),
    };
    cleanup(&*storage).await;

    let habit = storage
        .create_habit(&CreateHabit {
            title: "habit with session".into(),
            description: None,
            recurrence: r#"{"freq":"daily","interval":1,"by_day":[],"by_month":[],"by_month_day":[],"count":null,"exdates":[]}"#.into(),
            start_time: "09:00".parse().unwrap(),
            end_time: "10:00".parse().unwrap(),
            avg_minutes: 30,
            sigma_minutes: None,
            parallelizable: None,
            allows_parallel: None,
            abandonability: None,
            fixed: None,
            window_mode: None,
        })
        .await
        .expect("create_habit");
    let task = storage
        .create_task(&CreateTask {
            habit_id: Some(habit.id.clone()),
            ..sample_task("generated habit task")
        })
        .await
        .expect("create_task");
    let session = storage
        .start_work_session(
            &StartWorkSession {
                task_id: Some(task.id.clone()),
                ..Default::default()
            },
            None,
        )
        .await
        .expect("start_work_session");

    storage
        .delete_habit(&habit.id)
        .await
        .expect("delete_habit");

    assert!(matches!(
        storage.get_task(&task.id).await,
        Err(StorageError::NotFound(_))
    ));
    let stopped = storage
        .get_work_session(&session.id)
        .await
        .expect("get_work_session");
    assert!(stopped.task_id.is_none());
    assert!(stopped.ended_at.is_some());
}

#[rstest]
#[case::sqlite("sqlite")]
#[case::workers("workers")]
#[tokio::test]
#[ignore]
async fn zero_quantity_treated_as_unset(#[case] backend: &str) {
    let storage = match backend {
        "sqlite" => setup_sqlite().await,
        "workers" => setup_workers().await,
        _ => unreachable!(),
    };
    cleanup(&*storage).await;

    let created = storage
        .create_task(&CreateTask {
            title: "zero-total".into(),
            quantity_total: Some(Quantity::default()),
            quantity_done: None,
            quantity_unit: None,
            original_quantity_total: Some(Quantity::default()),
            ..sample_task("zero-total")
        })
        .await
        .expect("create_task");
    assert!(created.quantity_total.is_none());
    assert!(created.original_quantity_total.is_none());
    assert_eq!(created.quantity_done, 0);
}

#[rstest]
#[case::sqlite("sqlite")]
#[case::workers("workers")]
#[tokio::test]
#[ignore]
async fn verify_root_token(#[case] backend: &str) {
    let storage = match backend {
        "sqlite" => setup_sqlite().await,
        "workers" => setup_workers().await,
        _ => unreachable!(),
    };
    cleanup(&*storage).await;

    let claims = storage
        .verify_token(root_token())
        .await
        .expect("verify_token")
        .expect("root token should resolve to claims");
    assert!(claims.is_root());
}

#[rstest]
#[case::sqlite("sqlite")]
#[case::workers("workers")]
#[tokio::test]
#[ignore]
async fn health_check(#[case] backend: &str) {
    let storage = match backend {
        "sqlite" => setup_sqlite().await,
        "workers" => setup_workers().await,
        _ => unreachable!(),
    };
    let status = storage.health_check().await.expect("health_check");
    assert!(!status.is_empty());
}

#[rstest]
#[case::sqlite("sqlite")]
#[case::workers("workers")]
#[tokio::test]
#[ignore]
async fn task_lifecycle_and_tokens(#[case] backend: &str) {
    let storage = match backend {
        "sqlite" => setup_sqlite().await,
        "workers" => setup_workers().await,
        _ => unreachable!(),
    };
    cleanup(&*storage).await;

    // root token verifies, bogus token does not
    assert!(
        storage
            .verify_token(root_token())
            .await
            .expect("verify_token")
            .is_some()
    );
    assert!(
        storage
            .verify_token("tsk_bogus")
            .await
            .expect("verify_token bogus")
            .is_none()
    );

    // create → list → get → update → delete
    let created = storage
        .create_task(&CreateTask {
            title: "e2e task".into(),
            description: Some("integration test".into()),
            start_at: Some("2026-06-05T09:00:00+09:00".parse().unwrap()),
            end_at: "2026-06-05T18:00:00+09:00".parse().unwrap(),
            avg_minutes: 60,
            sigma_minutes: Some(15),
            depends: Some(vec![]),
            parallelizable: Some(false),
            allows_parallel: Some(false),
            abandonability: Some(0.3.into()),
            ..sample_task("e2e task")
        })
        .await
        .expect("create_task");
    assert_eq!(created.title, "e2e task");
    assert_eq!(created.status, TaskStatus::Pending);
    let id = created.id.clone();

    let rows = storage
        .list_tasks(&TaskQuery {
            status: Some(TaskStatusFilter::Pending),
            ..Default::default()
        })
        .await
        .expect("list_tasks");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, id);

    let fetched = storage.get_task(&id).await.expect("get_task");
    assert_eq!(fetched.id, id);

    let err = storage
        .get_task("00000000-0000-0000-0000-000000000000")
        .await;
    assert!(matches!(err, Err(StorageError::NotFound(_))));

    let updated = storage
        .update_task(
            &id,
            &UpdateTask {
                title: Some("e2e task updated".into()),
                ..Default::default()
            },
        )
        .await
        .expect("update_task");
    assert_eq!(updated.title, "e2e task updated");

    storage.delete_task(&id).await.expect("delete_task");
    let after = storage.get_task(&id).await;
    assert!(matches!(after, Err(StorageError::NotFound(_))));

    // token create → list → revoke
    let resp = storage
        .create_token(Some("e2e"))
        .await
        .expect("create_token");
    assert!(resp.token.starts_with("eyJ"));
    let tokens = storage.list_tokens().await.expect("list_tokens");
    assert_eq!(tokens.len(), 1);
    storage.revoke_token(resp.id).await.expect("revoke_token");
    let tokens_after = storage.list_tokens().await.expect("list_tokens after");
    assert!(tokens_after[0].revoked_at.is_some());
}

#[rstest]
#[case::sqlite("sqlite")]
#[case::workers("workers")]
#[tokio::test]
#[ignore]
async fn habit_uuid_existence(#[case] backend: &str) {
    let storage = match backend {
        "sqlite" => setup_sqlite().await,
        "workers" => setup_workers().await,
        _ => unreachable!(),
    };
    cleanup(&*storage).await;

    let habit = storage
        .create_habit(&CreateHabit {
            title: "test habit".into(),
            description: None,
            recurrence: r#"{"freq":"daily","interval":1,"by_day":[],"by_month":[],"by_month_day":[],"count":null,"exdates":[]}"#.into(),
            start_time: "09:00".parse().unwrap(),
            end_time: "10:00".parse().unwrap(),
            avg_minutes: 30,
            sigma_minutes: None,
            parallelizable: None,
            allows_parallel: None,
            abandonability: None,
            fixed: None,
            window_mode: None,
        })
        .await
        .expect("create_habit");
    let id = habit.id.clone();

    // Full UUID of an existing habit resolves fine.
    let fetched = storage.get_habit(&id).await.expect("get_habit");
    assert_eq!(fetched.id, id);

    // Non-existent full UUID → NotFound (not a silent empty result).
    let bogus = "00000000-0000-0000-0000-000000000000";
    let err = storage.get_habit(bogus).await;
    assert!(matches!(err, Err(StorageError::NotFound(_))));

    // Scheduled-spans listing on a non-existent habit UUID → NotFound.
    let spans_err = storage.list_habit_scheduled_spans(bogus).await;
    assert!(matches!(spans_err, Err(StorageError::NotFound(_))));

    // Creating a scheduled span on a non-existent habit UUID → NotFound.
    let create_span_err = storage
        .create_habit_scheduled_span(
            bogus,
            &CreateHabitScheduledSpan {
                start_date: takusu_types::Date::new(2026, 7, 1).unwrap(),
                end_date: takusu_types::Date::new(2026, 7, 2).unwrap(),
                reason: None,
            },
        )
        .await;
    assert!(matches!(create_span_err, Err(StorageError::NotFound(_))));
}

#[rstest]
#[case::sqlite("sqlite")]
#[case::workers("workers")]
#[tokio::test]
#[ignore]
async fn memory_crud(#[case] backend: &str) {
    let storage = match backend {
        "sqlite" => setup_sqlite().await,
        "workers" => setup_workers().await,
        _ => unreachable!(),
    };
    cleanup(&*storage).await;

    let create = CreateMemory {
        kind: MemoryKind::ProperNoun,
        key: "研究室".into(),
        content: "大学の研究室".into(),
        subject_type: None,
        subject_id: None,
        upsert: false,
    };
    let row = storage
        .create_memory(&create, None)
        .await
        .expect("create_memory");
    let id = row.id.clone();
    assert_eq!(row.key, "研究室");

    let fetched = storage.get_memory(&id).await.expect("get_memory");
    assert_eq!(fetched.id, id);

    let updated = storage
        .update_memory(
            &id,
            &UpdateMemory {
                observed_revision: row.revision,
                content: Some("大学の研究室（更新）".into()),
            },
            None,
        )
        .await
        .expect("update_memory");
    assert_eq!(updated.content, "大学の研究室（更新）");

    let found = storage
        .search_memories(&MemoryQuery {
            q: "研究室".into(),
            kind: None,
            subject_type: None,
            subject_id: None,
            limit: Some(10),
        })
        .await
        .expect("search_memories");
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].id, id);

    storage
        .delete_memory(&id, updated.revision, None)
        .await
        .expect("delete_memory");
    let after = storage.get_memory(&id).await;
    assert!(matches!(after, Err(StorageError::NotFound(_))));
}

#[rstest]
#[case::sqlite("sqlite")]
#[case::workers("workers")]
#[tokio::test]
#[ignore]
async fn comment_crud(#[case] backend: &str) {
    let storage = match backend {
        "sqlite" => setup_sqlite().await,
        "workers" => setup_workers().await,
        _ => unreachable!(),
    };
    cleanup(&*storage).await;

    let task = storage
        .create_task(&sample_task("comment target"))
        .await
        .expect("create task");

    // Append comments with different authors; seq must be assigned in order.
    let c1 = storage
        .create_comment(&task.id, CommentAuthor::User, "first", None)
        .await
        .expect("create user comment");
    let c2 = storage
        .create_comment(&task.id, CommentAuthor::Agent, "second", None)
        .await
        .expect("create agent comment");
    let c3 = storage
        .create_comment(&task.id, CommentAuthor::User, "third", None)
        .await
        .expect("create user comment");
    assert_eq!(c1.seq, 1);
    assert_eq!(c2.seq, 2);
    assert_eq!(c3.seq, 3);

    // Author is stored as passed (server-assigned at the endpoint boundary).
    assert_eq!(c1.author, CommentAuthor::User);
    assert_eq!(c2.author, CommentAuthor::Agent);
    assert_eq!(c3.author, CommentAuthor::User);

    // List returns ascending seq order.
    let all = storage
        .list_comments(&task.id)
        .await
        .expect("list_comments");
    let seqs: Vec<i64> = all.iter().map(|c| c.seq).collect();
    assert_eq!(seqs, vec![1, 2, 3]);
    let contents: Vec<&str> = all.iter().map(|c| c.content.as_str()).collect();
    assert_eq!(contents, vec!["first", "second", "third"]);

    // delete_comment removes one row.
    storage
        .delete_comment(&c2.id)
        .await
        .expect("delete_comment");
    let remaining = storage
        .list_comments(&task.id)
        .await
        .expect("list_comments after delete");
    assert_eq!(remaining.len(), 2);

    // Deleting a missing comment is NotFound.
    let missing = storage.delete_comment(&c2.id).await;
    assert!(matches!(missing, Err(StorageError::NotFound(_))));

    // Cascade: deleting the task removes its comments.
    storage
        .delete_task(&task.id)
        .await
        .expect("delete task");
    let gone = storage.delete_comment(&c3.id).await;
    assert!(matches!(gone, Err(StorageError::NotFound(_))));
}

#[rstest]
#[case::sqlite("sqlite")]
#[case::workers("workers")]
#[tokio::test]
#[ignore]
async fn comment_idempotency_replay(#[case] backend: &str) {
    let storage = match backend {
        "sqlite" => setup_sqlite().await,
        "workers" => setup_workers().await,
        _ => unreachable!(),
    };
    cleanup(&*storage).await;

    let task = storage
        .create_task(&sample_task("idempotent comment"))
        .await
        .expect("create task");
    let op = "op-comment-1".to_string();

    let first = storage
        .create_comment(&task.id, CommentAuthor::User, "hello", Some(&op))
        .await
        .expect("first create_comment");
    let replay = storage
        .create_comment(&task.id, CommentAuthor::User, "hello", Some(&op))
        .await
        .expect("replay create_comment");
    assert_eq!(replay.id, first.id);
    assert_eq!(replay.seq, first.seq);

    // Only one row was created despite two calls.
    let all = storage
        .list_comments(&task.id)
        .await
        .expect("list_comments");
    assert_eq!(all.len(), 1);
}

#[rstest]
#[case::sqlite("sqlite")]
#[case::workers("workers")]
#[tokio::test]
#[ignore]
async fn comment_unknown_task_not_found(#[case] backend: &str) {
    let storage = match backend {
        "sqlite" => setup_sqlite().await,
        "workers" => setup_workers().await,
        _ => unreachable!(),
    };
    cleanup(&*storage).await;

    let err = storage
        .create_comment(
            "does-not-exist",
            CommentAuthor::User,
            "no task",
            None,
        )
        .await;
    assert!(matches!(err, Err(StorageError::NotFound(_))));
}

/// Fire many same-key create_comment calls concurrently and assert exactly one
/// comment is persisted. This exercises the D1 batch + replay path (WI-1 review
/// issue 1): without atomicity, a concurrent loser would leave a stray comment.
/// Requires the worker backend, so it is `#[ignore]`d like the other suite
/// tests.
#[tokio::test]
#[ignore]
async fn comment_concurrent_same_key_worker_idempotent() {
    let storage = setup_workers().await;
    cleanup(&*storage).await;

    let task = storage
        .create_task(&sample_task("concurrent comments"))
        .await
        .expect("create task");
    let op = "op-comment-concurrent".to_string();

    let mut handles = Vec::new();
    for _ in 0..8 {
        let storage = storage.clone();
        let tid = task.id.clone();
        let op = op.clone();
        handles.push(tokio::spawn(async move {
            storage
                .create_comment(&tid, CommentAuthor::User, "same", Some(&op))
                .await
        }));
    }
    for h in handles {
        let r = h.await.expect("task handle");
        assert!(
            r.is_ok(),
            "concurrent same-key create_comment must not error: {r:?}"
        );
    }

    let all = storage
        .list_comments(&task.id)
        .await
        .expect("list_comments");
    assert_eq!(
        all.len(),
        1,
        "same-key concurrent creates must produce exactly one comment"
    );
}

#[rstest]
#[case::sqlite("sqlite")]
#[case::workers("workers")]
#[tokio::test]
#[ignore]
async fn list_tasks_no_overdue_filter(#[case] backend: &str) {
    let storage = match backend {
        "sqlite" => setup_sqlite().await,
        "workers" => setup_workers().await,
        _ => unreachable!(),
    };
    cleanup(&*storage).await;

    let overdue = CreateTask {
        title: "overdue task".into(),
        end_at: "2020-01-01T00:00:00+00:00".parse().unwrap(),
        avg_minutes: 10,
        sigma_minutes: Some(2),
        depends: Some(vec![]),
        parallelizable: Some(false),
        allows_parallel: Some(false),
        abandonability: Some(0.5.into()),
        ..sample_task("overdue task")
    };
    let future = CreateTask {
        title: "future task".into(),
        end_at: "2030-01-01T00:00:00+00:00".parse().unwrap(),
        ..overdue.clone()
    };
    storage.create_task(&overdue).await.expect("create overdue");
    storage.create_task(&future).await.expect("create future");

    let tasks = storage
        .list_tasks(&TaskQuery {
            no_overdue: Some(true),
            ..Default::default()
        })
        .await
        .expect("list_tasks");
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].title, "future task");
}
