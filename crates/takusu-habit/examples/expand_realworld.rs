//! Expand the real-world habit fixture into a task fixture for takusu-core benches.
//!
//! Run with:
//!     cargo run -p takusu-habit --example expand_realworld
//!
//! It reads `benches/fixtures/realworld_habits.json` and writes
//! `../takusu-core/benches/fixtures/realworld_tasks.json`.

use jiff::civil::Date;
use jiff::tz::TimeZone;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use takusu_core::{Minutes, NormalDist, Point, SleepConfig, Task};
use takusu_habit::{
    Habit, HabitStore, RecurrenceRule, TimeOfDay, date_time_to_point, point_to_date,
};

#[derive(Deserialize)]
struct Fixture {
    timezone: String,
    sleep_start: String,
    sleep_end: String,
    start_date: String,
    start_time: String,
    horizon_days: i64,
    habits: Vec<FixtureHabit>,
}

#[derive(Deserialize)]
struct FixtureHabit {
    id: String,
    recurrence: RecurrenceRule,
    start_time: String,
    duration_avg_minutes: i64,
    duration_sigma_minutes: i64,
    parallelizable: bool,
    allows_parallel: bool,
    abandonability: f64,
    fixed: bool,
    #[serde(default)]
    steps: Vec<FixtureStep>,
}

#[derive(Deserialize)]
struct FixtureStep {
    start_time: String,
    #[serde(default)]
    end_time: Option<String>,
    avg_minutes: i64,
    sigma_minutes: i64,
    parallelizable: bool,
    allows_parallel: bool,
    abandonability: f64,
    fixed: bool,
    #[serde(default)]
    depends: Vec<usize>,
}

#[derive(Serialize)]
struct TaskFixture {
    start: Option<i64>,
    end: i64,
    avg: u64,
    sigma: u64,
    depends: Vec<usize>,
    parallelizable: bool,
    allows_parallel: bool,
    abandonability: f64,
    fixed: bool,
    habit_group: Option<usize>,
}

#[derive(Serialize)]
struct Output {
    now: i64,
    sleep: SleepFixture,
    tasks: Vec<TaskFixture>,
}

#[derive(Serialize)]
struct SleepFixture {
    day_start: i64,
    start: i64,
    end: i64,
    enabled: bool,
}

fn parse_hhmm(s: &str) -> (u8, u8) {
    let mut parts = s.split(':');
    let h: u8 = parts
        .next()
        .expect("time must be HH:MM")
        .parse()
        .expect("hour must be a number");
    let m: u8 = parts
        .next()
        .expect("time must be HH:MM")
        .parse()
        .expect("minute must be a number");
    assert!(
        parts.next().is_none(),
        "time must be HH:MM, got extra parts"
    );
    (h, m)
}

fn main() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut input_path = manifest_dir.join("benches/fixtures/realworld_habits.json");
    let mut output_path = manifest_dir.join("../takusu-core/benches/fixtures/realworld_tasks.json");
    let mut horizon_days: Option<i64> = None;

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--horizon-days" => {
                let value = args.next().expect("--horizon-days requires a value");
                horizon_days = Some(
                    value
                        .parse()
                        .expect("--horizon-days value must be an integer"),
                );
            }
            "--output" => {
                output_path = manifest_dir.join(args.next().expect("--output requires a value"));
            }
            _ if !arg.starts_with('-') => {
                input_path = manifest_dir.join(arg);
            }
            _ => panic!("unknown argument: {arg}"),
        }
    }

    let input_json = std::fs::read_to_string(&input_path)
        .unwrap_or_else(|e| panic!("failed to read input fixture {input_path:?}: {e}"));
    let mut fixture: Fixture =
        serde_json::from_str(&input_json).expect("failed to parse input fixture JSON");
    if let Some(days) = horizon_days {
        fixture.horizon_days = days;
    }

    let tz = TimeZone::get(&fixture.timezone).expect("failed to load fixture timezone");
    let (sleep_sh, sleep_sm) = parse_hhmm(&fixture.sleep_start);
    let (sleep_eh, sleep_em) = parse_hhmm(&fixture.sleep_end);
    let sleep = SleepConfig::from_local(5, &tz, sleep_sh, sleep_sm, sleep_eh, sleep_em);

    let (start_h, start_m) = parse_hhmm(&fixture.start_time);
    let start_date = Date::strptime("%Y-%m-%d", &fixture.start_date)
        .expect("failed to parse fixture start_date");
    let start_dt = start_date.at(start_h as i8, start_m as i8, 0, 0);
    let now = Point::from_timestamp(
        tz.to_timestamp(start_dt)
            .expect("failed to convert fixture start datetime to timestamp"),
        5,
    );
    let until = now + fixture.horizon_days * 288;

    let mut tasks: Vec<Task> = Vec::new();
    let mut group_map: HashMap<String, usize> = HashMap::new();
    let mut next_group = 0usize;

    for habit in &fixture.habits {
        let group_id = *group_map.entry(habit.id.clone()).or_insert_with(|| {
            let g = next_group;
            next_group += 1;
            g
        });

        let (h, m) = parse_hhmm(&habit.start_time);
        let start_time = TimeOfDay::new(h, m).expect("failed to build habit TimeOfDay");
        let duration = NormalDist::from_minutes(
            Minutes(habit.duration_avg_minutes),
            Minutes(habit.duration_sigma_minutes),
        );

        let h_cfg = Habit {
            recurrence: habit.recurrence.clone(),
            start_time,
            tz: tz.clone(),
            duration,
            deadline_slots: None,
            parallelizable: habit.parallelizable,
            allows_parallel: habit.allows_parallel,
            abandonability: habit.abandonability,
            fixed: habit.fixed,
        };

        let mut store = HabitStore::new();
        store.add(h_cfg);
        let occurrences = store.generate(now, until);

        if habit.steps.is_empty() {
            for mut gt in occurrences {
                gt.task.habit_group = Some(group_id);
                tasks.push(gt.task);
            }
        } else {
            for gt in occurrences {
                let occ_start = gt.task.start.expect("generated task has no start");
                let date = point_to_date(occ_start, &tz)
                    .expect("failed to convert occurrence start to date");
                let base = tasks.len();

                for step in &habit.steps {
                    let (sh, sm) = parse_hhmm(&step.start_time);
                    let step_time = TimeOfDay::new(sh, sm).expect("failed to build step TimeOfDay");
                    let start_pt = date_time_to_point(date, &step_time, &tz)
                        .expect("failed to convert step datetime to point");
                    let end_pt = if step.fixed {
                        if let Some(ref end_time) = step.end_time {
                            let (eh, em) = parse_hhmm(end_time);
                            let end_minutes = eh as i64 * 60 + em as i64;
                            let start_minutes = sh as i64 * 60 + sm as i64;
                            assert!(
                                end_minutes >= start_minutes,
                                "step end_time {end_time} must be at or after start_time {}",
                                step.start_time
                            );
                            start_pt + Minutes(end_minutes - start_minutes).to_slots().0
                        } else {
                            start_pt + Minutes(step.avg_minutes).to_slots().0
                        }
                    } else {
                        start_pt + Minutes(step.avg_minutes).to_slots().0
                    };

                    let depends: Vec<usize> = step.depends.iter().map(|&d| base + d).collect();

                    let task = Task {
                        id: 0,
                        start: Some(start_pt),
                        end: end_pt,
                        cost_estimate: NormalDist::from_minutes(
                            Minutes(step.avg_minutes),
                            Minutes(step.sigma_minutes),
                        ),
                        depends,
                        parallelizable: step.parallelizable,
                        allows_parallel: step.allows_parallel,
                        abandonability: step.abandonability,
                        fixed: step.fixed,
                        habit_group: Some(group_id),
                    };
                    tasks.push(task);
                }
            }
        }
    }

    let task_fixtures: Vec<TaskFixture> = tasks
        .into_iter()
        .map(|t| TaskFixture {
            start: t.start.map(|p| p.0),
            end: t.end.0,
            avg: t.cost_estimate.avg,
            sigma: t.cost_estimate.sigma,
            depends: t.depends,
            parallelizable: t.parallelizable,
            allows_parallel: t.allows_parallel,
            abandonability: t.abandonability,
            fixed: t.fixed,
            habit_group: t.habit_group,
        })
        .collect();

    let output = Output {
        now: now.0,
        sleep: SleepFixture {
            day_start: sleep.day_start,
            start: sleep.start,
            end: sleep.end,
            enabled: sleep.enabled,
        },
        tasks: task_fixtures,
    };

    if let Some(parent) = output_path.parent() {
        std::fs::create_dir_all(parent)
            .unwrap_or_else(|e| panic!("failed to create output directory {parent:?}: {e}"));
    }
    let json = serde_json::to_string_pretty(&output).expect("failed to serialize task fixture");
    std::fs::write(&output_path, format!("{json}\n"))
        .unwrap_or_else(|e| panic!("failed to write output fixture {output_path:?}: {e}"));
    println!("wrote {}", output_path.display());
}
