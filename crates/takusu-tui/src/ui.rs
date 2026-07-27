use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Tabs, Wrap};
use takusu_storage::{ScheduleEntry, TaskRow};
use takusu_util::{EnumLabel, TaskStatus, WindowMode};

use crate::app::{App, Modal, Tab};
use crate::style;
use crate::widgets::detail::{fmt_day, fmt_dt, render_task_detail, render_text_detail};

pub fn draw(frame: &mut Frame, app: &mut App) {
    let area = frame.area();

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(0),
            Constraint::Length(1),
        ])
        .split(area);

    draw_tabs(frame, app, chunks[0]);
    draw_body(frame, app, chunks[1]);
    draw_status_bar(frame, app, chunks[2]);

    match app.modal {
        Modal::Help => draw_help(frame, area),
        Modal::ConfirmDelete => draw_confirm(frame, area),
        Modal::CreateTask { field } => draw_create_task(frame, area, app, field),
        Modal::None => {}
    }
}

fn draw_tabs(frame: &mut Frame, app: &App, area: Rect) {
    let titles: Vec<Line> = Tab::ALL
        .iter()
        .map(|t| {
            let style = if *t == app.tab {
                style::TAB_ACTIVE
            } else {
                style::TAB_INACTIVE
            };
            Line::from(Span::styled(t.title(), style))
        })
        .collect();

    let idx = Tab::ALL.iter().position(|t| *t == app.tab).unwrap_or(0);
    let tabs = Tabs::new(titles)
        .select(idx)
        .block(Block::default().borders(Borders::ALL).title(" takusu "))
        .highlight_style(Style::default());
    frame.render_widget(tabs, area);
}

fn draw_body(frame: &mut Frame, app: &mut App, area: Rect) {
    match app.tab {
        Tab::Schedule => draw_schedule_tab(frame, app, area),
        Tab::Tasks => draw_tasks_tab(frame, app, area),
        Tab::Habits => draw_habits_tab(frame, app, area),
        Tab::Settings => draw_settings_tab(frame, app, area),
    }
}

fn draw_schedule_tab(frame: &mut Frame, app: &mut App, area: Rect) {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(55), Constraint::Percentage(45)])
        .split(area);

    let list_height = chunks[0].height.saturating_sub(2) as usize;

    let (items, entry_to_item) =
        build_schedule_items(&app.schedule_entries, &app.tz, &app.all_tasks);

    let selected_item = app
        .schedule_list
        .selected()
        .and_then(|i| entry_to_item.get(i).copied());

    if let Some(si) = selected_item {
        app.schedule_list.ensure_visible_item(si, list_height);
    }

    let list = List::new(items)
        .block(Block::default().borders(Borders::ALL).title(" Schedule "))
        .highlight_style(style::selected_style());

    let mut state = ratatui::widgets::ListState::default()
        .with_selected(selected_item)
        .with_offset(app.schedule_list.scroll);
    frame.render_stateful_widget(list, chunks[0], &mut state);

    if let Some(entry) = app.selected_entry() {
        if let Some(task) = app.task_by_id(&entry.task_id) {
            render_task_detail(frame, chunks[1], task, &app.tz);
        } else {
            let lines = vec![
                Line::from(Span::raw(format!(
                    "Task: {}",
                    &entry.task_id[..8.min(entry.task_id.len())]
                ))),
                Line::from(Span::raw(format!(
                    "Start: {}",
                    fmt_dt(&entry.start_at, &app.tz)
                ))),
                Line::from(Span::raw(format!(
                    "End: {}",
                    fmt_dt(&entry.end_at, &app.tz)
                ))),
            ];
            render_text_detail(frame, chunks[1], "Entry", lines);
        }
    } else {
        let p = Paragraph::new("(no schedule)")
            .block(Block::default().borders(Borders::ALL).title(" Detail "));
        frame.render_widget(p, chunks[1]);
    }
}

fn build_schedule_items(
    entries: &[ScheduleEntry],
    tz: &jiff::tz::TimeZone,
    all_tasks: &[TaskRow],
) -> (Vec<ListItem<'static>>, Vec<usize>) {
    let mut items: Vec<ListItem<'static>> = Vec::new();
    let mut entry_to_item: Vec<usize> = Vec::with_capacity(entries.len());
    let mut prev_day: Option<String> = None;

    for e in entries {
        let day = fmt_day(&e.start_at, tz);
        if let Some(ref d) = day
            && prev_day.as_deref() != Some(d.as_str())
        {
            items.push(ListItem::new(Line::from(Span::styled(
                format!("── {d} ──"),
                Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::BOLD),
            ))));
            prev_day = Some(d.clone());
        }

        let task = all_tasks.iter().find(|t| t.id == e.task_id);
        let color = task
            .map(|t| style::status_color(t.status.as_str()))
            .unwrap_or(Color::White);
        let title = task.map(|t| t.title.as_str()).unwrap_or("?");
        let start = fmt_dt(&e.start_at, tz);
        let end = fmt_dt(&e.end_at, tz);
        items.push(ListItem::new(Line::from(vec![
            Span::styled(format!(" {start}"), Style::default().fg(Color::DarkGray)),
            Span::styled(" ─ ", Style::default().fg(Color::DarkGray)),
            Span::styled(end, Style::default().fg(Color::DarkGray)),
            Span::styled(" │ ", Style::default().fg(color)),
            Span::styled(title.to_string(), Style::default().fg(color)),
        ])));
        entry_to_item.push(items.len() - 1);
    }

    (items, entry_to_item)
}

fn draw_tasks_tab(frame: &mut Frame, app: &mut App, area: Rect) {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(area);

    let list_height = chunks[0].height.saturating_sub(2) as usize;
    app.task_list.ensure_visible(list_height);

    let filter_label = app.task_filter.as_deref().unwrap_or("all");
    let items: Vec<ListItem> = app
        .tasks
        .iter()
        .map(|t| {
            let color = style::status_color(t.status.as_str());
            let marker = match t.status {
                TaskStatus::Pending => "[ ]",
                TaskStatus::Scheduled => "[~]",
                TaskStatus::InProgress => "[>]",
                TaskStatus::Completed => "[x]",
                TaskStatus::Skipped => "[-]",
            };
            ListItem::new(Line::from(vec![
                Span::styled(format!("{marker} "), Style::default().fg(color)),
                Span::styled(
                    format!("#{:<4} ", t.display_id),
                    Style::default().fg(Color::DarkGray),
                ),
                Span::raw(t.title.as_str()),
            ]))
        })
        .collect();

    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(format!(" Tasks [{filter_label}] ")),
        )
        .highlight_style(style::selected_style());

    let mut state = ratatui::widgets::ListState::default()
        .with_selected(app.task_list.selected())
        .with_offset(app.task_list.scroll);
    frame.render_stateful_widget(list, chunks[0], &mut state);

    if let Some(task) = app.selected_task() {
        render_task_detail(frame, chunks[1], task, &app.tz);
    } else {
        let p = Paragraph::new("(no task selected)")
            .block(Block::default().borders(Borders::ALL).title(" Detail "));
        frame.render_widget(p, chunks[1]);
    }
}

fn draw_habits_tab(frame: &mut Frame, app: &mut App, area: Rect) {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(area);

    let list_height = chunks[0].height.saturating_sub(2) as usize;
    app.habit_list.ensure_visible(list_height);

    let items: Vec<ListItem> = app
        .habits
        .iter()
        .map(|h| {
            let active_style = if h.active {
                Style::default().fg(Color::Green)
            } else {
                Style::default().fg(Color::DarkGray)
            };
            ListItem::new(Line::from(vec![
                Span::styled(
                    format!("h{:<3} ", h.display_id),
                    Style::default().fg(Color::DarkGray),
                ),
                Span::raw(h.title.as_str()),
                Span::raw(" "),
                Span::styled(if h.active { "●" } else { "○" }, active_style),
            ]))
        })
        .collect();

    let list = List::new(items)
        .block(Block::default().borders(Borders::ALL).title(" Habits "))
        .highlight_style(style::selected_style());

    let mut state = ratatui::widgets::ListState::default()
        .with_selected(app.habit_list.selected())
        .with_offset(app.habit_list.scroll);
    frame.render_stateful_widget(list, chunks[0], &mut state);

    if let Some(habit) = app.selected_habit() {
        let mut lines: Vec<Line> = Vec::new();
        lines.push(Line::from(vec![
            Span::styled("Title: ", Style::default().fg(style::HEADER_FG)),
            Span::raw(&habit.title),
        ]));
        lines.push(Line::from(vec![
            Span::styled("Active: ", Style::default().fg(style::HEADER_FG)),
            Span::styled(
                if habit.active { "yes" } else { "no" },
                Style::default().fg(if habit.active {
                    Color::Green
                } else {
                    Color::DarkGray
                }),
            ),
        ]));
        lines.push(Line::from(vec![
            Span::styled("Window: ", Style::default().fg(style::HEADER_FG)),
            Span::raw(format!("{} - {}", habit.start_time, habit.end_time)),
        ]));
        lines.push(Line::from(vec![
            Span::styled("Mode: ", Style::default().fg(style::HEADER_FG)),
            Span::raw(match habit.window_mode {
                WindowMode::Day => "daily (within day)",
                WindowMode::Period => "period (within window)",
            }),
        ]));
        lines.push(Line::from(vec![
            Span::styled("Estimate: ", Style::default().fg(style::HEADER_FG)),
            Span::raw(format!(
                "{}m (σ={})",
                habit.avg_minutes, habit.sigma_minutes
            )),
        ]));
        lines.push(Line::from(vec![
            Span::styled("Abandon: ", Style::default().fg(style::HEADER_FG)),
            Span::raw(format!("{:.1}", habit.abandonability)),
        ]));
        lines.push(Line::from(vec![
            Span::styled("Parallel: ", Style::default().fg(style::HEADER_FG)),
            Span::raw(if habit.parallelizable { "✓" } else { "✗" }),
            Span::raw("  Allows: "),
            Span::raw(if habit.allows_parallel { "✓" } else { "✗" }),
        ]));
        if habit.fixed {
            lines.push(Line::from(Span::styled(
                "Fixed",
                Style::default().fg(Color::Red),
            )));
        }

        // Recurrence rule summary
        if let Ok(rule) = serde_json::from_str::<takusu_habit::RecurrenceRule>(&habit.recurrence) {
            let summary = takusu_habit::summarize(&rule);
            lines.push(Line::from(""));
            lines.push(Line::from(vec![
                Span::styled("Recurrence: ", Style::default().fg(style::HEADER_FG)),
                Span::raw(summary),
            ]));
        }

        if let Some(ref desc) = habit.description {
            lines.push(Line::from(""));
            lines.push(Line::from(vec![
                Span::styled("Desc: ", Style::default().fg(style::HEADER_FG)),
                Span::raw(desc.as_str()),
            ]));
        }

        render_text_detail(frame, chunks[1], "Habit", lines);
    } else {
        let p = Paragraph::new("(no habit selected)")
            .block(Block::default().borders(Borders::ALL).title(" Detail "));
        frame.render_widget(p, chunks[1]);
    }
}

fn draw_settings_tab(frame: &mut Frame, app: &App, area: Rect) {
    let mut lines: Vec<Line> = Vec::new();
    if let Some(ref s) = app.settings {
        lines.push(Line::from(vec![
            Span::styled("Timezone: ", Style::default().fg(style::HEADER_FG)),
            Span::raw(&s.tz),
        ]));
        lines.push(Line::from(vec![
            Span::styled("Sleep: ", Style::default().fg(style::HEADER_FG)),
            Span::raw(format!("{} - {}", s.sleep_start, s.sleep_end)),
        ]));
        lines.push(Line::from(vec![
            Span::styled("Solver: ", Style::default().fg(style::HEADER_FG)),
            Span::raw(&s.solver),
        ]));
        if let Some(cm) = s.comfortable_minutes {
            lines.push(Line::from(vec![
                Span::styled("Comfortable: ", Style::default().fg(style::HEADER_FG)),
                Span::raw(format!("{cm}m")),
            ]));
        }
        if let Some(mm) = s.maximum_minutes {
            lines.push(Line::from(vec![
                Span::styled("Maximum: ", Style::default().fg(style::HEADER_FG)),
                Span::raw(format!("{mm}m")),
            ]));
        }
        lines.push(Line::from(vec![
            Span::styled("Warm start: ", Style::default().fg(style::HEADER_FG)),
            Span::raw(if s.warm_start { "yes" } else { "no" }),
        ]));
    } else {
        lines.push(Line::from("(no settings)"));
    }
    render_text_detail(frame, area, "Settings", lines);
}

fn draw_status_bar(frame: &mut Frame, app: &App, area: Rect) {
    let msg = app.status_msg.as_deref().unwrap_or("");
    let hint = match app.tab {
        Tab::Schedule => "j/k:move g:generate r:reschedule h/l:tab ?:help q:quit",
        Tab::Tasks => "j/k:move n:new s:status w:work e:edit d:del f:filter h/l:tab ?:help",
        Tab::Habits => "j/k:move d:del h/l:tab ?:help q:quit",
        Tab::Settings => "h/l:tab ?:help q:quit",
    };
    let bar = Line::from(vec![
        Span::styled(format!(" {msg} "), Style::default().fg(Color::Green)),
        Span::styled(hint, Style::default().fg(Color::DarkGray)),
    ]);
    frame.render_widget(Paragraph::new(bar), area);
}

fn draw_help(frame: &mut Frame, area: Rect) {
    let popup = centered_rect(60, 70, area);
    frame.render_widget(Clear, popup);

    let text = vec![
        Line::from(Span::styled(
            "Keybindings",
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from("  q / Ctrl-C    Quit"),
        Line::from("  h / l         Switch tab (prev/next)"),
        Line::from("  1-4 / Tab     Switch tab (direct)"),
        Line::from("  j / k         Move up/down"),
        Line::from("  ?             Toggle help"),
        Line::from(""),
        Line::from(Span::styled(
            "Tasks:",
            Style::default().fg(style::HEADER_FG),
        )),
        Line::from("  n             New task"),
        Line::from("  s             Cycle status"),
        Line::from("  w             Start/pause work session"),
        Line::from("  e             Edit in $EDITOR"),
        Line::from("  d             Delete (confirm)"),
        Line::from("  f             Cycle filter"),
        Line::from("  g             Generate schedule"),
        Line::from("  r             Reschedule"),
        Line::from(""),
        Line::from(Span::styled(
            "Editor:",
            Style::default().fg(style::HEADER_FG),
        )),
        Line::from("  empty fields are not updated"),
        Line::from("  '-' clears description, start_at, quantity_unit, quantity_total, depends"),
        Line::from(""),
        Line::from(Span::styled(
            "Schedule:",
            Style::default().fg(style::HEADER_FG),
        )),
        Line::from("  g             Generate"),
        Line::from("  r             Reschedule"),
        Line::from(""),
        Line::from(Span::styled(
            "Modal:",
            Style::default().fg(style::HEADER_FG),
        )),
        Line::from("  Esc           Cancel"),
        Line::from("  Enter         Confirm / next field"),
        Line::from("  y / n         Confirm delete"),
    ];

    let p = Paragraph::new(text)
        .block(Block::default().borders(Borders::ALL).title(" Help "))
        .wrap(Wrap { trim: false });
    frame.render_widget(p, popup);
}

fn draw_confirm(frame: &mut Frame, area: Rect) {
    let popup = centered_rect(40, 15, area);
    frame.render_widget(Clear, popup);

    let text = vec![
        Line::from(""),
        Line::from("  Delete this item?"),
        Line::from(""),
        Line::from("  [y]es  [n]o"),
    ];
    let p = Paragraph::new(text).block(Block::default().borders(Borders::ALL).title(" Confirm "));
    frame.render_widget(p, popup);
}

fn draw_create_task(frame: &mut Frame, area: Rect, app: &App, field: usize) {
    let popup = centered_rect(50, 30, area);
    frame.render_widget(Clear, popup);

    let labels = ["Title", "Deadline (ISO)", "Avg minutes"];
    let mut lines: Vec<Line> = vec![Line::from("")];
    for (i, label) in labels.iter().enumerate() {
        let marker = if i == field { ">" } else { " " };
        let value = &app.create_fields[i];
        lines.push(Line::from(vec![
            Span::styled(
                format!(" {marker} {label}: "),
                Style::default().fg(style::HEADER_FG),
            ),
            Span::styled(
                value.as_str(),
                if i == field {
                    Style::default().add_modifier(Modifier::UNDERLINED)
                } else {
                    Style::default()
                },
            ),
            Span::styled(
                if i == field { "█" } else { "" },
                Style::default().fg(Color::White),
            ),
        ]));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "  Enter: next/submit  Esc: cancel",
        Style::default().fg(Color::DarkGray),
    )));

    let p = Paragraph::new(lines).block(Block::default().borders(Borders::ALL).title(" New Task "));
    frame.render_widget(p, popup);
}

fn centered_rect(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(area);
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_layout[1])[1]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task(id: &str) -> TaskRow {
        TaskRow {
            id: id.to_string(),
            display_id: 1,
            title: format!("Task {id}"),
            description: None,
            start_at: None,
            end_at: "2025-06-15T10:00:00Z".to_string(),
            avg_minutes: 30,
            sigma_minutes: 5,
            depends: "[]".to_string(),
            parallelizable: false,
            allows_parallel: false,
            abandonability: 0.5,
            status: TaskStatus::Pending,
            habit_id: None,
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
            created_at: "2025-06-14T00:00:00Z".to_string(),
            updated_at: "2025-06-14T00:00:00Z".to_string(),
        }
    }

    fn entry(task_id: &str, start: &str, end: &str) -> ScheduleEntry {
        ScheduleEntry {
            task_id: task_id.to_string(),
            start_at: start.to_string(),
            end_at: end.to_string(),
        }
    }

    #[test]
    fn build_schedule_items_counts_separators() {
        let tz = jiff::tz::TimeZone::UTC;
        let entries = vec![
            entry("a", "2025-06-15T08:00:00Z", "2025-06-15T09:00:00Z"),
            entry("b", "2025-06-15T10:00:00Z", "2025-06-15T11:00:00Z"),
            entry("c", "2025-06-16T08:00:00Z", "2025-06-16T09:00:00Z"),
        ];
        let all_tasks = vec![task("a"), task("b"), task("c")];
        let (items, entry_to_item) = build_schedule_items(&entries, &tz, &all_tasks);

        // Two day separators + three entries = five items.
        assert_eq!(items.len(), 5);
        assert_eq!(entry_to_item, vec![1, 2, 4]);
    }

    #[test]
    fn build_schedule_items_unknown_task_uses_placeholder() {
        let tz = jiff::tz::TimeZone::UTC;
        let entries = vec![entry(
            "missing",
            "2025-06-15T08:00:00Z",
            "2025-06-15T09:00:00Z",
        )];
        let (items, mapping) = build_schedule_items(&entries, &tz, &[]);
        assert_eq!(items.len(), 2); // separator + entry
        assert_eq!(mapping, vec![1]);
    }
}
