//! Shared display helpers and the `DisplayFormatter` trait.
//!
//! `display_rich` and `display_simple` render the same domain data into
//! terminal output; the only difference is whether they use `comfy-table`
//! (rich) or plain text (simple). This module owns the data-transformation
//! logic that is identical for both renderers — progress text, duration
//! formatting, habit label lookup — and defines the trait that each renderer
//! implements so call sites can dispatch with `mode.formatter().display_*(...)`
//! instead of repeating a `match` arm per renderer at every call site.
//!
//! Renderer-specific helpers (e.g. rich's parsed recurrence summary, simple's
//! plain-text status markers) live in their respective renderer modules.
use std::collections::HashMap;

use takusu_storage::{
    HabitRow, HabitScheduledSpanRow, HabitStepRow, ScheduleEntry, SkillRow, TaskRow, TokenRow,
};
use takusu_types::Timestamp;

/// Terminal renderer for the CLI's display commands.
///
/// `RichFormatter` (`comfy-table`) and `SimpleFormatter` (plain text) are the
/// only implementations. Call sites obtain the active renderer via
/// [`crate::DisplayMode::formatter`] and call the relevant method; the
/// renderer is responsible for all output, while shared data transformation
/// lives in the free functions below.
pub trait DisplayFormatter {
    fn display_task_detail(
        &self,
        task: &TaskRow,
        entry: Option<&ScheduleEntry>,
        tz: &jiff::tz::TimeZone,
        habit_map: &HashMap<String, i64>,
    );
    fn display_tasks(
        &self,
        tasks: &[TaskRow],
        tz: &jiff::tz::TimeZone,
        habit_map: &HashMap<String, i64>,
    );
    fn display_habits(&self, habits: &[HabitRow]);
    fn display_habit_detail(&self, habit: &HabitRow);
    fn display_habit_steps(&self, steps: &[HabitStepRow]);
    fn display_all_habit_scheduled_spans(
        &self,
        spans: &[HabitScheduledSpanRow],
        habits: &[HabitRow],
    );
    fn display_all_habit_steps(&self, steps: &[HabitStepRow], habits: &[HabitRow]);
    fn display_schedule(
        &self,
        entries: &[ScheduleEntry],
        tasks: &[TaskRow],
        tz: &jiff::tz::TimeZone,
        habit_map: &HashMap<String, i64>,
    );
    fn display_tokens(&self, tokens: &[TokenRow]);
    fn display_skills(&self, skills: &[SkillRow]);
    fn display_skill_detail(&self, skill: &SkillRow);
}

/// Format a task's progress as `done/total unit`, or `None` when the task has
/// no quantity target. Renderers decide how to present the missing case (e.g.
/// the rich renderer shows `—` in the progress column, the simple renderer
/// omits the line entirely).
pub fn progress_text(task: &TaskRow) -> Option<String> {
    task.quantity_total.map(|total| {
        format!(
            "{}/{} {}",
            task.quantity_done,
            total,
            task.quantity_unit.as_deref().unwrap_or("")
        )
    })
}

/// Format the span between two timestamps as e.g. `1h30m` or `45m`.
pub fn format_duration(start: &Timestamp, end: &Timestamp) -> String {
    let secs = (end.as_second() - start.as_second()).unsigned_abs();
    let mins = secs / 60;
    if mins >= 60 {
        format!("{}h{}m", mins / 60, mins % 60)
    } else {
        format!("{mins}m")
    }
}

/// Look up a habit's display label by id, returning `(title, display_id, id)`.
/// Falls back to `("(unknown)", 0, habit_id)` when the habit is not found.
pub fn habit_label_by_id<'a>(habit_id: &'a str, habits: &'a [HabitRow]) -> (&'a str, i64, &'a str) {
    habits
        .iter()
        .find(|h| h.id == habit_id)
        .map(|h| (h.title.as_str(), h.display_id, h.id.as_str()))
        .unwrap_or(("(unknown)", 0, habit_id))
}
