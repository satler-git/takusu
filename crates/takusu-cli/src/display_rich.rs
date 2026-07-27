use comfy_table::{Cell, Color, ContentArrangement, Table, presets::UTF8_FULL};
use takusu_habit::{RecurrenceRule, summarize};
use takusu_storage::{
    HabitRow, HabitScheduledSpanRow, HabitStepRow, ScheduleEntry, SkillRow, TaskRow, TokenRow,
};
use takusu_util::{TaskStatus, Timestamp, WindowMode};

use crate::task_ref::task_reference;

/// Parse a recurrence JSON string into a human-readable summary.
/// Falls back to the raw string if parsing fails.
fn format_recurrence(raw: &str) -> String {
    serde_json::from_str::<RecurrenceRule>(raw)
        .map(|r| summarize(&r))
        .unwrap_or_else(|_| raw.to_string())
}

pub fn display_task_detail(
    task: &TaskRow,
    entry: Option<&ScheduleEntry>,
    tz: &jiff::tz::TimeZone,
    habit_map: &std::collections::HashMap<String, i64>,
) {
    let status_color = match task.status {
        TaskStatus::Pending => Color::Yellow,
        TaskStatus::Scheduled => Color::Green,
        TaskStatus::InProgress => Color::DarkYellow,
        TaskStatus::Completed => Color::DarkCyan,
        TaskStatus::Skipped => Color::DarkGrey,
    };

    let mut table = Table::new();
    table
        .load_preset(UTF8_FULL)
        .set_content_arrangement(ContentArrangement::Dynamic);
    let progress = if let Some(total) = task.quantity_total {
        format!(
            "{}/{} {}",
            task.quantity_done,
            total,
            task.quantity_unit.as_deref().unwrap_or("")
        )
    } else {
        "—".into()
    };
    let completed = task
        .completed_at
        .as_ref()
        .map(|t| format_datetime(t, tz))
        .unwrap_or_else(|| "—".into());

    table.set_header(vec![
        Cell::new("ID").fg(Color::Cyan),
        Cell::new("Title").fg(Color::Cyan),
        Cell::new("Status").fg(Color::Cyan),
        Cell::new("Start").fg(Color::Cyan),
        Cell::new("Deadline").fg(Color::Cyan),
        Cell::new("Avg (min)").fg(Color::Cyan),
        Cell::new("σ (min)").fg(Color::Cyan),
        Cell::new("Parallel").fg(Color::Cyan),
        Cell::new("Host").fg(Color::Cyan),
        Cell::new("Abandon").fg(Color::Cyan),
        Cell::new("Progress").fg(Color::Cyan),
        Cell::new("Completed").fg(Color::Cyan),
    ]);
    table.add_row(vec![
        Cell::new(task_reference(task, habit_map)),
        Cell::new(&task.title),
        Cell::new(task.status).fg(status_color),
        Cell::new(
            task.start_at
                .as_ref()
                .map(|t| format_datetime(t, tz))
                .unwrap_or_else(|| "—".into()),
        ),
        Cell::new(format_datetime(&task.end_at, tz)),
        Cell::new(task.avg_minutes),
        Cell::new(task.sigma_minutes),
        Cell::new(if task.parallelizable { "✓" } else { "✗" }),
        Cell::new(if task.allows_parallel { "✓" } else { "✗" }),
        Cell::new(format!("{:.1}", task.abandonability)),
        Cell::new(progress),
        Cell::new(completed),
    ]);
    println!("{table}");

    if let Some(entry) = entry {
        println!();
        let mut table = Table::new();
        table
            .load_preset(UTF8_FULL)
            .set_content_arrangement(ContentArrangement::Dynamic);
        table.set_header(vec![
            Cell::new("Start").fg(Color::Cyan),
            Cell::new("End").fg(Color::Cyan),
            Cell::new("Duration").fg(Color::Cyan),
        ]);
        let start = format_datetime(&entry.start_at, tz);
        let end = format_datetime(&entry.end_at, tz);
        let dur = format_duration(&entry.start_at, &entry.end_at);
        table.add_row(vec![Cell::new(start), Cell::new(end), Cell::new(dur)]);
        println!("{table}");
    }
}

pub fn display_habits(habits: &[HabitRow]) {
    if habits.is_empty() {
        println!("No habits found.");
        return;
    }

    let mut table = Table::new();
    table
        .load_preset(UTF8_FULL)
        .set_content_arrangement(ContentArrangement::Dynamic)
        .set_header(vec![
            Cell::new("ID").fg(Color::Cyan),
            Cell::new("Title").fg(Color::Cyan),
            Cell::new("Recurrence").fg(Color::Cyan),
            Cell::new("Time").fg(Color::Cyan),
            Cell::new("Avg (min)").fg(Color::Cyan),
            Cell::new("σ (min)").fg(Color::Cyan),
            Cell::new("Parallel").fg(Color::Cyan),
            Cell::new("Host").fg(Color::Cyan),
            Cell::new("Abandon").fg(Color::Cyan),
            Cell::new("Active").fg(Color::Cyan),
        ]);

    for h in habits {
        let short_id = format!("h{}", h.display_id);
        let time = format!("{}–{}", h.start_time, h.end_time);
        let active_color = if h.active {
            Color::Green
        } else {
            Color::DarkGrey
        };
        let active_text = if h.active { "yes" } else { "no" };
        table.add_row(vec![
            Cell::new(short_id),
            Cell::new(&h.title),
            Cell::new(format_recurrence(&h.recurrence)),
            Cell::new(time),
            Cell::new(h.avg_minutes),
            Cell::new(h.sigma_minutes),
            Cell::new(if h.parallelizable { "✓" } else { "✗" }),
            Cell::new(if h.allows_parallel { "✓" } else { "✗" }),
            Cell::new(format!("{:.1}", h.abandonability)),
            Cell::new(active_text).fg(active_color),
        ]);
    }
    println!("{table}");
}

pub fn display_habit_detail(habit: &HabitRow) {
    let active_color = if habit.active {
        Color::Green
    } else {
        Color::DarkGrey
    };
    let active_text = if habit.active { "yes" } else { "no" };

    let mut table = Table::new();
    table
        .load_preset(UTF8_FULL)
        .set_content_arrangement(ContentArrangement::Dynamic);
    table.set_header(vec![
        Cell::new("ID").fg(Color::Cyan),
        Cell::new("Title").fg(Color::Cyan),
        Cell::new("Recurrence").fg(Color::Cyan),
        Cell::new("Time").fg(Color::Cyan),
        Cell::new("Avg (min)").fg(Color::Cyan),
        Cell::new("σ (min)").fg(Color::Cyan),
        Cell::new("Parallel").fg(Color::Cyan),
        Cell::new("Host").fg(Color::Cyan),
        Cell::new("Abandon").fg(Color::Cyan),
        Cell::new("Active").fg(Color::Cyan),
    ]);
    let time = format!("{}–{}", habit.start_time, habit.end_time);
    table.add_row(vec![
        Cell::new(format!("h{}", habit.display_id)),
        Cell::new(&habit.title),
        Cell::new(format_recurrence(&habit.recurrence)),
        Cell::new(time),
        Cell::new(habit.avg_minutes),
        Cell::new(habit.sigma_minutes),
        Cell::new(if habit.parallelizable { "✓" } else { "✗" }),
        Cell::new(if habit.allows_parallel { "✓" } else { "✗" }),
        Cell::new(format!("{:.1}", habit.abandonability)),
        Cell::new(active_text).fg(active_color),
    ]);
    println!("{table}");

    if let Some(ref desc) = habit.description
        && !desc.is_empty()
    {
        println!("\nDescription: {desc}");
    }
    if habit.window_mode == WindowMode::Period {
        println!("\nWindow: period (schedulable anywhere until next occurrence)");
    }
}

pub fn display_habit_steps(steps: &[HabitStepRow]) {
    if steps.is_empty() {
        println!("No steps found.");
        return;
    }

    let mut table = Table::new();
    table
        .load_preset(UTF8_FULL)
        .set_content_arrangement(ContentArrangement::Dynamic)
        .set_header(vec![
            Cell::new("ID").fg(Color::Cyan),
            Cell::new("Pos").fg(Color::Cyan),
            Cell::new("Title").fg(Color::Cyan),
            Cell::new("Time").fg(Color::Cyan),
            Cell::new("Avg (min)").fg(Color::Cyan),
            Cell::new("σ (min)").fg(Color::Cyan),
            Cell::new("Parallel").fg(Color::Cyan),
            Cell::new("Host").fg(Color::Cyan),
            Cell::new("Abandon").fg(Color::Cyan),
            Cell::new("Depends").fg(Color::Cyan),
        ]);

    for s in steps {
        let deps: Vec<String> = serde_json::from_str(&s.depends_on).unwrap_or_default();
        let deps_str = deps.join(",");
        let id_short: String = s.id.chars().take(8).collect();
        table.add_row(vec![
            Cell::new(id_short),
            Cell::new(s.position),
            Cell::new(&s.title),
            Cell::new(format!("{}–{}", s.start_time, s.end_time)),
            Cell::new(s.avg_minutes),
            Cell::new(s.sigma_minutes),
            Cell::new(if s.parallelizable { "✓" } else { "✗" }),
            Cell::new(if s.allows_parallel { "✓" } else { "✗" }),
            Cell::new(format!("{:.1}", s.abandonability)),
            Cell::new(if deps_str.is_empty() {
                "-".into()
            } else {
                deps_str
            }),
        ]);
    }
    println!("{table}");
}

fn habit_label_by_id<'a>(habit_id: &'a str, habits: &'a [HabitRow]) -> (&'a str, i64, &'a str) {
    let habit = habits.iter().find(|h| h.id == habit_id);
    match habit {
        Some(h) => (&h.title, h.display_id, &h.id),
        None => ("(unknown)", 0, habit_id),
    }
}

pub fn display_all_habit_scheduled_spans(spans: &[HabitScheduledSpanRow], habits: &[HabitRow]) {
    if spans.is_empty() {
        println!("No scheduled spans found.");
        return;
    }

    let mut table = Table::new();
    table
        .load_preset(UTF8_FULL)
        .set_content_arrangement(ContentArrangement::Dynamic)
        .set_header(vec![
            Cell::new("Habit").fg(Color::Cyan),
            Cell::new("ID").fg(Color::Cyan),
            Cell::new("Start").fg(Color::Cyan),
            Cell::new("End").fg(Color::Cyan),
            Cell::new("Reason").fg(Color::Cyan),
        ]);

    for s in spans {
        let (title, display_id, _id) = habit_label_by_id(&s.habit_id, habits);
        table.add_row(vec![
            Cell::new(format!("h{} {}", display_id, title)),
            Cell::new(&s.id),
            Cell::new(s.start_date),
            Cell::new(s.end_date),
            Cell::new(s.reason.as_deref().unwrap_or("")),
        ]);
    }
    println!("{table}");
}

pub fn display_all_habit_steps(steps: &[HabitStepRow], habits: &[HabitRow]) {
    if steps.is_empty() {
        println!("No steps found.");
        return;
    }

    let mut table = Table::new();
    table
        .load_preset(UTF8_FULL)
        .set_content_arrangement(ContentArrangement::Dynamic)
        .set_header(vec![
            Cell::new("Habit").fg(Color::Cyan),
            Cell::new("Pos").fg(Color::Cyan),
            Cell::new("Title").fg(Color::Cyan),
            Cell::new("Time").fg(Color::Cyan),
            Cell::new("Avg (min)").fg(Color::Cyan),
            Cell::new("σ (min)").fg(Color::Cyan),
            Cell::new("Parallel").fg(Color::Cyan),
            Cell::new("Abandon").fg(Color::Cyan),
            Cell::new("Depends").fg(Color::Cyan),
        ]);

    for s in steps {
        let (title, display_id, _id) = habit_label_by_id(&s.habit_id, habits);
        let deps: Vec<String> = serde_json::from_str(&s.depends_on).unwrap_or_default();
        let deps_str = deps.join(",");
        table.add_row(vec![
            Cell::new(format!("h{} {}", display_id, title)),
            Cell::new(s.position),
            Cell::new(&s.title),
            Cell::new(format!("{}–{}", s.start_time, s.end_time)),
            Cell::new(s.avg_minutes),
            Cell::new(s.sigma_minutes),
            Cell::new(if s.parallelizable { "✓" } else { "✗" }),
            Cell::new(format!("{:.1}", s.abandonability)),
            Cell::new(if deps_str.is_empty() {
                "-".into()
            } else {
                deps_str
            }),
        ]);
    }
    println!("{table}");
}

pub fn display_tasks(
    tasks: &[TaskRow],
    tz: &jiff::tz::TimeZone,
    habit_map: &std::collections::HashMap<String, i64>,
) {
    if tasks.is_empty() {
        println!("No tasks found.");
        return;
    }

    let mut table = Table::new();
    table
        .load_preset(UTF8_FULL)
        .set_content_arrangement(ContentArrangement::Dynamic)
        .set_header(vec![
            Cell::new("ID").fg(Color::Cyan),
            Cell::new("Title").fg(Color::Cyan),
            Cell::new("Status").fg(Color::Cyan),
            Cell::new("Start").fg(Color::Cyan),
            Cell::new("Deadline").fg(Color::Cyan),
            Cell::new("Avg (min)").fg(Color::Cyan),
            Cell::new("σ (min)").fg(Color::Cyan),
            Cell::new("Parallel").fg(Color::Cyan),
            Cell::new("Host").fg(Color::Cyan),
            Cell::new("Abandon").fg(Color::Cyan),
            Cell::new("Progress").fg(Color::Cyan),
            Cell::new("Completed").fg(Color::Cyan),
        ]);

    for t in tasks {
        let status_color = match t.status {
            TaskStatus::Pending => Color::Yellow,
            TaskStatus::Scheduled => Color::Green,
            TaskStatus::InProgress => Color::DarkYellow,
            TaskStatus::Completed => Color::DarkCyan,
            TaskStatus::Skipped => Color::DarkGrey,
        };
        let progress = if let Some(total) = t.quantity_total {
            format!(
                "{}/{} {}",
                t.quantity_done,
                total,
                t.quantity_unit.as_deref().unwrap_or("")
            )
        } else {
            "—".into()
        };
        let completed = t
            .completed_at
            .as_ref()
            .map(|t| format_datetime(t, tz))
            .unwrap_or_else(|| "—".into());
        let short_id = task_reference(t, habit_map);
        table.add_row(vec![
            Cell::new(short_id),
            Cell::new(&t.title),
            Cell::new(t.status).fg(status_color),
            Cell::new(
                t.start_at
                    .as_ref()
                    .map(|t| format_datetime(t, tz))
                    .unwrap_or_else(|| "—".into()),
            ),
            Cell::new(format_datetime(&t.end_at, tz)),
            Cell::new(t.avg_minutes),
            Cell::new(t.sigma_minutes),
            Cell::new(if t.parallelizable { "✓" } else { "✗" }),
            Cell::new(if t.allows_parallel { "✓" } else { "✗" }),
            Cell::new(format!("{:.1}", t.abandonability)),
            Cell::new(progress),
            Cell::new(completed),
        ]);
    }
    println!("{table}");
}

pub fn display_schedule(
    entries: &[ScheduleEntry],
    tasks: &[TaskRow],
    tz: &jiff::tz::TimeZone,
    habit_map: &std::collections::HashMap<String, i64>,
) {
    if entries.is_empty() {
        println!("No schedule found.");
        return;
    }

    let mut sorted = entries.to_vec();
    sorted.sort_by_key(|e| e.start_at);

    let mut table = Table::new();
    table
        .load_preset(UTF8_FULL)
        .set_content_arrangement(ContentArrangement::Dynamic)
        .set_header(vec![
            Cell::new("#").fg(Color::Cyan),
            Cell::new("Title").fg(Color::Cyan),
            Cell::new("Task ID").fg(Color::Cyan),
            Cell::new("Start").fg(Color::Cyan),
            Cell::new("End").fg(Color::Cyan),
            Cell::new("Duration").fg(Color::Cyan),
        ]);

    let task_map: std::collections::HashMap<&str, &TaskRow> =
        tasks.iter().map(|t| (t.id.as_str(), t)).collect();

    for (i, e) in sorted.iter().enumerate() {
        let task = task_map.get(e.task_id.as_str());
        let title = task.map(|t| t.title.as_str()).unwrap_or("(unknown)");
        let id_label = task
            .map(|t| task_reference(t, habit_map))
            .unwrap_or_else(|| e.task_id[..8].to_string());
        let start = format_datetime(&e.start_at, tz);
        let end = format_datetime(&e.end_at, tz);
        let dur = format_duration(&e.start_at, &e.end_at);
        table.add_row(vec![
            Cell::new(i + 1),
            Cell::new(title),
            Cell::new(id_label),
            Cell::new(start),
            Cell::new(end),
            Cell::new(dur),
        ]);
    }
    println!("{table}");
}

fn format_datetime(ts: &Timestamp, tz: &jiff::tz::TimeZone) -> String {
    let zdt = ts.to_zoned(tz.clone());
    zdt.strftime("%m/%d %H:%M").to_string()
}

fn format_duration(start: &Timestamp, end: &Timestamp) -> String {
    let secs = (end.as_second() - start.as_second()).unsigned_abs();
    let mins = secs / 60;
    if mins >= 60 {
        format!("{}h{}m", mins / 60, mins % 60)
    } else {
        format!("{mins}m")
    }
}

pub fn display_tokens(tokens: &[TokenRow]) {
    if tokens.is_empty() {
        println!("No tokens found.");
        return;
    }

    let mut table = Table::new();
    table
        .load_preset(UTF8_FULL)
        .set_content_arrangement(ContentArrangement::Dynamic)
        .set_header(vec![
            Cell::new("ID").fg(Color::Cyan),
            Cell::new("Label").fg(Color::Cyan),
            Cell::new("Created By").fg(Color::Cyan),
            Cell::new("Created At").fg(Color::Cyan),
            Cell::new("Revoked").fg(Color::Cyan),
        ]);

    for t in tokens {
        let revoked = t
            .revoked_at
            .as_ref()
            .map(|_| Cell::new("YES").fg(Color::Red))
            .unwrap_or_else(|| Cell::new("no").fg(Color::Green));
        table.add_row(vec![
            Cell::new(t.id),
            Cell::new(t.label.as_deref().unwrap_or("—")),
            Cell::new(&t.created_by[..8]),
            Cell::new(t.created_at),
            revoked,
        ]);
    }
    println!("{table}");
}

pub fn display_skills(skills: &[SkillRow]) {
    if skills.is_empty() {
        println!("No skills found.");
        return;
    }

    let mut table = Table::new();
    table
        .load_preset(UTF8_FULL)
        .set_content_arrangement(ContentArrangement::Dynamic)
        .set_header(vec![
            Cell::new("Slug").fg(Color::Cyan),
            Cell::new("Name").fg(Color::Cyan),
            Cell::new("Description").fg(Color::Cyan),
            Cell::new("Built-in").fg(Color::Cyan),
        ]);

    for s in skills {
        table.add_row(vec![
            Cell::new(&s.slug),
            Cell::new(&s.name),
            Cell::new(&s.description).fg(Color::DarkGrey),
            Cell::new(if s.built_in { "yes" } else { "no" }),
        ]);
    }
    println!("{table}");
}

pub fn display_skill_detail(skill: &SkillRow) {
    let mut table = Table::new();
    table
        .load_preset(UTF8_FULL)
        .set_content_arrangement(ContentArrangement::Dynamic);
    table.set_header(vec![
        Cell::new("Slug").fg(Color::Cyan),
        Cell::new("Name").fg(Color::Cyan),
        Cell::new("Description").fg(Color::Cyan),
        Cell::new("Built-in").fg(Color::Cyan),
        Cell::new("Created").fg(Color::Cyan),
        Cell::new("Updated").fg(Color::Cyan),
    ]);
    table.add_row(vec![
        Cell::new(&skill.slug),
        Cell::new(&skill.name),
        Cell::new(&skill.description),
        Cell::new(if skill.built_in { "yes" } else { "no" }),
        Cell::new(skill.created_at),
        Cell::new(skill.updated_at),
    ]);
    println!("{table}");
    println!("\n{}", skill.body);
}
