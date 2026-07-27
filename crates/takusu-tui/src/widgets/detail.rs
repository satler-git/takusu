use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use takusu_storage::TaskRow;
use takusu_util::EnumLabel;

use crate::style;

pub fn render_task_detail(frame: &mut Frame, area: Rect, task: &TaskRow, tz: &jiff::tz::TimeZone) {
    let mut lines: Vec<Line> = Vec::new();

    lines.push(Line::from(vec![
        Span::styled("Title: ", Style::default().fg(style::HEADER_FG)),
        Span::raw(&task.title),
    ]));
    lines.push(Line::from(vec![
        Span::styled("Status: ", Style::default().fg(style::HEADER_FG)),
        Span::styled(
            task.status.as_str(),
            Style::default().fg(style::status_color(task.status.as_str())),
        ),
    ]));
    lines.push(Line::from(vec![
        Span::styled("ID: ", Style::default().fg(style::HEADER_FG)),
        Span::raw(format!("#{}", task.display_id)),
    ]));
    if let Some(ref desc) = task.description {
        lines.push(Line::from(vec![
            Span::styled("Desc: ", Style::default().fg(style::HEADER_FG)),
            Span::raw(desc.as_str()),
        ]));
    }
    if let Some(ref start) = task.start_at {
        lines.push(Line::from(vec![
            Span::styled("Start: ", Style::default().fg(style::HEADER_FG)),
            Span::raw(fmt_dt(start, tz)),
        ]));
    }
    lines.push(Line::from(vec![
        Span::styled("Deadline: ", Style::default().fg(style::HEADER_FG)),
        Span::raw(fmt_dt(&task.end_at, tz)),
    ]));
    lines.push(Line::from(vec![
        Span::styled("Estimate: ", Style::default().fg(style::HEADER_FG)),
        Span::raw(format!("{}m (σ={})", task.avg_minutes, task.sigma_minutes)),
    ]));
    lines.push(Line::from(vec![
        Span::styled("Abandon: ", Style::default().fg(style::HEADER_FG)),
        Span::raw(format!("{:.1}", task.abandonability)),
    ]));
    lines.push(Line::from(vec![
        Span::styled("Parallel: ", Style::default().fg(style::HEADER_FG)),
        Span::raw(if task.parallelizable { "✓" } else { "✗" }),
        Span::raw("  Allows: "),
        Span::raw(if task.allows_parallel { "✓" } else { "✗" }),
    ]));
    if task.fixed {
        lines.push(Line::from(Span::styled(
            "Fixed",
            Style::default().fg(Color::Red),
        )));
    }
    if let Some(total) = task.quantity_total {
        let pct = if total > 0 {
            ((task.quantity_done as f64 / total as f64) * 100.0).clamp(0.0, 100.0) as u16
        } else {
            0
        };
        let bar_width = 20usize;
        let filled = (bar_width as f64 * pct as f64 / 100.0) as usize;
        let bar = format!(
            "[{}{}] {}%",
            "█".repeat(filled),
            "░".repeat(bar_width.saturating_sub(filled)),
            pct
        );
        lines.push(Line::from(vec![
            Span::styled("Progress: ", Style::default().fg(style::HEADER_FG)),
            Span::styled(
                bar,
                Style::default().fg(if pct >= 100 {
                    Color::Green
                } else {
                    Color::Yellow
                }),
            ),
        ]));
        lines.push(Line::from(vec![
            Span::styled("Quantity: ", Style::default().fg(style::HEADER_FG)),
            Span::raw(format!(
                "{}/{} {}",
                task.quantity_done,
                total,
                task.quantity_unit.as_deref().unwrap_or("")
            )),
        ]));
    }
    let deps: Vec<String> = serde_json::from_str(&task.depends).unwrap_or_default();
    if !deps.is_empty() {
        lines.push(Line::from(vec![
            Span::styled("Depends: ", Style::default().fg(style::HEADER_FG)),
            Span::raw(format!("{} task(s)", deps.len())),
        ]));
    }

    let para = Paragraph::new(lines)
        .block(Block::default().borders(Borders::ALL).title(" Detail "))
        .wrap(Wrap { trim: false });
    frame.render_widget(para, area);
}

pub fn render_text_detail(frame: &mut Frame, area: Rect, title: &str, lines: Vec<Line<'_>>) {
    let para = Paragraph::new(lines)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(format!(" {title} ")),
        )
        .wrap(Wrap { trim: false });
    frame.render_widget(para, area);
}

pub fn fmt_dt(iso: &str, tz: &jiff::tz::TimeZone) -> String {
    iso.parse::<jiff::Timestamp>()
        .ok()
        .and_then(|ts| {
            ts.to_zoned(tz.clone())
                .strftime("%m/%d %H:%M")
                .to_string()
                .into()
        })
        .unwrap_or_else(|| iso.to_string())
}

pub fn fmt_day(iso: &str, tz: &jiff::tz::TimeZone) -> Option<String> {
    iso.parse::<jiff::Timestamp>().ok().map(|ts| {
        ts.to_zoned(tz.clone())
            .strftime("%Y-%m-%d (%a)")
            .to_string()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fmt_dt_parses_iso() {
        let tz = jiff::tz::TimeZone::UTC;
        let s = fmt_dt("2025-06-15T08:30:00Z", &tz);
        assert_eq!(s, "06/15 08:30");
    }

    #[test]
    fn fmt_dt_falls_back_on_invalid() {
        let tz = jiff::tz::TimeZone::UTC;
        let s = fmt_dt("not-a-date", &tz);
        assert_eq!(s, "not-a-date");
    }

    #[test]
    fn fmt_day_parses_iso() {
        let tz = jiff::tz::TimeZone::UTC;
        let s = fmt_day("2025-06-15T08:30:00Z", &tz).unwrap();
        assert!(s.starts_with("2025-06-15"));
        assert!(s.contains("Sun"));
    }

    #[test]
    fn fmt_day_returns_none_for_invalid() {
        let tz = jiff::tz::TimeZone::UTC;
        assert!(fmt_day("not-a-date", &tz).is_none());
    }
}
