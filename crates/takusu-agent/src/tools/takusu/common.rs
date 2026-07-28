use serde_json::{Value, json};
use std::collections::{HashMap, HashSet, VecDeque};
use std::str::FromStr;
use takusu_client::{Client, HabitDetail, HabitRow, HabitStepRow, TaskRow};
use takusu_util::{TaskStatus, Timestamp, parse_datetime_to_timestamp};

use crate::{InvalidArgsError, ToolError};

pub(crate) fn optional_string(
    args: &serde_json::Map<String, Value>,
    name: &str,
) -> Result<Option<String>, ToolError> {
    match args.get(name) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value
            .as_str()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| Some(value.to_owned()))
            .ok_or_else(|| ToolError::InvalidArgs(InvalidArgsError::new(name, "must be a string"))),
    }
}

pub(super) fn summary_string(args: &serde_json::Map<String, Value>, name: &str) -> Option<String> {
    args.get(name)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

pub(crate) fn client_error(error: takusu_client::ClientError) -> ToolError {
    match error {
        takusu_client::ClientError::Api {
            status: 400..=499,
            body,
        } => {
            if body.contains("not found") || body.contains("Not found") {
                ToolError::NotFound(body)
            } else {
                ToolError::InvalidArgs(InvalidArgsError::no_field(body))
            }
        }
        error => ToolError::Other(Box::new(error)),
    }
}

/// Cache for the configured timezone, shared across tools in a session.
///
/// Successful `get_settings()` calls are cached for the lifetime of the
/// session. Failures are backed off (currently 30 seconds) to avoid hammering
/// the server when it is temporarily unreachable, and callers fall back to the
/// system timezone.
#[derive(Clone)]
pub struct TimeZoneCache {
    client: Client,
    state: std::sync::Arc<tokio::sync::Mutex<CacheState>>,
}

#[derive(Clone)]
enum CacheState {
    Empty,
    Ok(jiff::tz::TimeZone),
    Failed(std::time::Instant),
}

impl TimeZoneCache {
    pub fn new(client: Client) -> Self {
        Self {
            client,
            state: std::sync::Arc::new(tokio::sync::Mutex::new(CacheState::Empty)),
        }
    }

    /// Return the configured timezone, falling back to the system timezone on
    /// any failure.
    pub async fn get_with_fallback(&self) -> jiff::tz::TimeZone {
        const FAILURE_TTL: std::time::Duration = std::time::Duration::from_secs(30);
        let mut state = self.state.lock().await;
        match &*state {
            CacheState::Ok(tz) => return tz.clone(),
            CacheState::Failed(at) if at.elapsed() < FAILURE_TTL => {
                return jiff::Zoned::now().time_zone().clone();
            }
            _ => {}
        }

        match self.load_timezone().await {
            Ok(tz) => {
                *state = CacheState::Ok(tz.clone());
                tz
            }
            Err(_) => {
                *state = CacheState::Failed(std::time::Instant::now());
                jiff::Zoned::now().time_zone().clone()
            }
        }
    }

    async fn load_timezone(&self) -> Result<jiff::tz::TimeZone, ToolError> {
        let settings = self.client.get_settings().await.map_err(client_error)?;
        jiff::tz::TimeZone::get(&settings.tz).map_err(|error| ToolError::Other(Box::new(error)))
    }
}

pub(crate) async fn server_timezone(cache: &TimeZoneCache) -> jiff::tz::TimeZone {
    cache.get_with_fallback().await
}

/// Format a stored datetime string for display in the configured timezone.
///
/// Stored task/schedule datetimes are always UTC, but `datetime('now')`
/// returns a space-separated naive string (`YYYY-MM-DD HH:MM:SS`).
/// Standard RFC 3339 / ISO 8601 strings with `T`, `Z`, or an offset are
/// parsed as absolute timestamps. Naive strings matching the SQLite format
/// (with a space or `T`) are interpreted as UTC wall-clock times.
/// Returns the original string unchanged if parsing fails.
pub(crate) fn format_datetime_for_display(s: &str, tz: &jiff::tz::TimeZone) -> String {
    if s.is_empty() {
        return s.to_string();
    }
    let s = s.trim();
    if let Ok(ts) = jiff::Timestamp::from_str(s) {
        return ts.to_zoned(tz.clone()).to_string();
    }
    // SQLite `datetime('now')` and other naive UTC wall-clock formats.
    for fmt in ["%Y-%m-%d %H:%M:%S", "%Y-%m-%dT%H:%M:%S"] {
        if let Ok(dt) = jiff::civil::DateTime::strptime(fmt, s)
            && let Ok(zdt) = dt.to_zoned(jiff::tz::TimeZone::UTC)
        {
            return zdt.timestamp().to_zoned(tz.clone()).to_string();
        }
    }
    s.to_string()
}

/// Returns true if the task is not completed/skipped and its `end_at` has
/// passed relative to the current time.
pub(super) fn is_overdue(task: &TaskRow, _tz: &jiff::tz::TimeZone) -> bool {
    if task.status == TaskStatus::Completed || task.status == TaskStatus::Skipped {
        return false;
    }
    let end = task.end_at.to_jiff();
    end < jiff::Timestamp::now()
}

/// Returns true if a schedule entry overlaps the optional [from, to] range.
///
/// Missing or unparseable timestamps are treated conservatively: if a bound
/// required to verify overlap is unavailable, the entry is excluded from
/// ranged results.
pub(super) fn entry_in_range(
    entry: &Value,
    from: Option<jiff::Timestamp>,
    to: Option<jiff::Timestamp>,
    tz: &jiff::tz::TimeZone,
) -> bool {
    if from.is_none() && to.is_none() {
        return true;
    }

    if let (Some(from), Some(to)) = (from, to)
        && from > to
    {
        return false;
    }

    let parse = |v: Option<&str>| {
        v.and_then(|s| {
            jiff::Timestamp::from_str(s)
                .ok()
                .or_else(|| parse_datetime_to_timestamp(s, tz).ok())
        })
    };
    let entry_start = parse(entry.get("start_at").and_then(Value::as_str));
    let entry_end = parse(entry.get("end_at").and_then(Value::as_str));

    match (entry_start, entry_end) {
        (Some(start), Some(end)) => {
            if let Some(to) = to
                && start > to
            {
                return false;
            }
            if let Some(from) = from
                && end < from
            {
                return false;
            }
            true
        }
        (None, Some(end)) => {
            // The entry ends at `end`; without a start we can only exclude
            // entries that definitely fall outside the range.
            if let Some(from) = from
                && end < from
            {
                return false;
            }
            if let Some(to) = to
                && end > to
            {
                return false;
            }
            true
        }
        (Some(start), None) => {
            // The entry starts at `start`; without an end we can only exclude
            // entries that definitely fall outside the range.
            if let Some(to) = to
                && start > to
            {
                return false;
            }
            if let Some(from) = from
                && start < from
            {
                return false;
            }
            true
        }
        (None, None) => false,
    }
}

/// Returns true if an overdue task's deadline falls inside the optional range.
pub(super) fn overdue_in_range(
    task: &TaskRow,
    from: Option<jiff::Timestamp>,
    to: Option<jiff::Timestamp>,
    _tz: &jiff::tz::TimeZone,
) -> bool {
    if let (Some(from), Some(to)) = (from, to)
        && from > to
    {
        return false;
    }

    let end = task.end_at.to_jiff();
    if let Some(from) = from
        && end < from
    {
        return false;
    }
    if let Some(to) = to
        && end > to
    {
        return false;
    }
    true
}

/// Convert any absolute datetime fields in the display `args` map from the
/// canonical UTC representation back to the configured timezone.
///
/// Leaves `execution_args` untouched so the backend still receives UTC.
pub(super) fn format_display_datetime_args(
    args: &mut serde_json::Map<String, Value>,
    tz: &jiff::tz::TimeZone,
) {
    for key in ["start_at", "end_at", "from", "until"] {
        if let Some(Value::String(s)) = args.get(key) {
            args.insert(
                key.to_string(),
                Value::String(format_datetime_for_display(s, tz)),
            );
        }
    }
}

/// Strip a leading `#` from a user-supplied task reference.
/// Keeps habit-scoped references such as `h1#5` and raw UUIDs intact.
pub(crate) fn strip_leading_hash(reference: &str) -> &str {
    reference.strip_prefix('#').unwrap_or(reference)
}

/// Normalize a task status string to the canonical backend value.
/// Handles common LLM/user synonyms such as "done" -> "completed".
pub(crate) fn normalize_status(status: &str) -> String {
    let lower = status.trim().to_lowercase();
    match lower.as_str() {
        "done" | "complete" | "completed" => "completed".to_string(),
        "todo" | "to-do" | "to_do" | "pending" => "pending".to_string(),
        "in-progress" | "in_progress" | "inprogress" | "doing" | "in progress" => {
            "in_progress".to_string()
        }
        "skip" | "skipped" => "skipped".to_string(),
        "planned" | "scheduled" => "scheduled".to_string(),
        _ => lower,
    }
}

/// Normalize an array of task references by stripping leading `#` characters.
pub(super) fn normalize_reference_array(
    args: &mut serde_json::Map<String, Value>,
    key: &str,
) -> Result<(), ToolError> {
    if let Some(Value::Array(values)) = args.get_mut(key) {
        for value in values.iter_mut() {
            match value.as_str() {
                Some(reference) => {
                    *value = Value::String(strip_leading_hash(reference.trim()).to_string());
                }
                None => {
                    return Err(ToolError::InvalidArgs(InvalidArgsError::new(
                        key,
                        "must contain only strings",
                    )));
                }
            }
        }
    }
    Ok(())
}

#[derive(Debug, Clone)]
pub(crate) struct TaskRef {
    pub(crate) display_id: i64,
    pub(crate) reference: String,
    pub(crate) title: String,
}

#[derive(Debug, Clone)]
pub(crate) struct TaskContext {
    task_refs: HashMap<String, TaskRef>,
    habit_display_ids: HashMap<String, i64>,
}

impl TaskContext {
    pub(crate) fn new(tasks: &[TaskRow], habits: &[HabitRow]) -> Self {
        let habit_display_ids: HashMap<String, i64> = habits
            .iter()
            .map(|habit| (habit.id.clone(), habit.display_id))
            .collect();
        let task_refs: HashMap<String, TaskRef> = tasks
            .iter()
            .map(|task| {
                let reference = task_reference(task, &habit_display_ids);
                (
                    task.id.clone(),
                    TaskRef {
                        display_id: task.display_id,
                        reference,
                        title: task.title.clone(),
                    },
                )
            })
            .collect();
        Self {
            task_refs,
            habit_display_ids,
        }
    }

    pub(crate) fn ref_by_id(&self, id: &str) -> Option<&TaskRef> {
        self.task_refs.get(id)
    }

    pub(crate) fn reference(&self, task: &TaskRow) -> String {
        self.task_refs
            .get(&task.id)
            .map(|task_ref| task_ref.reference.clone())
            .unwrap_or_else(|| task_reference(task, &self.habit_display_ids))
    }

    pub(crate) fn depends(&self, task: &TaskRow) -> Vec<String> {
        task_dependency_ids(task)
            .into_iter()
            .filter_map(|id| self.task_refs.get(&id).map(|r| r.reference.clone()))
            .collect()
    }
}

pub(crate) fn task_reference(task: &TaskRow, habit_display_ids: &HashMap<String, i64>) -> String {
    task.habit_id
        .as_ref()
        .and_then(|habit_id| habit_display_ids.get(habit_id))
        .map(|habit_display_id| format!("h{habit_display_id}#{}", task.display_id))
        .unwrap_or_else(|| format!("#{}", task.display_id))
}

pub(crate) fn task_json(
    task: &TaskRow,
    ctx: &TaskContext,
    tz: Option<&jiff::tz::TimeZone>,
) -> Value {
    let fmt = |t: &Timestamp| match tz {
        Some(tz) => format_datetime_for_display(&t.to_string(), tz),
        None => t.to_string(),
    };
    let mut value = json!({
        "display_id": task.display_id,
        "reference": ctx.reference(task),
        "title": task.title,
        "description": task.description,
        "start_at": task.start_at.as_ref().map(fmt),
        "end_at": fmt(&task.end_at),
        "avg_minutes": task.avg_minutes,
        "sigma_minutes": task.sigma_minutes,
        "depends": ctx.depends(task),
        "parallelizable": task.parallelizable,
        "allows_parallel": task.allows_parallel,
        "abandonability": task.abandonability,
        "status": task.status,
        "fixed": task.fixed,
        "quantity_total": task.quantity_total,
        "quantity_done": task.quantity_done,
        "quantity_unit": task.quantity_unit,
        "completed_at": task.completed_at.as_ref().map(fmt),
        "split_from_task_id": task.split_from_task_id.as_deref().and_then(|id| ctx.ref_by_id(id).map(|r| r.reference.clone())),
        "original_quantity_total": task.original_quantity_total,
        "actual_minutes": task.actual_minutes,
        "created_at": fmt(&task.created_at),
        "updated_at": fmt(&task.updated_at),
    });
    if task.actual_minutes.is_none()
        && let Value::Object(map) = &mut value
    {
        map.remove("actual_minutes");
    }
    value
}

fn task_dependency_ids(task: &TaskRow) -> Vec<String> {
    serde_json::from_str::<Vec<String>>(&task.depends).unwrap_or_default()
}

/// Collect all transitive dependency tasks for `requested` from the full `all_tasks` list.
/// Returns dependency rows excluding the requested tasks themselves, sorted by display_id,
/// plus a list of any dependency IDs that were not found in `all_tasks`.
pub(super) fn transitive_dependencies<'a>(
    requested: &'a [TaskRow],
    all_tasks: &'a [TaskRow],
) -> (Vec<&'a TaskRow>, Vec<String>) {
    let by_id: HashMap<String, &TaskRow> = all_tasks.iter().map(|t| (t.id.clone(), t)).collect();
    let mut visited: HashSet<String> = HashSet::new();
    let mut missing: HashSet<String> = HashSet::new();
    let mut queue: VecDeque<String> = VecDeque::new();

    for task in requested {
        visited.insert(task.id.clone());
        for dep in task_dependency_ids(task) {
            queue.push_back(dep);
        }
    }

    while let Some(dep_id) = queue.pop_front() {
        if !visited.insert(dep_id.clone()) {
            continue;
        }
        if let Some(dep) = by_id.get(&dep_id) {
            for d in task_dependency_ids(dep) {
                queue.push_back(d);
            }
        } else {
            missing.insert(dep_id);
        }
    }

    let requested_ids: HashSet<String> = requested.iter().map(|t| t.id.clone()).collect();
    let mut deps: Vec<&TaskRow> = visited
        .into_iter()
        .filter(|id| !requested_ids.contains(id))
        .filter_map(|id| by_id.get(&id).copied())
        .collect();
    deps.sort_by_key(|t| t.display_id);
    let mut missing: Vec<String> = missing.into_iter().collect();
    missing.sort();
    (deps, missing)
}

pub(super) fn habit_summary_json(habit: &HabitRow) -> Value {
    json!({
        "display_id": habit.display_id,
        "reference": format!("h{}", habit.display_id),
        "title": habit.title,
        "description": habit.description,
        "recurrence": habit.recurrence,
        "start_time": habit.start_time,
        "end_time": habit.end_time,
        "avg_minutes": habit.avg_minutes,
        "sigma_minutes": habit.sigma_minutes,
        "parallelizable": habit.parallelizable,
        "allows_parallel": habit.allows_parallel,
        "abandonability": habit.abandonability,
        "active": habit.active,
        "fixed": habit.fixed,
        "window_mode": habit.window_mode,
    })
}

pub(super) fn habit_json(habit: &HabitDetail) -> Value {
    // Positions are exposed to the client as 1-indexed display numbers.
    let id_to_display_position: HashMap<String, i64> = habit
        .steps
        .iter()
        .map(|s| (s.id.clone(), s.position + 1))
        .collect();
    let has_steps = !habit.steps.is_empty();
    let mut value = serde_json::Map::new();
    value.insert("display_id".into(), json!(habit.habit.display_id));
    value.insert(
        "reference".into(),
        json!(format!("h{}", habit.habit.display_id)),
    );
    value.insert("title".into(), json!(habit.habit.title));
    value.insert("description".into(), json!(habit.habit.description));
    value.insert("recurrence".into(), json!(habit.habit.recurrence));
    // When a habit has steps, the scheduler uses per-step values for timing,
    // cost, and behavioral flags, so omit the habit-level fields that would
    // otherwise be ignored to avoid misleading the agent (#1084).
    if !has_steps {
        value.insert("start_time".into(), json!(habit.habit.start_time));
        value.insert("end_time".into(), json!(habit.habit.end_time));
        value.insert("avg_minutes".into(), json!(habit.habit.avg_minutes));
        value.insert("sigma_minutes".into(), json!(habit.habit.sigma_minutes));
        value.insert("parallelizable".into(), json!(habit.habit.parallelizable));
        value.insert("allows_parallel".into(), json!(habit.habit.allows_parallel));
        value.insert("abandonability".into(), json!(habit.habit.abandonability));
    }
    value.insert("active".into(), json!(habit.habit.active));
    value.insert("fixed".into(), json!(habit.habit.fixed));
    value.insert("window_mode".into(), json!(habit.habit.window_mode));
    value.insert(
        "steps".into(),
        json!(
            habit
                .steps
                .iter()
                .map(|s| step_json(s, &id_to_display_position))
                .collect::<Vec<_>>()
        ),
    );
    Value::Object(value)
}

fn step_json(step: &HabitStepRow, id_to_display_position: &HashMap<String, i64>) -> Value {
    let depends_on: Vec<i64> = serde_json::from_str::<Vec<String>>(&step.depends_on)
        .unwrap_or_default()
        .iter()
        .filter_map(|id| id_to_display_position.get(id).copied())
        .collect();
    json!({
        "position": step.position + 1,
        "title": step.title,
        "description": step.description,
        "start_time": step.start_time,
        "end_time": step.end_time,
        "avg_minutes": step.avg_minutes,
        "sigma_minutes": step.sigma_minutes,
        "parallelizable": step.parallelizable,
        "allows_parallel": step.allows_parallel,
        "abandonability": step.abandonability,
        "fixed": step.fixed,
        "depends_on": depends_on,
    })
}

pub(super) fn schedule_entry_value(
    entry: &Value,
    ctx: &TaskContext,
    tz: Option<&jiff::tz::TimeZone>,
) -> Value {
    let task_id = entry.get("task_id").and_then(Value::as_str).unwrap_or("");
    let (reference, display_id, title) = match ctx.ref_by_id(task_id) {
        Some(r) => (
            Value::String(r.reference.clone()),
            json!(r.display_id),
            Value::String(r.title.clone()),
        ),
        None => (
            Value::String("unknown".into()),
            Value::Null,
            Value::String("unknown task".into()),
        ),
    };
    let fmt = |s: &str| match tz {
        Some(tz) => format_datetime_for_display(s, tz),
        None => s.to_string(),
    };
    json!({
        "reference": reference,
        "display_id": display_id,
        "title": title,
        "start_at": entry.get("start_at").and_then(Value::as_str).map(fmt),
        "end_at": entry.get("end_at").and_then(Value::as_str).map(fmt),
    })
}

fn reference_value(id: &str, ctx: &TaskContext) -> Value {
    ctx.ref_by_id(id)
        .map(|r| Value::String(r.reference.clone()))
        .unwrap_or_else(|| Value::String("unknown".into()))
}

pub(super) fn transform_preview(
    preview: Value,
    ctx: &TaskContext,
    tz: Option<&jiff::tz::TimeZone>,
) -> Value {
    let mut out = preview.as_object().cloned().unwrap_or_default();

    if let Some(Value::Array(entries)) = out.get("entries").cloned() {
        let transformed = entries
            .iter()
            .map(|entry| schedule_entry_value(entry, ctx, tz))
            .collect::<Vec<_>>();
        out.insert("entries".into(), Value::Array(transformed));
    }

    for key in ["unscheduled_task_ids", "displaced_task_ids"] {
        if let Some(Value::Array(ids)) = out.get(key).cloned() {
            let transformed = ids
                .iter()
                .map(|id| {
                    id.as_str()
                        .map(|s| reference_value(s, ctx))
                        .unwrap_or_else(|| Value::String("unknown".into()))
                })
                .collect::<Vec<_>>();
            out.insert(key.into(), Value::Array(transformed));
        }
    }

    Value::Object(out)
}
