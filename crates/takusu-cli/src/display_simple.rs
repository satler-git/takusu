//! Plain-text renderer for the CLI's display commands.
//!
//! All data transformation shared with [`crate::display_rich`] lives in
//! [`crate::display_common`]; this module only owns the line-oriented output
//! and the simple-specific datetime format.
use std::collections::HashMap;
use takusu_storage::{
    HabitRow, HabitScheduledSpanRow, HabitStepRow, ScheduleEntry, SkillRow, TaskRow, TokenRow,
};
use takusu_util::{TaskStatus, Timestamp};

use crate::display_common::{DisplayFormatter, format_duration, habit_label_by_id, progress_text};
use crate::task_ref::task_reference;

/// Plain-text renderer for [`crate::DisplayMode::Simple`].
#[derive(Clone, Copy)]
pub struct SimpleFormatter;

fn fmt_simple(ts: &Timestamp, tz: &jiff::tz::TimeZone) -> String {
    let zdt = ts.to_zoned(tz.clone());
    zdt.strftime("%d %H:%M").to_string()
}

/// Plain-text status marker for a task status.
fn status_marker(status: TaskStatus) -> &'static str {
    match status {
        TaskStatus::Pending => "[ ]",
        TaskStatus::Scheduled => "[~]",
        TaskStatus::InProgress => "[>]",
        TaskStatus::Completed => "[x]",
        TaskStatus::Skipped => "[-]",
    }
}

impl DisplayFormatter for SimpleFormatter {
    fn display_task_detail(
        &self,
        task: &TaskRow,
        entry: Option<&ScheduleEntry>,
        tz: &jiff::tz::TimeZone,
        habit_map: &HashMap<String, i64>,
    ) {
        let marker = status_marker(task.status);
        println!(
            "{} {} {}",
            marker,
            task_reference(task, habit_map),
            task.title
        );
        println!(
            "   deadline: {} | est: {}min (+/-{}) | abandon: {:.1} | parallel: {} | host: {}",
            fmt_simple(&task.end_at, tz),
            task.avg_minutes,
            task.sigma_minutes,
            task.abandonability,
            if task.parallelizable { "yes" } else { "no" },
            if task.allows_parallel { "yes" } else { "no" },
        );
        if let Some(ref start) = task.start_at {
            println!("   start: {}", fmt_simple(start, tz));
        }
        if let Some(ref desc) = task.description {
            println!("   {desc}");
        }
        if let Some(p) = progress_text(task) {
            println!("   progress: {p}");
        }
        if let Some(ref completed) = task.completed_at {
            println!("   completed: {}", fmt_simple(completed, tz));
        }

        if let Some(entry) = entry {
            println!(
                "   scheduled: {} -- {} ({})",
                fmt_simple(&entry.start_at, tz),
                fmt_simple(&entry.end_at, tz),
                format_duration(&entry.start_at, &entry.end_at)
            );
        }
        println!();
    }

    fn display_tasks(
        &self,
        tasks: &[TaskRow],
        tz: &jiff::tz::TimeZone,
        habit_map: &HashMap<String, i64>,
    ) {
        if tasks.is_empty() {
            println!("  (no tasks)");
            return;
        }

        for t in tasks {
            let marker = status_marker(t.status);
            let short_id = task_reference(t, habit_map);
            println!("{} {} {}", marker, short_id, t.title);
            println!(
                "   deadline: {} | est: {}min (+/-{}) | abandon: {:.1} | parallel: {} | host: {}",
                fmt_simple(&t.end_at, tz),
                t.avg_minutes,
                t.sigma_minutes,
                t.abandonability,
                if t.parallelizable { "yes" } else { "no" },
                if t.allows_parallel { "yes" } else { "no" },
            );
            if let Some(ref desc) = t.description {
                println!("   {desc}");
            }
            if let Some(p) = progress_text(t) {
                println!("   progress: {p}");
            }
            if let Some(ref completed) = t.completed_at {
                println!("   completed: {}", fmt_simple(completed, tz));
            }
            println!();
        }
    }

    fn display_schedule(
        &self,
        entries: &[ScheduleEntry],
        tasks: &[TaskRow],
        tz: &jiff::tz::TimeZone,
        habit_map: &HashMap<String, i64>,
    ) {
        if entries.is_empty() {
            println!("  (no schedule)");
            return;
        }

        let mut sorted = entries.to_vec();
        sorted.sort_by_key(|e| e.start_at);

        let task_map: HashMap<&str, &TaskRow> = tasks.iter().map(|t| (t.id.as_str(), t)).collect();

        for (i, e) in sorted.iter().enumerate() {
            let task = task_map.get(e.task_id.as_str());
            let title = task.map(|t| t.title.as_str()).unwrap_or("(unknown)");
            let id_label = task
                .map(|t| task_reference(t, habit_map))
                .unwrap_or_else(|| e.task_id[..8].to_string());
            let start = fmt_simple(&e.start_at, tz);
            let end = fmt_simple(&e.end_at, tz);
            let dur = format_duration(&e.start_at, &e.end_at);
            println!("  {:>3}. {} -- {} [{}] {}", i + 1, start, end, dur, title);
            println!("       id: {}", id_label);
        }
    }

    fn display_tokens(&self, tokens: &[TokenRow]) {
        if tokens.is_empty() {
            println!("  (no tokens)");
            return;
        }
        for t in tokens {
            let revoked = t.revoked_at.as_ref().map(|_| " [REVOKED]").unwrap_or("");
            println!(
                "  #{} {:8}  {}{}",
                t.id,
                t.label.as_deref().unwrap_or("-"),
                &t.created_at,
                revoked
            );
        }
    }

    fn display_habits(&self, habits: &[HabitRow]) {
        if habits.is_empty() {
            println!("  (no habits)");
            return;
        }

        for h in habits {
            let active = if h.active { "active" } else { "inactive" };
            let short_id = format!("h{}", h.display_id);
            println!(
                "  {} {} [{}] {}–{} {}",
                short_id, h.title, h.recurrence, h.start_time, h.end_time, active
            );
            println!(
                "   est: {}min (+/-{}) | abandon: {:.1} | parallel: {} | host: {}",
                h.avg_minutes,
                h.sigma_minutes,
                h.abandonability,
                if h.parallelizable { "yes" } else { "no" },
                if h.allows_parallel { "yes" } else { "no" },
            );
            if let Some(ref desc) = h.description
                && !desc.is_empty()
            {
                println!("   {desc}");
            }
            println!();
        }
    }

    fn display_habit_detail(&self, habit: &HabitRow) {
        let active = if habit.active { "active" } else { "inactive" };
        println!(
            "h{} {} [{}] {}–{} {}",
            habit.display_id,
            habit.title,
            habit.recurrence,
            habit.start_time,
            habit.end_time,
            active
        );
        println!(
            "   est: {}min (+/-{}) | abandon: {:.1} | parallel: {} | host: {} | window: {}",
            habit.avg_minutes,
            habit.sigma_minutes,
            habit.abandonability,
            if habit.parallelizable { "yes" } else { "no" },
            if habit.allows_parallel { "yes" } else { "no" },
            habit.window_mode,
        );
        if let Some(ref desc) = habit.description
            && !desc.is_empty()
        {
            println!("   {desc}");
        }
        println!();
    }

    fn display_habit_steps(&self, steps: &[HabitStepRow]) {
        if steps.is_empty() {
            println!("  (no steps)");
            return;
        }

        for s in steps {
            let deps: Vec<String> = serde_json::from_str(&s.depends_on).unwrap_or_default();
            let deps_str = if deps.is_empty() {
                String::new()
            } else {
                format!(" ← {}", deps.join(","))
            };
            println!(
                "  {} [{}] {} ({}–{}, {}min) parallel: {} host: {}{}",
                s.id,
                s.position,
                s.title,
                s.start_time,
                s.end_time,
                s.avg_minutes,
                if s.parallelizable { "yes" } else { "no" },
                if s.allows_parallel { "yes" } else { "no" },
                deps_str
            );
            if let Some(ref desc) = s.description
                && !desc.is_empty()
            {
                println!("     {desc}");
            }
        }
    }

    fn display_skills(&self, skills: &[SkillRow]) {
        if skills.is_empty() {
            println!("  (no skills)");
            return;
        }
        for s in skills {
            let marker = if s.built_in { "[b]" } else { "[u]" };
            println!("{} {} {}: {}", marker, s.slug, s.name, s.description);
        }
    }

    fn display_skill_detail(&self, skill: &SkillRow) {
        let marker = if skill.built_in { "built-in" } else { "user" };
        println!("{} {} ({})", skill.slug, skill.name, marker);
        println!("  {}\n", skill.description);
        println!("{}", skill.body);
    }

    fn display_all_habit_scheduled_spans(
        &self,
        spans: &[HabitScheduledSpanRow],
        habits: &[HabitRow],
    ) {
        if spans.is_empty() {
            println!("  (no scheduled spans)");
            return;
        }
        for s in spans {
            let (title, display_id, _id) = habit_label_by_id(&s.habit_id, habits);
            println!(
                "h{} {}\t{}\t{}..{}\t{}",
                display_id,
                title,
                s.id,
                s.start_date,
                s.end_date,
                s.reason.as_deref().unwrap_or("")
            );
        }
    }

    fn display_all_habit_steps(&self, steps: &[HabitStepRow], habits: &[HabitRow]) {
        if steps.is_empty() {
            println!("  (no steps)");
            return;
        }
        for s in steps {
            let (title, display_id, _id) = habit_label_by_id(&s.habit_id, habits);
            let deps: Vec<String> = serde_json::from_str(&s.depends_on).unwrap_or_default();
            let deps_str = if deps.is_empty() {
                String::new()
            } else {
                format!(" ← {}", deps.join(","))
            };
            println!(
                "h{} {}\t{} [{}] {} ({}–{}, {}min) parallel: {}{}",
                display_id,
                title,
                s.id,
                s.position,
                s.title,
                s.start_time,
                s.end_time,
                s.avg_minutes,
                if s.parallelizable { "yes" } else { "no" },
                deps_str
            );
        }
    }
}
