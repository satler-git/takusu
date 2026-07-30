//! Display reference helpers for tasks.
use std::collections::HashMap;
use takusu_contracts::TaskRow;

/// Build the display reference for a task.
/// Habit-generated tasks show `h{habit_display_id}#{task_display_id}` (#305 / #933);
/// other tasks show `#{task_display_id}`.
pub fn task_reference(task: &TaskRow, habit_map: &HashMap<String, i64>) -> String {
    task.habit_id
        .as_ref()
        .and_then(|habit_id| habit_map.get(habit_id))
        .map(|habit_display_id| format!("h{habit_display_id}#{}", task.display_id))
        .unwrap_or_else(|| format!("#{}", task.display_id))
}
