use std::collections::HashMap;
use std::env;
use std::fmt;
use std::fs;
use std::io;
use std::process::Command;

use serde::{Deserialize, Serialize};
use takusu_contracts::{HabitRow, HabitStepInput, HabitStepRow, TaskRow, UpdateHabit, UpdateTask};
use takusu_types::{
    parse_datetime_to_timestamp, parse_duration, Abandonability, Quantity, TaskStatus, TimeOfDay,
    Timestamp, WindowMode,
};

use crate::task_ref::task_reference;

const MAX_EDIT_ATTEMPTS: usize = 5;

/// Errors that can occur when editing a file in `$EDITOR`.
#[derive(Debug)]
pub enum EditorError {
    /// The user saved an empty file or made no changes.
    Canceled,
    /// The editor exited with an error or the user exceeded retry attempts.
    Other(String),
}

impl fmt::Display for EditorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EditorError::Canceled => write!(f, "edit canceled"),
            EditorError::Other(msg) => write!(f, "{msg}"),
        }
    }
}

impl std::error::Error for EditorError {}

impl From<io::Error> for EditorError {
    fn from(e: io::Error) -> Self {
        EditorError::Other(e.to_string())
    }
}

// ── Task editor ──────────────────────────────────────────────────────────

#[derive(Debug, Default, Serialize, Deserialize)]
struct TaskEditFile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "at")]
    start_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "due")]
    end_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "time")]
    avg_time: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "sigma")]
    sigma_time: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    depends: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    advanced: Option<TaskAdvanced>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct TaskAdvanced {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    parallelizable: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    allows_parallel: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    abandonability: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    fixed: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    quantity_total: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    quantity_done: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    quantity_unit: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    original_quantity_total: Option<i64>,
}

fn fmt_ts(ts: Option<&Timestamp>, tz: &jiff::tz::TimeZone) -> Option<String> {
    ts.map(|t| {
        let zdt = t.to_zoned(tz.clone());
        let date = zdt.date();
        let time = zdt.time();
        format!(
            "{date} {:02}:{:02}:{:02}",
            time.hour(),
            time.minute(),
            time.second()
        )
    })
}

fn fmt_duration(minutes: i64) -> String {
    if minutes == 0 {
        return "0".to_string();
    }
    let sign = if minutes < 0 { "-" } else { "" };
    let abs_minutes = minutes.unsigned_abs() as i64;
    let hours = abs_minutes / 60;
    let mins = abs_minutes % 60;
    let body = if hours > 0 && mins > 0 {
        format!("{hours}h{mins}m")
    } else if hours > 0 {
        format!("{hours}h")
    } else {
        format!("{mins}m")
    };
    format!("{sign}{body}")
}

fn build_task_edit_file(
    task: &TaskRow,
    all_tasks: &[TaskRow],
    habit_map: &HashMap<String, i64>,
    tz: &jiff::tz::TimeZone,
) -> TaskEditFile {
    let depends_uuids: Vec<String> = task.depends.to_vec();
    let depends: Vec<String> = depends_uuids
        .iter()
        .map(|uuid| {
            all_tasks
                .iter()
                .find(|t| &t.id == uuid)
                .map(|t| task_reference(t, habit_map))
                .unwrap_or_else(|| uuid.clone())
        })
        .collect();

    TaskEditFile {
        title: Some(task.title.clone()),
        description: task.description.clone(),
        start_at: fmt_ts(task.start_at.as_ref(), tz),
        end_at: fmt_ts(Some(&task.end_at), tz),
        avg_time: Some(fmt_duration(task.avg_minutes)),
        sigma_time: Some(if task.sigma_minutes == 0 {
            "0".to_string()
        } else {
            fmt_duration(task.sigma_minutes)
        }),
        status: Some(task.status.to_string()),
        depends: Some(depends),
        advanced: Some(TaskAdvanced {
            parallelizable: Some(task.parallelizable),
            allows_parallel: Some(task.allows_parallel),
            abandonability: Some(task.abandonability.into()),
            fixed: Some(task.fixed),
            quantity_total: task.quantity_total.map(|q| q.get()),
            quantity_done: Some(task.quantity_done.get()),
            quantity_unit: task.quantity_unit.clone(),
            original_quantity_total: task.original_quantity_total.map(|q| q.get()),
        }),
    }
}

fn task_toml_header(task_ref: &str) -> String {
    format!(
        "# Edit task {task_ref}. Lines starting with '#' are comments.\n\
         # Remove a field (or its whole value) to leave it unchanged.\n\
         # Empty strings for description and quantity_unit clear those fields.\n\
         # Time values use h/m/s (e.g. \"1h30m\", \"30m\", \"0\"); 0 = auto (time/5).\n\
         # Datetime values use e.g. \"2025-06-05 23:59:00\" in the configured timezone.\n"
    )
}

fn parse_task_edit_file(file: &TaskEditFile, tz: &jiff::tz::TimeZone) -> Result<UpdateTask, String> {
    let title = match file.title.as_deref() {
        Some(s) if !s.trim().is_empty() => Some(s.to_string()),
        Some(_) => return Err("title cannot be empty".to_string()),
        None => None,
    };

    let start_at = match file.start_at.as_deref() {
        None | Some("") => None,
        Some(s) => Some(Some(
            parse_datetime_to_timestamp(s, tz)
                .map(Timestamp::from)
                .map_err(|e| format!("invalid at '{s}': {e}"))?,
        )),
    };

    let end_at = match file.end_at.as_deref() {
        None | Some("") => None,
        Some(s) => Some(
            parse_datetime_to_timestamp(s, tz)
                .map(Timestamp::from)
                .map_err(|e| format!("invalid due '{s}': {e}"))?,
        ),
    };

    let avg_minutes = match file.avg_time.as_deref() {
        None | Some("") => None,
        Some(s) => Some(
            parse_duration(s).map_err(|e| format!("invalid time '{s}': {e}"))?,
        ),
    };

    let sigma_minutes = match file.sigma_time.as_deref() {
        None | Some("") => None,
        Some(s) => Some(
            parse_duration(s).map_err(|e| format!("invalid sigma '{s}': {e}"))?,
        ),
    };

    let status = match file.status.as_deref() {
        None | Some("") => None,
        Some(s) => Some(
            s.parse::<TaskStatus>()
                .map_err(|e| format!("invalid status '{s}': {e}"))?,
        ),
    };

    let depends = file.depends.as_ref().map(|items| {
        items
            .iter()
            .map(|s| s.trim().trim_start_matches('#').to_string())
            .filter(|s| !s.is_empty())
            .collect()
    });

    let default_advanced = TaskAdvanced::default();
    let advanced = file.advanced.as_ref().unwrap_or(&default_advanced);

    let quantity_total = advanced
        .quantity_total
        .map(|v| Quantity::new(v).map_err(|e| format!("invalid quantity_total: {e}")))
        .transpose()?;
    let quantity_done = advanced
        .quantity_done
        .map(|v| Quantity::new(v).map_err(|e| format!("invalid quantity_done: {e}")))
        .transpose()?;
    let original_quantity_total = advanced
        .original_quantity_total
        .map(|v| Quantity::new(v).map_err(|e| format!("invalid original_quantity_total: {e}")))
        .transpose()?;

    Ok(UpdateTask {
        title,
        description: file.description.clone(),
        start_at,
        end_at,
        avg_minutes,
        sigma_minutes,
        depends,
        parallelizable: advanced.parallelizable,
        allows_parallel: advanced.allows_parallel,
        abandonability: advanced.abandonability.map(Abandonability::new),
        status,
        habit_id: None,
        user_edited: None,
        fixed: advanced.fixed,
        habit_step_id: None,
        quantity_total,
        quantity_done,
        quantity_unit: advanced.quantity_unit.clone(),
        original_quantity_total,
    })
}

pub fn edit_task(
    task: &TaskRow,
    all_tasks: &[TaskRow],
    habit_map: &HashMap<String, i64>,
    tz: &jiff::tz::TimeZone,
) -> Result<UpdateTask, EditorError> {
    let file = build_task_edit_file(task, all_tasks, habit_map, tz);
    let toml = toml::to_string_pretty(&file).map_err(|e| EditorError::Other(e.to_string()))?;
    let ref_str = task_reference(task, habit_map);
    let content = task_toml_header(&ref_str) + &toml;
    let suffix = sanitize_suffix(&task.id);

    edit_loop(&content, &suffix, |edited| {
        let file: TaskEditFile =
            toml::from_str(edited).map_err(|e| format!("invalid TOML: {e}"))?;
        parse_task_edit_file(&file, tz)
    })
}

// ── Habit editor ─────────────────────────────────────────────────────────

#[derive(Debug, Default, Serialize, Deserialize)]
struct HabitEditFile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    recurrence: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "start_time")]
    start_time: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "end_time")]
    end_time: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "time")]
    avg_time: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "sigma")]
    sigma_time: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    active: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    window: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    advanced: Option<HabitAdvanced>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct HabitAdvanced {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    parallelizable: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    allows_parallel: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    abandonability: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    fixed: Option<bool>,
}

fn build_habit_edit_file(habit: &HabitRow) -> HabitEditFile {
    HabitEditFile {
        title: Some(habit.title.clone()),
        description: habit.description.clone(),
        recurrence: Some(habit.recurrence.clone()),
        start_time: Some(habit.start_time.to_string()),
        end_time: Some(habit.end_time.to_string()),
        avg_time: Some(fmt_duration(habit.avg_minutes)),
        sigma_time: Some(if habit.sigma_minutes == 0 {
            "0".to_string()
        } else {
            fmt_duration(habit.sigma_minutes)
        }),
        active: Some(habit.active),
        window: Some(habit.window_mode.to_string()),
        advanced: Some(HabitAdvanced {
            parallelizable: Some(habit.parallelizable),
            allows_parallel: Some(habit.allows_parallel),
            abandonability: Some(habit.abandonability.into()),
            fixed: Some(habit.fixed),
        }),
    }
}

fn habit_toml_header(habit_ref: &str) -> String {
    format!(
        "# Edit habit {habit_ref}. Lines starting with '#' are comments.\n\
         # Remove a field (or its whole value) to leave it unchanged.\n\
         # Empty strings for description clear that field.\n\
         # Time values use h/m/s (e.g. \"1h30m\", \"30m\", \"0\"); 0 = auto (time/5).\n\
         # Times of day use HH:MM (e.g. \"09:00\").\n\
         # Window mode is either \"day\" or \"period\".\n"
    )
}

fn parse_habit_edit_file(file: &HabitEditFile) -> Result<UpdateHabit, String> {
    let title = match file.title.as_deref() {
        Some(s) if !s.trim().is_empty() => Some(s.to_string()),
        Some(_) => return Err("title cannot be empty".to_string()),
        None => None,
    };

    let start_time = match file.start_time.as_deref() {
        None | Some("") => None,
        Some(s) => Some(
            s.parse::<TimeOfDay>().map_err(|e| format!("invalid start_time '{s}': {e}"))?,
        ),
    };

    let end_time = match file.end_time.as_deref() {
        None | Some("") => None,
        Some(s) => Some(
            s.parse::<TimeOfDay>().map_err(|e| format!("invalid end_time '{s}': {e}"))?,
        ),
    };

    let avg_minutes = match file.avg_time.as_deref() {
        None | Some("") => None,
        Some(s) => Some(
            parse_duration(s).map_err(|e| format!("invalid time '{s}': {e}"))?,
        ),
    };

    let sigma_minutes = match file.sigma_time.as_deref() {
        None | Some("") => None,
        Some(s) => Some(
            parse_duration(s).map_err(|e| format!("invalid sigma '{s}': {e}"))?,
        ),
    };

    let window_mode = match file.window.as_deref() {
        None | Some("") => None,
        Some(s) => Some(
            s.parse::<WindowMode>()
                .map_err(|e| format!("invalid window '{s}': {e}"))?,
        ),
    };

    let default_advanced = HabitAdvanced::default();
    let advanced = file.advanced.as_ref().unwrap_or(&default_advanced);

    Ok(UpdateHabit {
        title,
        description: file.description.clone(),
        recurrence: file.recurrence.clone(),
        start_time,
        end_time,
        avg_minutes,
        sigma_minutes,
        parallelizable: advanced.parallelizable,
        allows_parallel: advanced.allows_parallel,
        abandonability: advanced.abandonability.map(Abandonability::new),
        active: file.active,
        fixed: advanced.fixed,
        window_mode,
    })
}

pub fn edit_habit(habit: &HabitRow) -> Result<UpdateHabit, EditorError> {
    let file = build_habit_edit_file(habit);
    let toml = toml::to_string_pretty(&file).map_err(|e| EditorError::Other(e.to_string()))?;
    let content = habit_toml_header(&format!("h{}", habit.display_id)) + &toml;
    let suffix = sanitize_suffix(&habit.id);

    edit_loop(&content, &suffix, |edited| {
        let file: HabitEditFile =
            toml::from_str(edited).map_err(|e| format!("invalid TOML: {e}"))?;
        parse_habit_edit_file(&file)
    })
}

// ── Habit steps editor ───────────────────────────────────────────────────

#[derive(Debug, Default, Serialize, Deserialize)]
struct StepsEditFile {
    #[serde(default, rename = "step")]
    steps: Vec<StepEditFile>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct StepEditFile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    id: Option<String>,
    position: i64,
    title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "start_time")]
    start_time: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "end_time")]
    end_time: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "time")]
    avg_time: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "sigma")]
    sigma_time: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    parallelizable: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    allows_parallel: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    abandonability: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    fixed: Option<bool>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    depends_on: Vec<String>,
}

fn step_to_edit_file(step: &HabitStepRow) -> StepEditFile {
    StepEditFile {
        id: Some(step.id.clone()),
        position: step.position,
        title: step.title.clone(),
        description: step.description.clone(),
        start_time: Some(step.start_time.to_string()),
        end_time: Some(step.end_time.to_string()),
        avg_time: Some(fmt_duration(step.avg_minutes)),
        sigma_time: Some(if step.sigma_minutes == 0 {
            "0".to_string()
        } else {
            fmt_duration(step.sigma_minutes)
        }),
        parallelizable: Some(step.parallelizable),
        allows_parallel: Some(step.allows_parallel),
        abandonability: Some(step.abandonability.into()),
        fixed: Some(step.fixed),
        depends_on: step.depends_on.to_vec(),
    }
}

fn build_steps_edit_file(steps: &[HabitStepRow]) -> StepsEditFile {
    StepsEditFile {
        steps: steps.iter().map(step_to_edit_file).collect(),
    }
}

fn steps_toml_header(habit_ref: &str) -> String {
    format!(
        "# Edit habit steps for {habit_ref}. Lines starting with '#' are comments.\n\
         # Each [[step]] table is one step, in order.\n\
         # Omit or leave 'id' empty to create a new step.\n\
         # Time values use h/m/s (e.g. \"15m\", \"0\"); 0 = auto (time/5).\n\
         # Times of day use HH:MM (e.g. \"09:00\").\n"
    )
}

fn parse_step_edit_file(step: &StepEditFile) -> Result<HabitStepInput, String> {
    let id = step
        .id
        .as_ref()
        .and_then(|s| {
            let s = s.trim();
            if s.is_empty() { None } else { Some(s.to_string()) }
        });

    let title = if step.title.trim().is_empty() {
        return Err("step title cannot be empty".to_string());
    } else {
        step.title.clone()
    };

    let start_time = step
        .start_time
        .as_deref()
        .filter(|s| !s.is_empty())
        .ok_or("step start_time is required")?
        .parse::<TimeOfDay>()
        .map_err(|e| format!("invalid step start_time '{}': {e}", step.start_time.as_deref().unwrap_or("")))?;

    let end_time = step
        .end_time
        .as_deref()
        .filter(|s| !s.is_empty())
        .ok_or("step end_time is required")?
        .parse::<TimeOfDay>()
        .map_err(|e| format!("invalid step end_time '{}': {e}", step.end_time.as_deref().unwrap_or("")))?;

    let avg_time = step
        .avg_time
        .as_deref()
        .filter(|s| !s.is_empty())
        .ok_or("step time is required")?;
    let avg_minutes = parse_duration(avg_time)
        .map_err(|e| format!("invalid step time '{avg_time}': {e}"))?;

    let sigma_minutes = step
        .sigma_time
        .as_deref()
        .map(|s| {
            if s.is_empty() {
                Ok(0)
            } else {
                parse_duration(s)
            }
        })
        .unwrap_or(Ok(0))?;

    Ok(HabitStepInput {
        id,
        position: step.position,
        title,
        description: step.description.clone(),
        start_time,
        end_time,
        avg_minutes,
        sigma_minutes: Some(sigma_minutes),
        parallelizable: step.parallelizable,
        allows_parallel: step.allows_parallel,
        abandonability: step.abandonability.map(Abandonability::new),
        fixed: step.fixed,
        depends_on: step.depends_on.clone(),
    })
}

fn parse_steps_edit_file(file: &StepsEditFile) -> Result<Vec<HabitStepInput>, String> {
    file.steps
        .iter()
        .enumerate()
        .map(|(i, step)| {
            parse_step_edit_file(step).map_err(|e| format!("step {}: {e}", i + 1))
        })
        .collect()
}

pub fn edit_steps(
    habit_ref: &str,
    steps: &[HabitStepRow],
) -> Result<Vec<HabitStepInput>, EditorError> {
    let file = build_steps_edit_file(steps);
    let toml = toml::to_string_pretty(&file).map_err(|e| EditorError::Other(e.to_string()))?;
    let content = steps_toml_header(habit_ref) + &toml;
    let suffix = format!("{}", uuid::Uuid::now_v7());

    edit_loop(&content, &suffix, |edited| {
        let file: StepsEditFile =
            toml::from_str(edited).map_err(|e| format!("invalid TOML: {e}"))?;
        parse_steps_edit_file(&file)
    })
}

pub fn parse_edited_steps(content: &str) -> Result<Vec<HabitStepInput>, String> {
    let file: StepsEditFile = toml::from_str(content).map_err(|e| format!("invalid TOML: {e}"))?;
    parse_steps_edit_file(&file)
}

// ── Generic editor loop ──────────────────────────────────────────────────

fn sanitize_suffix(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_alphanumeric() { c } else { '_' })
        .collect()
}

pub fn open_editor(content: &str, suffix: &str, ext: &str) -> io::Result<String> {
    let dir = env::temp_dir();
    let path = dir.join(format!("takusu-edit-{suffix}.{ext}"));
    fs::write(&path, content)?;

    let editor = env::var("EDITOR").unwrap_or_else(|_| "vi".to_string());
    let parts: Vec<&str> = editor.split_whitespace().collect();
    if parts.is_empty() {
        return Err(io::Error::other("EDITOR is empty"));
    }
    let mut cmd = Command::new(parts[0]);
    cmd.args(&parts[1..]);
    cmd.arg(&path);
    let status = cmd.status()?;

    if !status.success() {
        fs::remove_file(&path).ok();
        return Err(io::Error::other("editor exited with non-zero status"));
    }

    let edited = fs::read_to_string(&path)?;
    fs::remove_file(&path).ok();
    Ok(edited)
}

fn prepend_error_comment(content: &str, error: &str) -> String {
    let mut out = String::new();
    for line in error.lines() {
        out.push_str("# ERROR: ");
        out.push_str(line);
        out.push('\n');
    }
    out.push_str("# Fix the error above and save. Lines starting with '#' are ignored.\n");
    out.push('\n');
    out.push_str(content);
    out
}

fn strip_error_comments(content: &str) -> String {
    let mut out = Vec::new();
    let mut skip_blank = false;
    for line in content.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("# ERROR:")
            || trimmed.starts_with("# Fix the error above")
        {
            skip_blank = true;
            continue;
        }
        if skip_blank && line.trim().is_empty() {
            continue;
        }
        skip_blank = false;
        out.push(line);
    }
    let mut result = out.join("\n");
    result = result.trim_start().to_string();
    if !result.is_empty() {
        result.push('\n');
    }
    result
}

fn edit_loop<T>(
    initial_content: &str,
    suffix: &str,
    parse: impl Fn(&str) -> Result<T, String>,
) -> Result<T, EditorError> {
    let mut content = initial_content.to_string();
    let mut last_error = String::new();

    for _ in 0..MAX_EDIT_ATTEMPTS {
        let current = if last_error.is_empty() {
            content.clone()
        } else {
            prepend_error_comment(&content, &last_error)
        };

        let edited = open_editor(&current, suffix, "toml")?;

        if edited.trim().is_empty() {
            return Err(EditorError::Canceled);
        }
        if edited == initial_content {
            return Err(EditorError::Canceled);
        }
        if edited == content {
            return Err(EditorError::Canceled);
        }

        match parse(&edited) {
            Ok(value) => return Ok(value),
            Err(e) => {
                last_error = e;
                content = strip_error_comments(&edited);
            }
        }
    }

    Err(EditorError::Other(format!(
        "too many edit attempts; last error: {last_error}"
    )))
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

    fn habit_row() -> HabitRow {
        HabitRow {
            id: "habit-1".into(),
            display_id: 1,
            title: "Morning jog".into(),
            description: None,
            recurrence: "RRULE:FREQ=DAILY".into(),
            start_time: "07:00".parse().unwrap(),
            end_time: "07:30".parse().unwrap(),
            avg_minutes: 30,
            sigma_minutes: 5,
            parallelizable: false,
            allows_parallel: false,
            abandonability: 0.5.into(),
            active: true,
            fixed: false,
            window_mode: WindowMode::Day,
            created_at: "2026-07-23T00:00:00Z".parse().unwrap(),
            updated_at: "2026-07-23T00:00:00Z".parse().unwrap(),
        }
    }

    fn step_row() -> HabitStepRow {
        HabitStepRow {
            id: "step-1".into(),
            habit_id: "habit-1".into(),
            position: 1,
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
        }
    }

    #[test]
    fn parse_task_edit_file_empty_optional_fields_are_skipped() {
        let file = TaskEditFile {
            title: Some("t".into()),
            ..Default::default()
        };
        let update = parse_task_edit_file(&file, &jiff::tz::TimeZone::UTC).unwrap();
        assert_eq!(update.title.as_deref(), Some("t"));
        assert_eq!(update.description, None);
        assert_eq!(update.end_at, None);
        assert_eq!(update.avg_minutes, None);
    }

    #[test]
    fn parse_task_edit_file_due_and_time_use_cli_formats() {
        let file = TaskEditFile {
            title: Some("t".into()),
            end_at: Some("2026-07-06 18:00".into()),
            avg_time: Some("1h30m".into()),
            sigma_time: Some("0".into()),
            ..Default::default()
        };
        let update = parse_task_edit_file(&file, &jiff::tz::TimeZone::UTC).unwrap();
        assert_eq!(
            update.end_at.map(|t| t.to_string()),
            Some("2026-07-06T18:00:00Z".to_string())
        );
        assert_eq!(update.avg_minutes, Some(90));
        assert_eq!(update.sigma_minutes, Some(0));
    }

    #[test]
    fn parse_task_edit_file_empty_string_clears_description() {
        let file = TaskEditFile {
            title: Some("t".into()),
            description: Some("".into()),
            ..Default::default()
        };
        let update = parse_task_edit_file(&file, &jiff::tz::TimeZone::UTC).unwrap();
        assert_eq!(update.description, Some("".to_string()));
    }

    #[test]
    fn parse_task_edit_file_quantity_fields_round_trip() {
        let file = TaskEditFile {
            title: Some("t".into()),
            advanced: Some(TaskAdvanced {
                quantity_total: Some(10),
                quantity_done: Some(3),
                quantity_unit: Some("pages".into()),
                original_quantity_total: Some(10),
                ..Default::default()
            }),
            ..Default::default()
        };
        let update = parse_task_edit_file(&file, &jiff::tz::TimeZone::UTC).unwrap();
        assert_eq!(update.quantity_total.map(|q| q.get()), Some(10));
        assert_eq!(update.quantity_done.map(|q| q.get()), Some(3));
        assert_eq!(update.quantity_unit, Some("pages".into()));
        assert_eq!(update.original_quantity_total.map(|q| q.get()), Some(10));
    }

    #[test]
    fn parse_task_edit_file_invalid_value_reports_field() {
        let file = TaskEditFile {
            title: Some("t".into()),
            avg_time: Some("abc".into()),
            ..Default::default()
        };
        let err = parse_task_edit_file(&file, &jiff::tz::TimeZone::UTC).unwrap_err();
        assert!(err.contains("time"), "error should mention the field: {err}");
        assert!(err.contains("abc"), "error should mention the bad value: {err}");
    }

    #[test]
    fn parse_task_edit_file_preserves_habit_dependency_reference() {
        let file = TaskEditFile {
            title: Some("t".into()),
            depends: Some(vec!["h1#5".into(), "#3".into(), "task-uuid".into()]),
            ..Default::default()
        };
        let update = parse_task_edit_file(&file, &jiff::tz::TimeZone::UTC).unwrap();
        assert_eq!(
            update.depends,
            Some(vec!["h1#5".to_string(), "3".to_string(), "task-uuid".to_string()])
        );
    }

    #[test]
    fn build_task_edit_file_uses_habit_scoped_reference() {
        let mut habit_map = HashMap::new();
        habit_map.insert("habit-1".into(), 7);

        let standalone = task_row("task-a", 3, "standalone", None, &[]);
        let habit_task = task_row("task-b", 3, "habit task", Some("habit-1"), &[]);
        let edited = task_row("edited", 1, "edited", None, &["task-a", "task-b"]);

        let file = build_task_edit_file(&edited, &[standalone, habit_task], &habit_map, &jiff::tz::TimeZone::UTC);
        assert_eq!(file.depends, Some(vec!["#3".into(), "h7#3".into()]));
    }

    #[test]
    fn parse_habit_edit_file_empty_optional_fields_are_skipped() {
        let file = HabitEditFile {
            title: Some("h".into()),
            ..Default::default()
        };
        let update = parse_habit_edit_file(&file).unwrap();
        assert_eq!(update.title.as_deref(), Some("h"));
        assert_eq!(update.start_time, None);
        assert_eq!(update.avg_minutes, None);
    }

    #[test]
    fn parse_habit_edit_file_time_and_window_use_cli_formats() {
        let file = HabitEditFile {
            title: Some("h".into()),
            start_time: Some("09:00".into()),
            end_time: Some("10:00".into()),
            avg_time: Some("30m".into()),
            window: Some("period".into()),
            ..Default::default()
        };
        let update = parse_habit_edit_file(&file).unwrap();
        assert_eq!(update.start_time.map(|t| t.to_string()), Some("09:00".to_string()));
        assert_eq!(update.end_time.map(|t| t.to_string()), Some("10:00".to_string()));
        assert_eq!(update.avg_minutes, Some(30));
        assert_eq!(update.window_mode, Some(WindowMode::Period));
    }

    #[test]
    fn parse_habit_edit_file_invalid_window_reports_error() {
        let file = HabitEditFile {
            title: Some("h".into()),
            window: Some("weekly".into()),
            ..Default::default()
        };
        let err = parse_habit_edit_file(&file).unwrap_err();
        assert!(err.contains("window"), "error: {err}");
    }

    #[test]
    fn parse_steps_edit_file_round_trip() {
        let row = step_row();
        let file = build_steps_edit_file(&[row]);
        let parsed = parse_steps_edit_file(&file).unwrap();
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].id.as_deref(), Some("step-1"));
        assert_eq!(parsed[0].position, 1);
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
    fn parse_steps_edit_file_allows_new_step_without_id() {
        let file = StepsEditFile {
            steps: vec![StepEditFile {
                position: 1,
                title: "New".into(),
                start_time: Some("09:00".into()),
                end_time: Some("09:30".into()),
                avg_time: Some("15m".into()),
                ..Default::default()
            }],
        };
        let parsed = parse_steps_edit_file(&file).unwrap();
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].id, None);
        assert_eq!(parsed[0].title, "New");
        assert_eq!(parsed[0].sigma_minutes, Some(0));
    }

    #[test]
    fn parse_steps_edit_file_missing_required_field_reports_error() {
        let file = StepsEditFile {
            steps: vec![StepEditFile {
                position: 1,
                title: "Bad".into(),
                ..Default::default()
            }],
        };
        let err = parse_steps_edit_file(&file).unwrap_err();
        assert!(err.contains("step 1"), "error: {err}");
        assert!(err.contains("start_time"), "error: {err}");
    }

    #[test]
    fn parse_steps_edit_file_invalid_time_reports_field() {
        let file = StepsEditFile {
            steps: vec![StepEditFile {
                position: 1,
                title: "Bad".into(),
                start_time: Some("09:00".into()),
                end_time: Some("09:30".into()),
                avg_time: Some("abc".into()),
                ..Default::default()
            }],
        };
        let err = parse_steps_edit_file(&file).unwrap_err();
        assert!(err.contains("step 1"), "error: {err}");
        assert!(err.contains("time"), "error: {err}");
        assert!(err.contains("abc"), "error: {err}");
    }

    #[test]
    fn build_habit_edit_file_round_trip() {
        let habit = habit_row();
        let file = build_habit_edit_file(&habit);
        assert_eq!(file.title, Some("Morning jog".into()));
        assert_eq!(file.start_time, Some("07:00".into()));
        assert_eq!(file.advanced.as_ref().unwrap().fixed, Some(false));
    }

    #[test]
    fn fmt_duration_zero_is_plain_zero() {
        assert_eq!(fmt_duration(0), "0");
    }

    #[test]
    fn fmt_duration_hours_and_minutes() {
        assert_eq!(fmt_duration(90), "1h30m");
    }

    #[test]
    fn fmt_duration_only_hours() {
        assert_eq!(fmt_duration(120), "2h");
    }

    #[test]
    fn fmt_duration_only_minutes() {
        assert_eq!(fmt_duration(45), "45m");
    }
}
