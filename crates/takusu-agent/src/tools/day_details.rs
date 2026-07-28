//! Day detail tool for the agent (issue #1021).
//!
//! `day_details` takes one or more date expressions and returns, for each day:
//! - the ISO date
//! - the weekday (in Japanese)
//! - whether it is a Japanese public holiday, and if so the holiday name
//! - optionally, the schedule entries overlapping that day

use std::collections::HashMap;

use async_trait::async_trait;
use jiff::ToSpan;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};
use takusu_client::{Client, ScheduleEntry, TaskQuery, TaskRow};
use takusu_util::parse_date_expression;

use crate::tools::takusu::TimeZoneCache;
use crate::tools::takusu::client_error;
use crate::tools::{ToolContext, ToolModule};
use crate::{InvalidArgsError, ToolError, ToolName, ToolOutput, ToolRegistry, TypedTool};

struct DayDetailsModule;

impl ToolModule for DayDetailsModule {
    fn register(&self, registry: &mut ToolRegistry, ctx: &ToolContext) {
        registry.register(Box::new(crate::tool::Typed(DayDetails {
            client: ctx.client.clone(),
            tz_cache: ctx.tz_cache.clone(),
        })));
    }
}

static DAY_DETAILS_MODULE: &dyn ToolModule = &DayDetailsModule;

inventory::submit!(DAY_DETAILS_MODULE);

struct DayDetails {
    client: Client,
    tz_cache: TimeZoneCache,
}

/// Arguments for [`DayDetails`].
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct DayDetailsArgs {
    /// Date expressions such as '2026-07-27', 'today', or '3d'.
    #[schemars(length(min = 1))]
    dates: Vec<String>,
    /// If true, include schedule entries for each day. Default false.
    #[serde(default)]
    include_schedule: bool,
}

#[async_trait]
impl TypedTool for DayDetails {
    type Params = DayDetailsArgs;

    fn name(&self) -> &'static str {
        ToolName::DayDetails.into()
    }

    fn description(&self) -> &'static str {
        "Return weekday, Japanese public holiday information, and optionally \
        the schedule for one or more dates. Dates can be absolute (YYYY-MM-DD) \
        or relative (today, tomorrow, 7d, -3d)."
    }

    fn validate_args(&self, args: &Self::Params) -> Result<(), InvalidArgsError> {
        if args.dates.is_empty() {
            return Err(InvalidArgsError::new("dates", "must be a non-empty array"));
        }
        Ok(())
    }

    async fn call_typed(&self, args: Self::Params) -> Result<ToolOutput, ToolError> {
        let tz = self.tz_cache.get_with_fallback().await;

        let mut parsed_dates = Vec::with_capacity(args.dates.len());
        for s in &args.dates {
            let ts = parse_date_expression(s, &tz, false).map_err(|e| {
                ToolError::InvalidArgs(InvalidArgsError::new(
                    "dates",
                    format!("invalid date '{s}': {e}"),
                ))
            })?;
            parsed_dates.push(ts.to_zoned(tz.clone()).date());
        }
        parsed_dates.sort();
        parsed_dates.dedup();

        let mut results = Vec::with_capacity(parsed_dates.len());

        if args.include_schedule {
            let c1 = self.client.clone();
            let c2 = self.client.clone();
            let (schedule_row, tasks) =
                tokio::try_join!(async { c1.get_schedule().await }, async {
                    c2.list_tasks(&TaskQuery::default()).await
                },)
                .map_err(client_error)?;

            let entries: Vec<ScheduleEntry> = schedule_row.schedule.as_inner().clone();
            let task_by_id: HashMap<String, &TaskRow> =
                tasks.iter().map(|t| (t.id.clone(), t)).collect();

            for date in parsed_dates {
                let schedule = schedule_for_date(date, &entries, &task_by_id, &tz)?;
                results.push(day_json(date, &schedule, true));
            }
        } else {
            for date in parsed_dates {
                results.push(day_json(date, &[], false));
            }
        }

        Ok(ToolOutput {
            content: serde_json::to_string(&results).unwrap_or_default(),
            ..Default::default()
        })
    }
}

struct ScheduleItem<'a> {
    start: jiff::Timestamp,
    end: jiff::Timestamp,
    entry: &'a ScheduleEntry,
    task: Option<&'a TaskRow>,
}

fn schedule_for_date(
    date: jiff::civil::Date,
    entries: &[ScheduleEntry],
    task_by_id: &HashMap<String, &TaskRow>,
    tz: &jiff::tz::TimeZone,
) -> Result<Vec<Value>, ToolError> {
    let day_start = tz
        .to_timestamp(date.at(0, 0, 0, 0))
        .map_err(|e| ToolError::Other(Box::new(e)))?;
    let next_day = date
        .checked_add(1.day())
        .map_err(|e| ToolError::Other(Box::new(e)))?;
    let day_end_excl = tz
        .to_timestamp(next_day.at(0, 0, 0, 0))
        .map_err(|e| ToolError::Other(Box::new(e)))?;

    let mut items = Vec::new();
    for entry in entries {
        let start = entry.start_at.to_jiff();
        let end = entry.end_at.to_jiff();

        // Overlap: entry starts before the next day and ends after today starts.
        if start >= day_end_excl || end <= day_start {
            continue;
        }

        let task = task_by_id.get(&entry.task_id).copied();
        items.push(ScheduleItem {
            start,
            end,
            entry,
            task,
        });
    }

    items.sort_by_key(|item| item.start);

    Ok(items
        .into_iter()
        .map(|item| {
            json!({
                "task_id": item.entry.task_id,
                "title": item.task.map(|t| t.title.clone()).unwrap_or_default(),
                "start_at": format_ts(item.start, tz),
                "end_at": format_ts(item.end, tz),
                "status": item.task.map(|t| t.status).unwrap_or_default(),
            })
        })
        .collect())
}

fn day_json(date: jiff::civil::Date, schedule: &[Value], include_schedule: bool) -> Value {
    let (is_holiday, holiday_name) = holiday_info(date);
    let mut obj = json!({
        "date": date.to_string(),
        "weekday": weekday_ja(date.weekday()),
        "is_holiday": is_holiday,
    });
    let map = obj.as_object_mut().unwrap();
    if let Some(name) = holiday_name {
        map.insert("holiday_name".into(), json!(name));
    }
    if include_schedule {
        map.insert("schedule".into(), json!(schedule));
    }
    obj
}

fn weekday_ja(wd: jiff::civil::Weekday) -> &'static str {
    match wd {
        jiff::civil::Weekday::Monday => "月",
        jiff::civil::Weekday::Tuesday => "火",
        jiff::civil::Weekday::Wednesday => "水",
        jiff::civil::Weekday::Thursday => "木",
        jiff::civil::Weekday::Friday => "金",
        jiff::civil::Weekday::Saturday => "土",
        jiff::civil::Weekday::Sunday => "日",
    }
}

fn holiday_info(date: jiff::civil::Date) -> (bool, Option<String>) {
    if let Ok(jp_date) = jpholiday::Date::new(
        i32::from(date.year()),
        date.month() as u32,
        date.day() as u32,
    ) && let Some(name) = jpholiday::is_holiday_name(jp_date)
    {
        return (true, Some(name));
    }
    (false, None)
}

fn format_ts(ts: jiff::Timestamp, tz: &jiff::tz::TimeZone) -> String {
    ts.to_zoned(tz.clone())
        .strftime("%Y-%m-%dT%H:%M:%S%:z")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use jiff::civil::date;

    #[test]
    fn weekday_ja_monday() {
        assert_eq!(weekday_ja(date(2026, 7, 27).weekday()), "月");
    }

    #[test]
    fn holiday_info_new_year() {
        let (is_holiday, name) = holiday_info(date(2026, 1, 1));
        assert!(is_holiday);
        assert_eq!(name.as_deref(), Some("元日"));
    }

    #[test]
    fn holiday_info_normal_day() {
        let (is_holiday, name) = holiday_info(date(2026, 1, 3));
        assert!(!is_holiday);
        assert!(name.is_none());
    }

    #[test]
    fn schedule_for_date_filters_and_overlaps() {
        let tz = jiff::tz::TimeZone::UTC;
        let d = date(2026, 7, 27);
        let entries = vec![
            ScheduleEntry {
                task_id: "t1".into(),
                start_at: "2026-07-27T09:00:00Z".parse().unwrap(),
                end_at: "2026-07-27T10:00:00Z".parse().unwrap(),
            },
            ScheduleEntry {
                task_id: "t2".into(),
                start_at: "2026-07-26T23:00:00Z".parse().unwrap(),
                end_at: "2026-07-27T01:00:00Z".parse().unwrap(),
            },
            ScheduleEntry {
                task_id: "t3".into(),
                start_at: "2026-07-28T09:00:00Z".parse().unwrap(),
                end_at: "2026-07-28T10:00:00Z".parse().unwrap(),
            },
        ];
        let tasks = HashMap::new();
        let schedule = schedule_for_date(d, &entries, &tasks, &tz).unwrap();
        assert_eq!(schedule.len(), 2);
        let ids: Vec<String> = schedule
            .iter()
            .map(|v| v.get("task_id").unwrap().as_str().unwrap().to_string())
            .collect();
        assert_eq!(ids, vec!["t2", "t1"]);
    }

    #[test]
    fn schedule_is_sorted_by_start_time() {
        let tz = jiff::tz::TimeZone::UTC;
        let d = date(2026, 7, 27);
        let entries = vec![
            ScheduleEntry {
                task_id: "later".into(),
                start_at: "2026-07-27T14:00:00Z".parse().unwrap(),
                end_at: "2026-07-27T15:00:00Z".parse().unwrap(),
            },
            ScheduleEntry {
                task_id: "early".into(),
                start_at: "2026-07-27T08:00:00Z".parse().unwrap(),
                end_at: "2026-07-27T09:00:00Z".parse().unwrap(),
            },
        ];
        let tasks = HashMap::new();
        let schedule = schedule_for_date(d, &entries, &tasks, &tz).unwrap();
        let ids: Vec<String> = schedule
            .iter()
            .map(|v| v.get("task_id").unwrap().as_str().unwrap().to_string())
            .collect();
        assert_eq!(ids, vec!["early", "later"]);
    }
}
