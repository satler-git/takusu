use async_trait::async_trait;
use futures_util::future::try_join_all;
use jiff::civil::Date;
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet, VecDeque};
use std::str::FromStr;
use std::sync::Arc;
use takusu_client::{
    Client, HabitDetail, HabitRow, HabitScheduledSpanRow, HabitStepRow, SchedulePreviewRequest,
    TaskQuery, TaskRow,
};
use takusu_util::{parse_date_expression, parse_datetime_to_timestamp, parse_datetime_tz};

use crate::{InvalidArgsError, Tool, ToolError, ToolOutput, ToolRegistry, UserInputProvider};

/// Registers planner read tools, approval-only mutation proposals, and the ASR
/// correction tool.
pub fn register_tools(
    registry: &mut ToolRegistry,
    client: Client,
    tz_cache: TimeZoneCache,
    user_input_provider: Arc<dyn UserInputProvider>,
) {
    register_read_tools(registry, client.clone(), tz_cache.clone());
    register_mutation_tools(registry, client.clone(), tz_cache.clone());
    crate::tools::progress::register_tools(registry, client.clone(), tz_cache.clone());
    crate::tools::rrule::register_tools(registry, tz_cache.clone());
    crate::tools::day_details::register_tools(registry, client.clone(), tz_cache.clone());
    crate::tools::memory::register_tools(registry, client.clone());
    registry.register(Box::new(PreviewScheduleTool {
        client: client.clone(),
        tz_cache: tz_cache.clone(),
    }));
    registry.register(Box::new(MoveTaskTool {
        client: client.clone(),
        tz_cache,
    }));
    crate::tools::skills::register_tools(registry, client.clone());
    crate::tools::user_input::register_user_input_tool(registry, user_input_provider);
}

/// Registers the read-only planner tools used by the agent.
pub fn register_read_tools(registry: &mut ToolRegistry, client: Client, tz_cache: TimeZoneCache) {
    registry.register(Box::new(ListTasks {
        client: client.clone(),
        tz_cache: tz_cache.clone(),
    }));
    registry.register(Box::new(GetTask {
        client: client.clone(),
        tz_cache: tz_cache.clone(),
    }));
    registry.register(Box::new(ListHabits {
        client: client.clone(),
    }));
    registry.register(Box::new(GetHabit {
        client: client.clone(),
    }));
    registry.register(Box::new(GetSchedule {
        client: client.clone(),
        tz_cache,
    }));
    registry.register(Box::new(GetSettings { client }));
}

pub(crate) fn object(args: Value) -> Result<serde_json::Map<String, Value>, ToolError> {
    args.as_object()
        .cloned()
        .ok_or_else(|| ToolError::InvalidArgs(InvalidArgsError::new("args", "must be an object")))
}

pub(crate) fn required_string(
    args: &serde_json::Map<String, Value>,
    name: &str,
) -> Result<String, ToolError> {
    args.get(name)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .ok_or_else(|| ToolError::InvalidArgs(InvalidArgsError::new(name, "missing or empty")))
}

pub(crate) fn required_i64(
    args: &serde_json::Map<String, Value>,
    name: &str,
) -> Result<i64, ToolError> {
    args.get(name)
        .and_then(Value::as_i64)
        .ok_or_else(|| ToolError::InvalidArgs(InvalidArgsError::new(name, "missing or invalid")))
}

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

pub(crate) fn optional_bool(
    args: &serde_json::Map<String, Value>,
    name: &str,
) -> Result<Option<bool>, ToolError> {
    match args.get(name) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value.as_bool().map(Some).ok_or_else(|| {
            ToolError::InvalidArgs(InvalidArgsError::new(name, "must be a boolean"))
        }),
    }
}

pub(crate) fn optional_i64(
    args: &serde_json::Map<String, Value>,
    name: &str,
) -> Result<Option<i64>, ToolError> {
    match args.get(name) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value.as_i64().map(Some).ok_or_else(|| {
            ToolError::InvalidArgs(InvalidArgsError::new(name, "must be an integer"))
        }),
    }
}

fn summary_string(args: &serde_json::Map<String, Value>, name: &str) -> Option<String> {
    args.get(name)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

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

fn normalize_datetime(
    value: Option<String>,
    tz: &jiff::tz::TimeZone,
    name: &str,
) -> Result<Option<String>, ToolError> {
    let Some(value) = value else { return Ok(None) };
    parse_datetime_tz(&value, tz).map(Some).map_err(|error| {
        ToolError::InvalidArgs(InvalidArgsError::new(name, format!("invalid: {error}")))
    })
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
fn is_overdue(task: &TaskRow, tz: &jiff::tz::TimeZone) -> bool {
    if task.status == "completed" || task.status == "skipped" {
        return false;
    }
    let Ok(end) = parse_datetime_to_timestamp(&task.end_at, tz) else {
        return false;
    };
    end < jiff::Timestamp::now()
}

/// Returns true if a schedule entry overlaps the optional [from, to] range.
///
/// Missing or unparseable timestamps are treated conservatively: if a bound
/// required to verify overlap is unavailable, the entry is excluded from
/// ranged results.
fn entry_in_range(
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
fn overdue_in_range(
    task: &TaskRow,
    from: Option<jiff::Timestamp>,
    to: Option<jiff::Timestamp>,
    tz: &jiff::tz::TimeZone,
) -> bool {
    if let (Some(from), Some(to)) = (from, to)
        && from > to
    {
        return false;
    }

    let Ok(end) = parse_datetime_to_timestamp(&task.end_at, tz) else {
        // Cannot verify the deadline; fail closed.
        return false;
    };
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
fn format_display_datetime_args(
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
fn normalize_reference_array(
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
    display_id: i64,
    reference: String,
    title: String,
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
    let fmt = |s: &str| match tz {
        Some(tz) => format_datetime_for_display(s, tz),
        None => s.to_string(),
    };
    let mut value = json!({
        "display_id": task.display_id,
        "reference": ctx.reference(task),
        "title": task.title,
        "description": task.description,
        "start_at": task.start_at.as_deref().map(fmt),
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
        "completed_at": task.completed_at.as_deref().map(fmt),
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
fn transitive_dependencies<'a>(
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

fn habit_summary_json(habit: &HabitRow) -> Value {
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

fn habit_json(habit: &HabitDetail) -> Value {
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

fn schedule_entry_value(
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

fn transform_preview(preview: Value, ctx: &TaskContext, tz: Option<&jiff::tz::TimeZone>) -> Value {
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

struct ListTasks {
    client: Client,
    tz_cache: TimeZoneCache,
}

#[async_trait]
impl Tool for ListTasks {
    fn name(&self) -> &'static str {
        "list_tasks"
    }
    fn description(&self) -> &'static str {
        "List tasks, optionally filtered by status, time range, or habit."
    }
    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "status": {
                    "type": "string",
                    "enum": ["pending", "scheduled", "in_progress", "completed", "skipped", "overdue"],
                    "description": "Task status filter. Use 'completed' for done tasks, 'overdue' for tasks whose end_at has passed but are not completed or skipped."
                },
                "from": {"type": "string", "description": "Start of range; interpreted in server timezone."},
                "until": {"type": "string", "description": "End of range; interpreted in server timezone."},
                "no_overdue": {"type": "boolean", "description": "If true, exclude tasks whose end_at has passed. Do not use together with status='overdue'."},
                "habit_id": {"type": "string", "description": "Habit reference such as h1."},
                "q": {"type": "string", "description": "Free-form search query with qualifiers. Boolean: AND (space/AND), OR, -/NOT. Parentheses supported. Qualifiers: status:<pending|scheduled|in_progress|completed|skipped|overdue>, title:<text>, desc:<text>, start:<date>, end:<date>, scheduled-start:<date>, scheduled-end:<date>, from:<date> (alias end:>=), until:<date> (alias start:<=), habit:<hN|N>, depends:<#N|UUID>, dependents:<#N|UUID>, deps_count:<op>N, is:<overdue|fixed|parallelizable|allows_parallel>, has:<description|completed_at|schedule|depends>. Date: YYYY-MM-DD, today, tomorrow, yesterday, Nd (relative), or operators like >=2026-07-25. Examples: status:pending 買い物, end:today OR end:tomorrow, -habit:h1, depends:#42"},
                "limit": {"type": "integer", "description": "Maximum number of tasks to return."},
            },
            "additionalProperties": false,
        })
    }
    async fn call(&self, args: Value) -> Result<ToolOutput, ToolError> {
        let args = object(args)?;
        let habit_ref = optional_string(&args, "habit_id")?;
        let habit = match habit_ref {
            Some(reference) => Some(
                self.client
                    .get_habit(&reference)
                    .await
                    .map_err(client_error)?,
            ),
            None => None,
        };
        let tz = server_timezone(&self.tz_cache).await;
        let query = TaskQuery {
            status: optional_string(&args, "status")?.map(|s| normalize_status(&s)),
            from: normalize_datetime(optional_string(&args, "from")?, &tz, "from")?,
            until: normalize_datetime(optional_string(&args, "until")?, &tz, "until")?,
            no_overdue: optional_bool(&args, "no_overdue")?,
            habit_id: habit.as_ref().map(|habit| habit.habit.id.clone()),
            ical_uid: None,
            q: optional_string(&args, "q")?,
            limit: optional_i64(&args, "limit")?,
        };

        let default_query = TaskQuery::default();
        let c1 = self.client.clone();
        let c2 = self.client.clone();
        let c3 = self.client.clone();
        let (tasks, all_tasks, habits) = tokio::try_join!(
            async { c1.list_tasks(&query).await },
            async { c2.list_tasks(&default_query).await },
            async { c3.list_habits().await },
        )
        .map_err(client_error)?;

        let ctx = TaskContext::new(&all_tasks, &habits);
        let content = tasks
            .iter()
            .map(|task| task_json(task, &ctx, Some(&tz)))
            .collect::<Vec<_>>();
        Ok(ToolOutput {
            content: serde_json::to_string(&content).unwrap(),
            ..Default::default()
        })
    }
}

struct GetTask {
    client: Client,
    tz_cache: TimeZoneCache,
}

#[async_trait]
impl Tool for GetTask {
    fn name(&self) -> &'static str {
        "get_task"
    }
    fn description(&self) -> &'static str {
        "Get one or more tasks by display reference. task_ref may be a single #id or h<habit>#<task>, or an array of such references. Returns an object with `tasks` (requested), `dependencies` (all transitive dependencies), and `missing_dependencies` (dependency IDs not found in the task list)."
    }
    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "task_ref": {
                    "anyOf": [
                        {"type": "string", "description": "#42 or h1#5"},
                        {"type": "array", "items": {"type": "string"}, "description": "Array of task references such as [\"#1\", \"h1#5\"]"}
                    ]
                },
            },
            "required": ["task_ref"],
            "additionalProperties": false,
        })
    }
    async fn call(&self, args: Value) -> Result<ToolOutput, ToolError> {
        let args = object(args)?;
        let task_refs = refs_from_args(&args, "task_ref")?;

        let tz = server_timezone(&self.tz_cache).await;
        let default_query = TaskQuery::default();
        let c1 = self.client.clone();
        let c2 = self.client.clone();
        let c3 = self.client.clone();

        let fetch_tasks = try_join_all(task_refs.iter().cloned().map(|r| {
            let client = c1.clone();
            async move { client.get_task(&r).await }
        }));

        let (tasks, all_tasks, habits) = tokio::try_join!(
            fetch_tasks,
            async { c2.list_tasks(&default_query).await },
            async { c3.list_habits().await },
        )
        .map_err(client_error)?;

        let ctx = TaskContext::new(&all_tasks, &habits);
        let (deps, missing) = transitive_dependencies(&tasks, &all_tasks);

        let tasks_json: Vec<Value> = tasks
            .iter()
            .map(|task| task_json(task, &ctx, Some(&tz)))
            .collect();
        let deps_json: Vec<Value> = deps
            .into_iter()
            .map(|task| task_json(task, &ctx, Some(&tz)))
            .collect();

        let result = json!({
            "tasks": tasks_json,
            "dependencies": deps_json,
            "missing_dependencies": missing,
        });
        Ok(ToolOutput {
            content: serde_json::to_string(&result).unwrap(),
            ..Default::default()
        })
    }
}

struct ListHabits {
    client: Client,
}

#[async_trait]
impl Tool for ListHabits {
    fn name(&self) -> &'static str {
        "list_habits"
    }
    fn description(&self) -> &'static str {
        "List all habits."
    }
    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {},
            "additionalProperties": false,
        })
    }
    async fn call(&self, args: Value) -> Result<ToolOutput, ToolError> {
        let _ = object(args)?;
        let habits = self.client.list_habits().await.map_err(client_error)?;
        let content = habits.iter().map(habit_summary_json).collect::<Vec<_>>();
        Ok(ToolOutput {
            content: serde_json::to_string(&content).unwrap(),
            ..Default::default()
        })
    }
}

struct GetHabit {
    client: Client,
}

#[async_trait]
impl Tool for GetHabit {
    fn name(&self) -> &'static str {
        "get_habit"
    }
    fn description(&self) -> &'static str {
        "Get one or more habits by display reference. habit_ref may be a single h<id> or an array of such references. Returns an object with `habits`."
    }
    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "habit_ref": {
                    "anyOf": [
                        {"type": "string", "description": "Habit reference such as h1"},
                        {"type": "array", "items": {"type": "string"}, "description": "Array of habit references such as [\"h1\", \"h2\"]"}
                    ]
                },
            },
            "required": ["habit_ref"],
            "additionalProperties": false,
        })
    }
    async fn call(&self, args: Value) -> Result<ToolOutput, ToolError> {
        let args = object(args)?;
        let habit_refs = refs_from_args(&args, "habit_ref")?;

        let habits = try_join_all(habit_refs.iter().cloned().map(|r| {
            let client = self.client.clone();
            async move { client.get_habit(&r).await }
        }))
        .await
        .map_err(client_error)?;

        let content = habits.iter().map(habit_json).collect::<Vec<_>>();
        Ok(ToolOutput {
            content: serde_json::to_string(&json!({"habits": content})).unwrap(),
            ..Default::default()
        })
    }
}

struct GetSchedule {
    client: Client,
    tz_cache: TimeZoneCache,
}

#[async_trait]
impl Tool for GetSchedule {
    fn name(&self) -> &'static str {
        "get_schedule"
    }
    fn description(&self) -> &'static str {
        "Get the current generated schedule with absolute timestamps. Optionally filter by a date range using from/to (e.g. 2026-07-20, 7d, today, now). Includes overdue tasks by default."
    }
    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "from": {
                    "type": "string",
                    "description": "Start of the range; omitted means unbounded. Accepts absolute date (YYYY-MM-DD), relative days (e.g. '7d' for 7 days from now), 'today', or 'now'."
                },
                "to": {
                    "type": "string",
                    "description": "End of the range; omitted means unbounded. Accepts absolute date (YYYY-MM-DD), relative days (e.g. '7d' for 7 days from now), 'today', or 'now'."
                },
                "no_overdue": {
                    "type": "boolean",
                    "description": "If true, omit the overdue tasks section from the response."
                }
            },
            "additionalProperties": false,
        })
    }
    async fn call(&self, args: Value) -> Result<ToolOutput, ToolError> {
        let args = object(args)?;
        let no_overdue = optional_bool(&args, "no_overdue")?.unwrap_or(false);

        let tz = server_timezone(&self.tz_cache).await;
        let from = optional_string(&args, "from")?;
        let to = optional_string(&args, "to")?;
        let from_ts = from
            .map(|s| parse_date_expression(&s, &tz, false))
            .transpose()
            .map_err(|e| {
                ToolError::InvalidArgs(InvalidArgsError::new("from", format!("invalid: {e}")))
            })?;
        let to_ts = to
            .map(|s| parse_date_expression(&s, &tz, true))
            .transpose()
            .map_err(|e| {
                ToolError::InvalidArgs(InvalidArgsError::new("to", format!("invalid: {e}")))
            })?;

        if let (Some(from), Some(to)) = (from_ts, to_ts)
            && from > to
        {
            return Err(ToolError::InvalidArgs(InvalidArgsError::new(
                "from",
                "must not be later than to",
            )));
        }

        let default_query = TaskQuery::default();
        let c1 = self.client.clone();
        let c2 = self.client.clone();
        let c3 = self.client.clone();
        let (schedule, tasks, habits) = tokio::try_join!(
            async { c1.get_schedule().await },
            async { c2.list_tasks(&default_query).await },
            async { c3.list_habits().await },
        )
        .map_err(client_error)?;

        let ctx = TaskContext::new(&tasks, &habits);
        let entries: Value = serde_json::from_str(&schedule.schedule)
            .map_err(|error| ToolError::Other(Box::new(error)))?;
        let entries = match entries {
            Value::Array(entries) => entries,
            _ => Vec::new(),
        };
        let entries = entries
            .iter()
            .filter(|entry| entry_in_range(entry, from_ts, to_ts, &tz))
            .map(|entry| schedule_entry_value(entry, &ctx, Some(&tz)))
            .collect::<Vec<_>>();

        let mut content = json!({
            "id": schedule.id,
            "created_at": format_datetime_for_display(&schedule.created_at, &tz),
            "updated_at": format_datetime_for_display(&schedule.updated_at, &tz),
            "entries": entries,
        });

        if !no_overdue {
            let overdue: Vec<Value> = tasks
                .iter()
                .filter(|task| is_overdue(task, &tz))
                .filter(|task| overdue_in_range(task, from_ts, to_ts, &tz))
                .map(|task| {
                    let (reference, display_id, title) = match ctx.ref_by_id(&task.id) {
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
                    json!({
                        "reference": reference,
                        "display_id": display_id,
                        "title": title,
                        "end_at": format_datetime_for_display(&task.end_at, &tz),
                    })
                })
                .collect();
            if !overdue.is_empty() {
                content
                    .as_object_mut()
                    .unwrap()
                    .insert("overdue".into(), Value::Array(overdue));
            }
        }

        Ok(ToolOutput {
            content: serde_json::to_string(&content).unwrap(),
            ..Default::default()
        })
    }
}

struct HabitScheduledSpans {
    client: Client,
    tz_cache: TimeZoneCache,
}

#[async_trait]
impl Tool for HabitScheduledSpans {
    fn name(&self) -> &'static str {
        "habit_scheduled_spans"
    }

    fn description(&self) -> &'static str {
        "List, create, or delete scheduled spans for a habit. The effect of a span depends on the habit's active flag: for active habits it is a pause period, for disabled habits it is an activation window. action=list returns existing spans; action=create and action=delete generate approval proposals."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "habit_ref": {"type": "string", "description": "Habit reference such as h1."},
                "action": {"type": "string", "enum": ["list", "create", "delete"], "description": "Operation to perform."},
                "start_date": {"type": "string", "description": "Start date (YYYY-MM-DD) for action=create."},
                "end_date": {"type": "string", "description": "End date (YYYY-MM-DD) for action=create."},
                "reason": {"type": "string", "description": "Optional reason for action=create."},
                "span_id": {"type": "string", "description": "Span id to delete for action=delete. Use the id returned by action=list."},
                "why": {"type": "string", "description": "Short user-facing reason for the proposed change."},
                "warnings": {"type": "array", "items": {"type": "string"}},
                "inferred_fields": crate::inferred_fields_schema("List of fields that were inferred from ambiguous user input and should be highlighted. Do not include obvious conversions (e.g. '1 hour' -> 60 minutes) or values filled from the current date/time."),
            },
            "required": ["habit_ref", "action"],
            "additionalProperties": false,
        })
    }

    async fn call(&self, args: Value) -> Result<ToolOutput, ToolError> {
        let mut args = object(args)?;
        let habit_ref = required_string(&args, "habit_ref")?;
        let habit_ref = strip_leading_hash(&habit_ref).to_string();
        args.insert("habit_ref".to_string(), Value::String(habit_ref.clone()));
        let action = required_string(&args, "action")?.to_lowercase();
        args.remove("action");

        let habit = self
            .client
            .get_habit(&habit_ref)
            .await
            .map_err(client_error)?;

        let tz = server_timezone(&self.tz_cache).await;

        match action.as_str() {
            "list" => self.list(&habit, &tz).await,
            "create" => self.propose_create(&mut args, &habit).await,
            "delete" => self.propose_delete(&mut args, &habit, &tz).await,
            _ => Err(ToolError::InvalidArgs(InvalidArgsError::new(
                "action",
                format!("must be 'list', 'create', or 'delete', got {action}"),
            ))),
        }
    }
}

impl HabitScheduledSpans {
    async fn list(
        &self,
        habit: &HabitDetail,
        tz: &jiff::tz::TimeZone,
    ) -> Result<ToolOutput, ToolError> {
        let spans = self
            .client
            .list_habit_scheduled_spans(&habit.habit.id)
            .await
            .map_err(client_error)?;
        let content = json!({
            "habit_ref": format!("h{}", habit.habit.display_id),
            "active": habit.habit.active,
            "kind": if habit.habit.active { "pause" } else { "active" },
            "spans": spans.iter().map(|span| span_json(span, tz)).collect::<Vec<_>>(),
        });
        Ok(ToolOutput {
            content: serde_json::to_string(&content).unwrap(),
            ..Default::default()
        })
    }

    async fn propose_create(
        &self,
        args: &mut serde_json::Map<String, Value>,
        habit: &HabitDetail,
    ) -> Result<ToolOutput, ToolError> {
        let start_date = required_string(args, "start_date")?;
        args.insert("start_date".to_string(), Value::String(start_date.clone()));
        let end_date = required_string(args, "end_date")?;
        args.insert("end_date".to_string(), Value::String(end_date.clone()));

        if let Some(reason) = optional_string(args, "reason")? {
            args.insert("reason".to_string(), Value::String(reason));
        } else {
            args.remove("reason");
        }

        validate_scheduled_span_dates(&start_date, &end_date)?;

        self.proposal_output(args, habit, "create_scheduled_span", None)
            .await
    }

    async fn propose_delete(
        &self,
        args: &mut serde_json::Map<String, Value>,
        habit: &HabitDetail,
        tz: &jiff::tz::TimeZone,
    ) -> Result<ToolOutput, ToolError> {
        let span_id = required_string(args, "span_id")?;
        args.insert("span_id".to_string(), Value::String(span_id.clone()));
        let spans = self
            .client
            .list_habit_scheduled_spans(&habit.habit.id)
            .await
            .map_err(client_error)?;
        let span = spans
            .into_iter()
            .find(|span| span.id == span_id)
            .ok_or_else(|| {
                ToolError::NotFound(format!(
                    "scheduled span {span_id} not found for habit h{}",
                    habit.habit.display_id
                ))
            })?;
        let before = span_json(&span, tz);
        self.proposal_output(args, habit, "delete_scheduled_span", Some(before))
            .await
    }

    async fn proposal_output(
        &self,
        args: &mut serde_json::Map<String, Value>,
        habit: &HabitDetail,
        operation: &str,
        before: Option<Value>,
    ) -> Result<ToolOutput, ToolError> {
        let mut start_date = optional_string(args, "start_date")?.unwrap_or_default();
        let mut end_date = optional_string(args, "end_date")?.unwrap_or_default();

        // For delete, prefer the actual span dates from `before`.
        if let Some(before_obj) = before.as_ref().and_then(Value::as_object) {
            if let Some(s) = before_obj.get("start_date").and_then(Value::as_str) {
                start_date = s.to_owned();
            }
            if let Some(s) = before_obj.get("end_date").and_then(Value::as_str) {
                end_date = s.to_owned();
            }
        }

        let description = match operation {
            "create_scheduled_span" => {
                format!(
                    "h{}にscheduled span {start_date}〜{end_date}を追加",
                    habit.habit.display_id
                )
            }
            "delete_scheduled_span" => {
                format!(
                    "h{}のscheduled span {start_date}〜{end_date}を削除",
                    habit.habit.display_id
                )
            }
            _ => format!("h{}のscheduled spanを{operation}", habit.habit.display_id),
        };

        let why = optional_string(args, "why")?.unwrap_or_default();
        if why.is_empty() {
            args.remove("why");
        } else {
            args.insert("why".to_string(), Value::String(why.clone()));
        }
        let warnings = args
            .get("warnings")
            .and_then(Value::as_array)
            .map(|values| {
                values
                    .iter()
                    .filter_map(Value::as_str)
                    .map(ToOwned::to_owned)
                    .collect()
            })
            .unwrap_or_default();
        let inferred_fields = parse_inferred_fields(args)?;

        args.remove("action");
        let mut display_args = args.clone();
        format_display_datetime_args(&mut display_args, &server_timezone(&self.tz_cache).await);
        let execution_args = args.clone();

        let proposal = crate::ProposedChange {
            operation: operation.to_owned(),
            target_label: format!("habit h{}", habit.habit.display_id),
            description,
            before,
            after: Some(Value::Object(display_args)),
            arguments: Some(Value::Object(execution_args)),
            observed_updated_at: None,
        };

        let content = json!({
            "approval_required": true,
            "operation": proposal.operation,
            "target": proposal.target_label,
            "inferred_fields": inferred_fields,
            "why": why,
            "warnings": warnings,
        });

        Ok(ToolOutput {
            content: serde_json::to_string(&content).unwrap(),
            why: Some(why),
            warnings,
            proposed_changes: vec![proposal],
            inferred_fields,
            schedule_dirty: false,
            ..Default::default()
        })
    }
}

fn span_json(span: &HabitScheduledSpanRow, tz: &jiff::tz::TimeZone) -> Value {
    json!({
        "id": span.id,
        "start_date": span.start_date,
        "end_date": span.end_date,
        "reason": span.reason,
        "created_at": format_datetime_for_display(&span.created_at, tz),
    })
}

fn parse_inferred_fields(
    args: &serde_json::Map<String, Value>,
) -> Result<Vec<crate::InferredField>, ToolError> {
    let inferred_fields = args
        .get("inferred_fields")
        .cloned()
        .unwrap_or_else(|| json!([]));
    serde_json::from_value::<Vec<crate::InferredField>>(inferred_fields).map_err(|error| {
        ToolError::InvalidArgs(InvalidArgsError::new(
            "inferred_fields",
            format!("invalid: {error}"),
        ))
    })
}

fn validate_scheduled_span_dates(start: &str, end: &str) -> Result<(), ToolError> {
    let start = Date::strptime("%Y-%m-%d", start).map_err(|error| {
        ToolError::InvalidArgs(InvalidArgsError::new(
            "start_date",
            format!("invalid date: {error}"),
        ))
    })?;
    let end = Date::strptime("%Y-%m-%d", end).map_err(|error| {
        ToolError::InvalidArgs(InvalidArgsError::new(
            "end_date",
            format!("invalid date: {error}"),
        ))
    })?;
    if start > end {
        return Err(ToolError::InvalidArgs(InvalidArgsError::new(
            "end_date",
            "must be on or after start_date",
        )));
    }
    Ok(())
}

struct GetSettings {
    client: Client,
}

#[async_trait]
impl Tool for GetSettings {
    fn name(&self) -> &'static str {
        "get_settings"
    }
    fn description(&self) -> &'static str {
        "Get server timezone and sleep/work settings."
    }
    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {},
            "additionalProperties": false,
        })
    }
    async fn call(&self, args: Value) -> Result<ToolOutput, ToolError> {
        let _ = object(args)?;
        let settings = self.client.get_settings().await.map_err(client_error)?;
        Ok(ToolOutput {
            content: serde_json::to_string(&settings).unwrap(),
            ..Default::default()
        })
    }
}

struct PreviewScheduleTool {
    client: Client,
    tz_cache: TimeZoneCache,
}

#[async_trait]
impl Tool for PreviewScheduleTool {
    fn name(&self) -> &'static str {
        "preview_schedule"
    }
    fn description(&self) -> &'static str {
        "Preview a schedule without replacing the active schedule; reports moved, unscheduled, and sleep impact."
    }
    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "mode": {"type": "string"},
                "from": {"type": "string", "description": "Start of range; interpreted in server timezone."},
                "until": {"type": "string", "description": "End of range; interpreted in server timezone."},
                "task_ids": {"type": "array", "items": {"type": "string"}},
                "pinned": {"type": "array", "items": {"type": "string"}},
                "sleep": {"type": "string"},
            },
            "additionalProperties": false,
        })
    }
    async fn call(&self, args: Value) -> Result<ToolOutput, ToolError> {
        let mut args = object(args)?;
        args.entry("mode")
            .or_insert_with(|| Value::String("full".into()));

        let tz = server_timezone(&self.tz_cache).await;
        normalize_mutation_field(&mut args, "from", &tz)?;
        normalize_mutation_field(&mut args, "until", &tz)?;
        normalize_reference_array(&mut args, "task_ids")?;
        normalize_reference_array(&mut args, "pinned")?;

        let request: SchedulePreviewRequest =
            serde_json::from_value(Value::Object(args)).map_err(|error| {
                ToolError::InvalidArgs(InvalidArgsError::no_field(error.to_string()))
            })?;

        let default_query = TaskQuery::default();
        let c1 = self.client.clone();
        let c2 = self.client.clone();
        let c3 = self.client.clone();
        let req = request;
        let (preview, tasks, habits) = tokio::try_join!(
            async { c1.preview_schedule(&req).await },
            async { c2.list_tasks(&default_query).await },
            async { c3.list_habits().await },
        )
        .map_err(client_error)?;

        let ctx = TaskContext::new(&tasks, &habits);
        Ok(ToolOutput {
            content: serde_json::to_string(&transform_preview(preview, &ctx, Some(&tz))).unwrap(),
            ..Default::default()
        })
    }
}

/// Registers planner mutation tools and the hybrid habit_scheduled_spans tool.
/// Calls produce approval proposals or read data; they never write directly.
pub fn register_mutation_tools(
    registry: &mut ToolRegistry,
    client: Client,
    tz_cache: TimeZoneCache,
) {
    for kind in [
        MutationKind::CreateTask,
        MutationKind::UpdateTask,
        MutationKind::DeleteTask,
        MutationKind::CreateHabit,
        MutationKind::UpdateHabit,
        MutationKind::DeleteHabit,
        MutationKind::GenerateSchedule,
        MutationKind::Reschedule,
    ] {
        registry.register(Box::new(MutationTool {
            client: client.clone(),
            tz_cache: tz_cache.clone(),
            kind,
        }));
    }
    registry.register(Box::new(HabitScheduledSpans { client, tz_cache }));
}

/// JSON schema for a single habit step input used in create/update habit.
fn habit_step_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "position": {"type": "integer", "description": "1-indexed display position of the step within the habit. On update, matching existing positions update existing steps; new positions create new steps."},
            "title": {"type": "string"},
            "description": {"type": "string"},
            "start_time": {"type": "string", "description": "Time of day (HH:MM)."},
            "end_time": {"type": "string", "description": "Time of day (HH:MM)."},
            "avg_minutes": {"type": "integer"},
            "sigma_minutes": {"type": "integer"},
            "parallelizable": {"type": "boolean"},
            "allows_parallel": {"type": "boolean"},
            "abandonability": {"type": "number"},
            "fixed": {"type": "boolean"},
            "depends_on": {"type": "array", "items": {"type": "integer"}, "description": "Display positions (1-indexed) of steps this step depends on."}
        },
        "required": ["position", "title", "start_time", "end_time", "avg_minutes"],
        "additionalProperties": false
    })
}

#[derive(Clone, Copy)]
enum MutationKind {
    CreateTask,
    UpdateTask,
    DeleteTask,
    CreateHabit,
    UpdateHabit,
    DeleteHabit,
    GenerateSchedule,
    Reschedule,
}

impl MutationKind {
    fn name(self) -> &'static str {
        match self {
            Self::CreateTask => "create_task",
            Self::UpdateTask => "update_task",
            Self::DeleteTask => "delete_task",
            Self::CreateHabit => "create_habit",
            Self::UpdateHabit => "update_habit",
            Self::DeleteHabit => "delete_habit",
            Self::GenerateSchedule => "generate_schedule",
            Self::Reschedule => "reschedule",
        }
    }

    fn description(self) -> &'static str {
        match self {
            Self::CreateTask => {
                "Create a task proposal. Calling this tool generates a pending approval request; it does not write immediately. For example, \"演習30題追加\"."
            }
            Self::UpdateTask => {
                "Create a task update proposal. Calling this tool generates a pending approval request; it does not write immediately."
            }
            Self::DeleteTask => {
                "Create a task deletion proposal. Calling this tool generates a pending approval request; it does not write immediately."
            }
            Self::CreateHabit => {
                "Create a recurring habit proposal. Calling this tool generates a pending approval request; it does not write immediately."
            }
            Self::UpdateHabit => {
                "Create a recurring habit update proposal. Calling this tool generates a pending approval request; it does not write immediately."
            }
            Self::DeleteHabit => {
                "Create a recurring habit deletion proposal. Calling this tool generates a pending approval request; it does not write immediately."
            }
            Self::GenerateSchedule => {
                "Create a schedule generation proposal. Calling this tool generates a pending approval request; it does not write immediately."
            }
            Self::Reschedule => {
                "Create a partial reschedule proposal. Calling this tool generates a pending approval request; it does not write immediately."
            }
        }
    }

    fn target_type(self) -> &'static str {
        match self {
            Self::CreateTask | Self::UpdateTask | Self::DeleteTask => "task",
            Self::CreateHabit | Self::UpdateHabit | Self::DeleteHabit => "habit",
            Self::GenerateSchedule | Self::Reschedule => "schedule",
        }
    }

    fn operation(self) -> &'static str {
        match self {
            Self::CreateTask | Self::CreateHabit => "create",
            Self::UpdateTask | Self::UpdateHabit => "update",
            Self::DeleteTask | Self::DeleteHabit => "delete",
            Self::GenerateSchedule => "generate",
            Self::Reschedule => "reschedule",
        }
    }

    fn change_summary(self, args: &serde_json::Map<String, Value>) -> (String, String) {
        let title = summary_string(args, "title");
        let task_ref = summary_string(args, "task_ref");
        let habit_ref = summary_string(args, "habit_ref");
        match self {
            Self::CreateTask => {
                let t = title.unwrap_or_else(|| "(名称未設定)".to_owned());
                (t.clone(), format!("「{t}」を作成"))
            }
            Self::UpdateTask => {
                let r = task_ref.unwrap_or_else(|| "(参照不明)".to_owned());
                let description =
                    title.map_or_else(|| format!("{r}を更新"), |t| format!("「{t}」を更新"));
                (r, description)
            }
            Self::DeleteTask => {
                let r = task_ref.unwrap_or_else(|| "(参照不明)".to_owned());
                (r.clone(), format!("{r}を削除"))
            }
            Self::CreateHabit => {
                let t = title.unwrap_or_else(|| "(名称未設定)".to_owned());
                (t.clone(), format!("「{t}」を作成"))
            }
            Self::UpdateHabit => {
                let r = habit_ref.unwrap_or_else(|| "(参照不明)".to_owned());
                let description =
                    title.map_or_else(|| format!("{r}を更新"), |t| format!("「{t}」を更新"));
                (r, description)
            }
            Self::DeleteHabit => {
                let r = habit_ref.unwrap_or_else(|| "(参照不明)".to_owned());
                (r.clone(), format!("{r}を削除"))
            }
            Self::GenerateSchedule => (String::new(), "スケジュールを生成".to_owned()),
            Self::Reschedule => (String::new(), "スケジュールを再調整".to_owned()),
        }
    }

    fn schema(self) -> Value {
        let (required, properties) = match self {
            Self::CreateTask => (
                json!(["title", "end_at", "avg_minutes"]),
                json!({
                    "title": {"type": "string"},
                    "description": {"type": "string"},
                    "start_at": {"type": "string", "description": "Start time; interpreted in server timezone if no offset is given."},
                    "end_at": {"type": "string", "description": "Deadline; interpreted in server timezone if no offset is given."},
                    "avg_minutes": {"type": "integer"},
                    "sigma_minutes": {"type": "integer"},
                    "depends": {"type": "array", "items": {"type": "string"}},
                    "parallelizable": {"type": "boolean"},
                    "allows_parallel": {"type": "boolean"},
                    "abandonability": {"type": "number"},
                    "fixed": {"type": "boolean", "description": "If true, the start time is fixed and the scheduler will not move the task."},
                    "quantity_total": {"type": "integer", "description": "Total quantity for a quantitative task (e.g. 30)."},
                    "quantity_done": {"type": "integer", "description": "Quantity already completed; defaults to 0."},
                    "quantity_unit": {"type": "string", "description": "Unit for the quantity (e.g. 'pages', 'questions')."},
                    "inferred_fields": crate::inferred_fields_schema("List of fields that were inferred from ambiguous user input and should be highlighted. Do not include obvious conversions (e.g. '1 hour' -> 60 minutes) or values filled from the current date/time."),
                }),
            ),
            Self::UpdateTask => (
                json!(["task_ref"]),
                json!({
                    "task_ref": {"type": "string"},
                    "title": {"type": "string"},
                    "description": {"type": "string"},
                    "start_at": {"type": "string", "description": "Start time; interpreted in server timezone if no offset is given."},
                    "end_at": {"type": "string", "description": "Deadline; interpreted in server timezone if no offset is given."},
                    "avg_minutes": {"type": "integer"},
                    "sigma_minutes": {"type": "integer"},
                    "depends": {"type": "array", "items": {"type": "string"}},
                    "parallelizable": {"type": "boolean"},
                    "allows_parallel": {"type": "boolean"},
                    "abandonability": {"type": "number"},
                    "status": {
                        "type": "string",
                        "enum": ["pending", "scheduled", "in_progress", "completed", "skipped"],
                        "description": "New task status. 'completed' means done."
                    },
                    "fixed": {"type": "boolean", "description": "If true, the start time is fixed and the scheduler will not move the task."},
                    "quantity_total": {"type": "integer", "description": "Total quantity for a quantitative task (e.g. 30)."},
                    "quantity_done": {"type": "integer", "description": "Quantity already completed."},
                    "quantity_unit": {"type": "string", "description": "Unit for the quantity (e.g. 'pages', 'questions')."},
                    "inferred_fields": crate::inferred_fields_schema("List of fields that were inferred from ambiguous user input and should be highlighted. Do not include obvious conversions (e.g. '1 hour' -> 60 minutes) or values filled from the current date/time."),
                }),
            ),
            Self::DeleteTask => (
                json!(["task_ref"]),
                json!({
                    "task_ref": {"type": "string"},
                }),
            ),
            Self::CreateHabit => (
                json!([
                    "title",
                    "recurrence",
                    "start_time",
                    "end_time",
                    "avg_minutes"
                ]),
                json!({
                    "title": {"type": "string"},
                    "description": {"type": "string"},
                    "recurrence": {"type": "string"},
                    "start_time": {"type": "string", "description": "Time of day (HH:MM)."},
                    "end_time": {"type": "string", "description": "Time of day (HH:MM)."},
                    "avg_minutes": {"type": "integer"},
                    "sigma_minutes": {"type": "integer"},
                    "parallelizable": {"type": "boolean"},
                    "allows_parallel": {"type": "boolean"},
                    "abandonability": {"type": "number"},
                    "fixed": {"type": "boolean", "description": "If true, generated tasks start at a fixed time and the scheduler will not move them."},
                    "window_mode": {"type": "string", "enum": ["day", "period"], "description": "Scheduling window mode for generated tasks."},
                    "steps": {"type": "array", "items": habit_step_schema(), "description": "Ordered steps for a multi-step habit. Existing step ids are omitted; match by position on update."},
                    "inferred_fields": crate::inferred_fields_schema("List of fields that were inferred from ambiguous user input and should be highlighted. Do not include obvious conversions (e.g. '1 hour' -> 60 minutes) or values filled from the current date/time."),
                }),
            ),
            Self::UpdateHabit => (
                json!(["habit_ref"]),
                json!({
                    "habit_ref": {"type": "string"},
                    "title": {"type": "string"},
                    "description": {"type": "string"},
                    "recurrence": {"type": "string"},
                    "start_time": {"type": "string", "description": "Time of day (HH:MM)."},
                    "end_time": {"type": "string", "description": "Time of day (HH:MM)."},
                    "avg_minutes": {"type": "integer"},
                    "sigma_minutes": {"type": "integer"},
                    "parallelizable": {"type": "boolean"},
                    "allows_parallel": {"type": "boolean"},
                    "abandonability": {"type": "number"},
                    "active": {"type": "boolean"},
                    "fixed": {"type": "boolean", "description": "If true, generated tasks start at a fixed time and the scheduler will not move them."},
                    "window_mode": {"type": "string", "enum": ["day", "period"], "description": "Scheduling window mode for generated tasks."},
                    "steps": {"type": "array", "items": habit_step_schema(), "description": "Complete ordered steps to replace existing ones. Match existing steps by position."},
                    "inferred_fields": crate::inferred_fields_schema("List of fields that were inferred from ambiguous user input and should be highlighted. Do not include obvious conversions (e.g. '1 hour' -> 60 minutes) or values filled from the current date/time."),
                }),
            ),
            Self::DeleteHabit => (
                json!(["habit_ref"]),
                json!({
                    "habit_ref": {"type": "string"},
                }),
            ),
            Self::GenerateSchedule => (
                json!([]),
                json!({
                    "task_ids": {"type": "array", "items": {"type": "string"}},
                    "sleep": {"type": "string"},
                }),
            ),
            Self::Reschedule => (
                json!(["mode"]),
                json!({
                    "mode": {"type": "string"},
                    "from": {"type": "string", "description": "Start of range; interpreted in server timezone if no offset is given."},
                    "until": {"type": "string", "description": "End of range; interpreted in server timezone if no offset is given."},
                    "task_ids": {"type": "array", "items": {"type": "string"}},
                    "pinned": {"type": "array", "items": {"type": "string"}},
                    "sleep": {"type": "string"},
                }),
            ),
        };
        let properties = properties.as_object().cloned().unwrap_or_default();
        let mut properties = serde_json::Map::from_iter(properties);
        properties.insert(
            "why".into(),
            json!({"type": "string", "description": "Short user-facing reason for the proposed change."}),
        );
        properties.insert(
            "warnings".into(),
            json!({"type": "array", "items": {"type": "string"}}),
        );
        json!({
            "type": "object",
            "properties": properties,
            "required": required,
            "additionalProperties": false,
        })
    }
}

struct MutationTool {
    client: Client,
    tz_cache: TimeZoneCache,
    kind: MutationKind,
}

#[async_trait]
impl Tool for MutationTool {
    fn name(&self) -> &'static str {
        self.kind.name()
    }
    fn description(&self) -> &'static str {
        self.kind.description()
    }
    fn parameters_schema(&self) -> Value {
        self.kind.schema()
    }

    async fn call(&self, args: Value) -> Result<ToolOutput, ToolError> {
        let mut args = object(args)?;
        validate_mutation(self.kind, &args)?;

        let tz = server_timezone(&self.tz_cache).await;
        normalize_mutation_args(self.kind, &mut args, &tz)?;

        let mut execution_args = args.clone();
        normalize_execution_references(self.kind, &mut execution_args)?;
        // Convert absolute datetimes back to the configured timezone for the
        // approval UI; execution_args retains the canonical UTC values.
        format_display_datetime_args(&mut args, &tz);

        let (before, observed_updated_at) = match self.kind {
            MutationKind::UpdateTask | MutationKind::DeleteTask => {
                let lookup = required_string(&execution_args, "task_ref")?;

                let default_query = TaskQuery::default();
                let c1 = self.client.clone();
                let c2 = self.client.clone();
                let c3 = self.client.clone();
                let (task, all_tasks, habits) = tokio::try_join!(
                    async { c1.get_task(&lookup).await },
                    async { c2.list_tasks(&default_query).await },
                    async { c3.list_habits().await },
                )
                .map_err(client_error)?;

                let ctx = TaskContext::new(&all_tasks, &habits);
                (
                    Some(task_json(&task, &ctx, Some(&tz))),
                    Some(task.updated_at),
                )
            }
            MutationKind::UpdateHabit | MutationKind::DeleteHabit => {
                let reference = required_string(&args, "habit_ref")?;
                let habit = self
                    .client
                    .get_habit(&reference)
                    .await
                    .map_err(client_error)?;
                (Some(habit_json(&habit)), Some(habit.habit.updated_at))
            }
            _ => (None, None),
        };

        if matches!(
            self.kind,
            MutationKind::GenerateSchedule | MutationKind::Reschedule
        ) {
            let mut preview_args = execution_args.clone();
            if matches!(self.kind, MutationKind::GenerateSchedule) {
                preview_args.insert("mode".into(), Value::String("full".into()));
            }
            let request: SchedulePreviewRequest =
                serde_json::from_value(Value::Object(preview_args)).map_err(|error| {
                    ToolError::InvalidArgs(InvalidArgsError::no_field(error.to_string()))
                })?;

            let default_query = TaskQuery::default();
            let c1 = self.client.clone();
            let c2 = self.client.clone();
            let c3 = self.client.clone();
            let req = request;
            let (preview, all_tasks, habits) = tokio::try_join!(
                async { c1.preview_schedule(&req).await },
                async { c2.list_tasks(&default_query).await },
                async { c3.list_habits().await },
            )
            .map_err(client_error)?;

            let ctx = TaskContext::new(&all_tasks, &habits);
            let entries = preview.get("entries").cloned().ok_or_else(|| {
                ToolError::InvalidArgs(InvalidArgsError::no_field(
                    "schedule preview did not return entries",
                ))
            })?;
            execution_args.insert("_preview_entries".into(), entries);
            args.insert(
                "_preview".into(),
                transform_preview(preview, &ctx, Some(&tz)),
            );
        }

        let (target, description) = self.kind.change_summary(&args);
        let inferred_fields = args
            .get("inferred_fields")
            .cloned()
            .unwrap_or_else(|| json!([]));
        let inferred_fields = serde_json::from_value::<Vec<crate::InferredField>>(inferred_fields)
            .map_err(|error| {
                ToolError::InvalidArgs(InvalidArgsError::new(
                    "inferred_fields",
                    format!("invalid: {error}"),
                ))
            })?;
        let why = optional_string(&args, "why")?;
        let warnings = args
            .get("warnings")
            .and_then(Value::as_array)
            .map(|values| {
                values
                    .iter()
                    .filter_map(Value::as_str)
                    .map(ToOwned::to_owned)
                    .collect()
            })
            .unwrap_or_default();
        let target_type = self.kind.target_type();
        let target_label = if target.is_empty() {
            target_type.to_owned()
        } else {
            format!("{target_type} {target}")
        };
        let proposal = crate::ProposedChange {
            operation: self.kind.operation().to_owned(),
            target_label,
            description,
            before,
            after: Some(Value::Object(args)),
            arguments: Some(Value::Object(execution_args)),
            observed_updated_at,
        };
        Ok(ToolOutput {
            content: serde_json::to_string(&json!({"approval_required":true,"operation":proposal.operation,"target":proposal.target_label,"inferred_fields":inferred_fields,"why":why,"warnings":warnings})).unwrap(),
            why,
            warnings,
            proposed_changes: vec![proposal],
            inferred_fields,
            schedule_dirty: false,
            ..Default::default()
        })
    }
}

struct MoveTaskTool {
    client: Client,
    tz_cache: TimeZoneCache,
}

#[async_trait]
impl Tool for MoveTaskTool {
    fn name(&self) -> &'static str {
        "move_task"
    }

    fn description(&self) -> &'static str {
        "Propose moving a scheduled task to a new start time. The task can also be marked fixed (default true). Generates a pending approval request; it does not write immediately."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "task_ref": {"type": "string", "description": "Task reference such as #42 or h1#3."},
                "start_at": {"type": "string", "description": "New start time; interpreted in server timezone if no offset is given."},
                "force": {"type": "boolean", "description": "Override deadline violation warnings."},
                "fixed": {"type": "boolean", "description": "Mark the task as fixed after moving; defaults to true."},
                "why": {"type": "string", "description": "Short user-facing reason for the proposed change."},
                "warnings": {"type": "array", "items": {"type": "string"}},
                "inferred_fields": crate::inferred_fields_schema("List of fields that were inferred from ambiguous user input and should be highlighted. Do not include obvious conversions (e.g. '1 hour' -> 60 minutes) or values filled from the current date/time."),
            },
            "required": ["task_ref", "start_at"],
            "additionalProperties": false,
        })
    }

    async fn call(&self, args: Value) -> Result<ToolOutput, ToolError> {
        let mut args = object(args)?;
        let tz = server_timezone(&self.tz_cache).await;
        normalize_mutation_field(&mut args, "start_at", &tz)?;
        normalize_task_ref(&mut args, "task_ref")?;

        if args.get("fixed").is_none() {
            args.insert("fixed".to_string(), Value::Bool(true));
        }

        // Validate optional booleans; defaults are applied above.
        optional_bool(&args, "force")?;
        optional_bool(&args, "fixed")?;

        let task_ref = required_string(&args, "task_ref")?;
        let start_at = required_string(&args, "start_at")?;

        let display_ref = if task_ref.starts_with('h') || task_ref.starts_with('H') {
            task_ref.clone()
        } else {
            format!("#{task_ref}")
        };

        let default_query = TaskQuery::default();
        let c1 = self.client.clone();
        let c2 = self.client.clone();
        let c3 = self.client.clone();
        let (task, all_tasks, habits) = tokio::try_join!(
            async { c1.get_task(&task_ref).await },
            async { c2.list_tasks(&default_query).await },
            async { c3.list_habits().await },
        )
        .map_err(client_error)?;

        let schedule_row = self.client.get_schedule().await.map_err(client_error)?;
        let entries: Vec<Value> = serde_json::from_str(&schedule_row.schedule)
            .map_err(|error| ToolError::Other(Box::new(error)))?;
        let current_entry = entries
            .into_iter()
            .find(|e| e.get("task_id").and_then(Value::as_str) == Some(&task.id));
        let Some(current_entry) = current_entry else {
            return Err(ToolError::NotFound(format!(
                "task {display_ref} is not in the active schedule"
            )));
        };

        let ctx = TaskContext::new(&all_tasks, &habits);
        let mut before = task_json(&task, &ctx, Some(&tz));
        before["schedule_start_at"] = current_entry
            .get("start_at")
            .cloned()
            .unwrap_or(Value::Null);
        before["schedule_end_at"] = current_entry.get("end_at").cloned().unwrap_or(Value::Null);

        let start_ts = jiff::Timestamp::from_str(&start_at).map_err(|error| {
            ToolError::InvalidArgs(InvalidArgsError::new(
                "start_at",
                format!("invalid: {error}"),
            ))
        })?;
        let duration_minutes = if let (Some(old_start_str), Some(old_end_str)) = (
            current_entry.get("start_at").and_then(Value::as_str),
            current_entry.get("end_at").and_then(Value::as_str),
        ) {
            let old_start = jiff::Timestamp::from_str(old_start_str).map_err(|error| {
                ToolError::InvalidArgs(InvalidArgsError::new(
                    "schedule_start_at",
                    format!("invalid: {error}"),
                ))
            })?;
            let old_end = jiff::Timestamp::from_str(old_end_str).map_err(|error| {
                ToolError::InvalidArgs(InvalidArgsError::new(
                    "schedule_end_at",
                    format!("invalid: {error}"),
                ))
            })?;
            let duration = old_end - old_start;
            duration
                .total(jiff::Unit::Minute)
                .map_err(|error| ToolError::Other(Box::new(error)))? as i64
        } else {
            task.avg_minutes
        };
        let end_ts = start_ts
            .checked_add(jiff::Span::new().minutes(duration_minutes))
            .expect("valid end time");
        let end_at = end_ts.to_string();

        let mut display_args = args.clone();
        display_args.insert("task_ref".to_string(), Value::String(display_ref.clone()));
        display_args.insert("end_at".to_string(), Value::String(end_at));
        format_display_datetime_args(&mut display_args, &tz);

        let inferred_fields = args
            .get("inferred_fields")
            .cloned()
            .unwrap_or_else(|| json!([]));
        let inferred_fields = serde_json::from_value::<Vec<crate::InferredField>>(inferred_fields)
            .map_err(|error| {
                ToolError::InvalidArgs(InvalidArgsError::new(
                    "inferred_fields",
                    format!("invalid: {error}"),
                ))
            })?;

        let why = optional_string(&args, "why")?;
        let warnings = args
            .get("warnings")
            .and_then(Value::as_array)
            .map(|values| {
                values
                    .iter()
                    .filter_map(Value::as_str)
                    .map(ToOwned::to_owned)
                    .collect()
            })
            .unwrap_or_default();

        let display_start = display_args
            .get("start_at")
            .and_then(Value::as_str)
            .unwrap_or(&start_at);
        let description = format!("「{}」を {} に移動", task.title, display_start);

        let mut execution_args = args.clone();
        execution_args.remove("why");
        execution_args.remove("warnings");
        execution_args.remove("inferred_fields");

        let proposal = crate::ProposedChange {
            operation: "move".to_string(),
            target_label: format!("task {}", display_ref),
            description,
            before: Some(before),
            after: Some(Value::Object(display_args)),
            arguments: Some(Value::Object(execution_args)),
            observed_updated_at: Some(task.updated_at),
        };
        Ok(ToolOutput {
            content: serde_json::to_string(&json!({"approval_required":true,"operation":proposal.operation,"target":proposal.target_label,"inferred_fields":inferred_fields,"why":why,"warnings":warnings})).unwrap(),
            why,
            warnings,
            proposed_changes: vec![proposal],
            inferred_fields,
            schedule_dirty: false,
            ..Default::default()
        })
    }
}

fn normalize_mutation_field(
    args: &mut serde_json::Map<String, Value>,
    name: &str,
    tz: &jiff::tz::TimeZone,
) -> Result<(), ToolError> {
    if let Some(value) = optional_string(args, name)? {
        let normalized = parse_datetime_tz(&value, tz).map_err(|error| {
            ToolError::InvalidArgs(InvalidArgsError::new(name, format!("invalid: {error}")))
        })?;
        args.insert(name.into(), Value::String(normalized));
    }
    Ok(())
}

fn normalize_mutation_args(
    kind: MutationKind,
    args: &mut serde_json::Map<String, Value>,
    tz: &jiff::tz::TimeZone,
) -> Result<(), ToolError> {
    match kind {
        MutationKind::CreateTask | MutationKind::UpdateTask => {
            normalize_mutation_field(args, "start_at", tz)?;
            normalize_mutation_field(args, "end_at", tz)?;
            if let Some(status) = args.get("status").and_then(Value::as_str) {
                args.insert("status".into(), Value::String(normalize_status(status)));
            }
        }
        MutationKind::Reschedule => {
            normalize_mutation_field(args, "from", tz)?;
            normalize_mutation_field(args, "until", tz)?;
        }
        _ => {}
    }
    Ok(())
}

/// Strip a leading `#` from a single string reference field for backend execution.
fn normalize_task_ref(
    args: &mut serde_json::Map<String, Value>,
    key: &str,
) -> Result<(), ToolError> {
    if let Some(value) = optional_string(args, key)? {
        args.insert(
            key.to_string(),
            Value::String(strip_leading_hash(&value).to_string()),
        );
    }
    Ok(())
}

/// Strip leading `#` characters from reference fields used for backend execution.
/// Display-facing `args` keep the original user input so approval diffs stay clean.
fn normalize_execution_references(
    kind: MutationKind,
    args: &mut serde_json::Map<String, Value>,
) -> Result<(), ToolError> {
    match kind {
        MutationKind::CreateTask => {
            normalize_reference_array(args, "depends")?;
        }
        MutationKind::UpdateTask => {
            normalize_task_ref(args, "task_ref")?;
            normalize_reference_array(args, "depends")?;
        }
        MutationKind::DeleteTask => {
            normalize_task_ref(args, "task_ref")?;
        }
        MutationKind::GenerateSchedule => {
            normalize_reference_array(args, "task_ids")?;
        }
        MutationKind::Reschedule => {
            normalize_reference_array(args, "task_ids")?;
            normalize_reference_array(args, "pinned")?;
        }
        _ => {}
    }
    Ok(())
}

fn validate_mutation(
    kind: MutationKind,
    args: &serde_json::Map<String, Value>,
) -> Result<(), ToolError> {
    match kind {
        MutationKind::CreateTask => {
            required_string(args, "title")?;
            required_string(args, "end_at")?;
            required_i64(args, "avg_minutes")?;
        }
        MutationKind::UpdateTask | MutationKind::DeleteTask => {
            required_string(args, "task_ref")?;
        }
        MutationKind::CreateHabit => {
            required_string(args, "title")?;
            required_string(args, "recurrence")?;
            required_string(args, "start_time")?;
            required_string(args, "end_time")?;
            required_i64(args, "avg_minutes")?;
        }
        MutationKind::UpdateHabit | MutationKind::DeleteHabit => {
            required_string(args, "habit_ref")?;
        }
        MutationKind::GenerateSchedule => {}
        MutationKind::Reschedule => {
            required_string(args, "mode")?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{Json, Router, routing::get};
    use takusu_client::{HabitDetail, HabitScheduledSpanRow, ScheduleRow, SettingsResponse};

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
            end_at: "2025-06-05T10:00:00Z".to_string(),
            avg_minutes: 30,
            sigma_minutes: 5,
            depends: serde_json::to_string(
                &depends.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
            )
            .unwrap(),
            parallelizable: false,
            allows_parallel: false,
            abandonability: 0.5,
            status: "pending".to_string(),
            habit_id: habit_id.map(|s| s.to_string()),
            ical_uid: None,
            user_edited: false,
            fixed: false,
            habit_step_id: None,
            quantity_total: None,
            quantity_done: 0,
            quantity_unit: None,
            completed_at: None,
            split_from_task_id: None,
            original_quantity_total: None,
            actual_minutes: None,
            created_at: "2025-06-01T00:00:00Z".to_string(),
            updated_at: "2025-06-01T00:00:00Z".to_string(),
        }
    }

    fn habit_row(id: &str, display_id: i64, title: &str) -> HabitRow {
        HabitRow {
            id: id.to_string(),
            display_id,
            title: title.to_string(),
            description: None,
            recurrence: "FREQ=DAILY".to_string(),
            start_time: "08:00".to_string(),
            end_time: "09:00".to_string(),
            avg_minutes: 60,
            sigma_minutes: 10,
            parallelizable: false,
            allows_parallel: false,
            abandonability: 0.5,
            active: true,
            fixed: false,
            window_mode: "day".to_string(),
            created_at: "2025-06-01T00:00:00Z".to_string(),
            updated_at: "2025-06-01T00:00:00Z".to_string(),
        }
    }

    fn step_row(id: &str, habit_id: &str, position: i64, title: &str) -> HabitStepRow {
        HabitStepRow {
            id: id.to_string(),
            habit_id: habit_id.to_string(),
            position,
            title: title.to_string(),
            description: None,
            start_time: "08:00".to_string(),
            end_time: "09:00".to_string(),
            avg_minutes: 30,
            sigma_minutes: 5,
            parallelizable: false,
            allows_parallel: false,
            abandonability: 0.5,
            fixed: false,
            depends_on: "[]".to_string(),
            created_at: "2025-06-01T00:00:00Z".to_string(),
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
        second.depends_on = r#"["step-1"]"#.to_string();
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
    fn normalize_mutation_field_interprets_naive_datetime_in_server_timezone() {
        let tz = jiff::tz::TimeZone::get("Asia/Tokyo").unwrap();
        let mut args = serde_json::Map::new();
        args.insert(
            "end_at".to_string(),
            Value::String("2025-06-05T14:00".to_string()),
        );

        normalize_mutation_field(&mut args, "end_at", &tz).unwrap();

        // 2025-06-05 14:00 JST == 2025-06-05 05:00 UTC
        assert!(
            args["end_at"]
                .as_str()
                .unwrap()
                .starts_with("2025-06-05T05:00:00")
        );
        assert!(args["end_at"].as_str().unwrap().ends_with('Z'));
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
    fn normalize_execution_references_strips_hashes_for_backend() {
        let tz = jiff::tz::TimeZone::get("UTC").unwrap();
        let mut display_args = serde_json::Map::new();
        display_args.insert("task_ref".to_string(), Value::String("#42".to_string()));
        display_args.insert("depends".to_string(), json!(["#1", "h2#3"]));

        normalize_mutation_args(MutationKind::UpdateTask, &mut display_args, &tz).unwrap();

        let mut execution_args = display_args.clone();
        normalize_execution_references(MutationKind::UpdateTask, &mut execution_args).unwrap();

        assert_eq!(display_args["task_ref"], "#42");
        assert_eq!(display_args["depends"], json!(["#1", "h2#3"]));
        assert_eq!(execution_args["task_ref"], "42");
        assert_eq!(execution_args["depends"], json!(["1", "h2#3"]));
    }

    #[test]
    fn normalize_status_maps_common_synonyms() {
        assert_eq!(normalize_status("done"), "completed");
        assert_eq!(normalize_status("Done"), "completed");
        assert_eq!(normalize_status("  DONE  "), "completed");
        assert_eq!(normalize_status("complete"), "completed");
        assert_eq!(normalize_status("in-progress"), "in_progress");
        assert_eq!(normalize_status("in progress"), "in_progress");
        assert_eq!(normalize_status("todo"), "pending");
        assert_eq!(normalize_status("skip"), "skipped");
        assert_eq!(normalize_status("completed"), "completed");
        assert_eq!(normalize_status("pending"), "pending");
    }

    #[test]
    fn normalize_mutation_args_normalizes_status_for_update_task() {
        let tz = jiff::tz::TimeZone::get("UTC").unwrap();
        let mut args = serde_json::Map::new();
        args.insert("status".to_string(), Value::String("done".to_string()));

        normalize_mutation_args(MutationKind::UpdateTask, &mut args, &tz).unwrap();

        assert_eq!(args["status"], "completed");
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
        let tool = MutationTool {
            client: client.clone(),
            tz_cache: TimeZoneCache::new(client),
            kind: MutationKind::UpdateTask,
        };
        let schema = tool.parameters_schema();
        let values: Vec<String> =
            serde_json::from_value(schema["properties"]["status"]["enum"].clone()).unwrap();
        assert!(values.contains(&"completed".to_string()));
    }

    #[test]
    fn change_summary_covers_all_kinds_and_fallbacks() {
        fn args(pairs: &[(&str, &str)]) -> serde_json::Map<String, Value> {
            pairs
                .iter()
                .map(|(k, v)| (k.to_string(), Value::String(v.to_string())))
                .collect()
        }

        assert_eq!(
            MutationKind::CreateTask.change_summary(&args(&[("title", "演習30題追加")])),
            (
                "演習30題追加".to_owned(),
                "「演習30題追加」を作成".to_owned()
            ),
        );
        assert_eq!(
            MutationKind::UpdateTask
                .change_summary(&args(&[("task_ref", "#42"), ("title", "予習")])),
            ("#42".to_owned(), "「予習」を更新".to_owned()),
        );
        assert_eq!(
            MutationKind::UpdateTask.change_summary(&args(&[("task_ref", "#42")])),
            ("#42".to_owned(), "#42を更新".to_owned()),
        );
        assert_eq!(
            MutationKind::DeleteTask.change_summary(&args(&[("task_ref", "#7")])),
            ("#7".to_owned(), "#7を削除".to_owned()),
        );
        assert_eq!(
            MutationKind::CreateHabit.change_summary(&args(&[("title", "毎朝ジョギング")])),
            (
                "毎朝ジョギング".to_owned(),
                "「毎朝ジョギング」を作成".to_owned()
            ),
        );
        assert_eq!(
            MutationKind::UpdateHabit
                .change_summary(&args(&[("habit_ref", "h3"), ("title", "夜ジョギング")])),
            ("h3".to_owned(), "「夜ジョギング」を更新".to_owned()),
        );
        assert_eq!(
            MutationKind::DeleteHabit.change_summary(&args(&[("habit_ref", "h1")])),
            ("h1".to_owned(), "h1を削除".to_owned()),
        );
        assert_eq!(
            MutationKind::GenerateSchedule.change_summary(&serde_json::Map::new()),
            (String::new(), "スケジュールを生成".to_owned()),
        );
        assert_eq!(
            MutationKind::Reschedule.change_summary(&serde_json::Map::new()),
            (String::new(), "スケジュールを再調整".to_owned()),
        );

        let mut blank_title = serde_json::Map::new();
        blank_title.insert("title".to_string(), Value::String("   ".to_string()));
        assert_eq!(
            MutationKind::CreateTask.change_summary(&blank_title),
            (
                "(名称未設定)".to_owned(),
                "「(名称未設定)」を作成".to_owned()
            ),
        );
        assert_eq!(
            MutationKind::UpdateTask.change_summary(&serde_json::Map::new()),
            ("(参照不明)".to_owned(), "(参照不明)を更新".to_owned()),
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
        task.end_at = "2026-07-20T10:00:00Z".to_string();

        let before = jiff::Timestamp::from_str("2026-07-20T09:00:00Z").unwrap();
        let after = jiff::Timestamp::from_str("2026-07-20T11:00:00Z").unwrap();

        assert!(overdue_in_range(&task, Some(before), Some(after), &tz));
        assert!(!overdue_in_range(&task, Some(after), None, &tz));
        assert!(!overdue_in_range(&task, None, Some(before), &tz));

        // Reversed range excludes everything.
        assert!(!overdue_in_range(&task, Some(after), Some(before), &tz));

        // Unparseable deadline fails closed.
        task.end_at = "not-a-date".to_string();
        assert!(!overdue_in_range(&task, Some(before), Some(after), &tz));
    }

    #[tokio::test]
    async fn move_task_tool_proposes_move_with_existing_entry() {
        let task = Arc::new(task_row("task-uuid", 42, "買い物", None, &[]));
        let task_for_get = task.as_ref().clone();
        let task_for_list = vec![task.as_ref().clone()];
        let habits = vec![habit_row("habit-uuid", 1, "朝のランニング")];
        let schedule = serde_json::to_string(&json!([{
            "task_id": "task-uuid",
            "start_at": "2025-06-05T18:00:00Z",
            "end_at": "2025-06-05T18:30:00Z",
        }]))
        .unwrap();
        let schedule_row = ScheduleRow {
            id: "sched-1".into(),
            created_at: "2025-06-01T00:00:00Z".into(),
            updated_at: "2025-06-01T00:00:00Z".into(),
            schedule,
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
                        sleep_start: "23:00".into(),
                        sleep_end: "07:00".into(),
                        comfortable_minutes: None,
                        maximum_minutes: None,
                        solver: "auto".into(),
                        time_budget_ms: None,
                        seed: None,
                        warm_start: false,
                    })
                }),
            );

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let client = Client::new(&format!("http://{addr}"), "");
        let tz_cache = TimeZoneCache::new(client.clone());
        let tool = MoveTaskTool { client, tz_cache };
        let args = json!({"task_ref": "#42", "start_at": "2025-06-05T19:00:00+09:00"});
        let output = tool.call(args).await.unwrap();

        assert_eq!(output.proposed_changes.len(), 1);
        let change = &output.proposed_changes[0];
        assert_eq!(change.operation, "move");
        assert_eq!(change.target_label, "task #42");

        let before = change.before.as_ref().unwrap();
        assert_eq!(before["schedule_start_at"], "2025-06-05T18:00:00Z");
        assert_eq!(before["schedule_end_at"], "2025-06-05T18:30:00Z");

        let after = change.after.as_ref().unwrap().as_object().unwrap();
        assert_eq!(after["task_ref"], "#42");
        assert!(after.get("end_at").is_some());

        let execution = change.arguments.as_ref().unwrap().as_object().unwrap();
        assert_eq!(execution["task_ref"], "42");
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
            start_date: "2025-08-01".into(),
            end_date: "2025-08-07".into(),
            reason: Some("旅行".into()),
            created_at: "2025-06-01T00:00:00Z".into(),
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
                        sleep_start: "23:00".into(),
                        sleep_end: "07:00".into(),
                        comfortable_minutes: None,
                        maximum_minutes: None,
                        solver: "auto".into(),
                        time_budget_ms: None,
                        seed: None,
                        warm_start: false,
                    })
                }),
            );

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let client = Client::new(&format!("http://{addr}"), "");
        let tz_cache = TimeZoneCache::new(client.clone());
        let tool = HabitScheduledSpans { client, tz_cache };

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
        assert_eq!(change.operation, "create_scheduled_span");
        assert_eq!(change.target_label, "habit h1");
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
        assert_eq!(change.operation, "delete_scheduled_span");
        assert_eq!(change.target_label, "habit h1");
        let before = change.before.as_ref().unwrap();
        assert_eq!(before["start_date"], "2025-08-01");
        let args = change.arguments.as_ref().unwrap().as_object().unwrap();
        assert_eq!(args["span_id"], "span-uuid");
        assert_eq!(args["habit_ref"], "h1");
    }
}
