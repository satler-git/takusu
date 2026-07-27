use criterion::{Criterion, criterion_group, criterion_main};
use jiff::civil::Date;
use jiff::tz::TimeZone;
use std::hint::black_box;
use takusu_core::{Minutes, NormalDist, Point};
use takusu_habit::{Habit, HabitStore, RecurrenceRule, TimeOfDay};

#[derive(serde::Deserialize)]
struct Fixture {
    timezone: String,
    start_date: String,
    start_time: String,
    horizon_days: i64,
    habits: Vec<FixtureHabit>,
}

#[derive(serde::Deserialize)]
struct FixtureHabit {
    recurrence: RecurrenceRule,
    start_time: String,
    duration_avg_minutes: i64,
    duration_sigma_minutes: i64,
    parallelizable: bool,
    allows_parallel: bool,
    abandonability: f64,
    fixed: bool,
}

const FIXTURE: &str = include_str!("fixtures/realworld_habits.json");

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

fn bench_habit_generate(c: &mut Criterion) {
    let fixture: Fixture =
        serde_json::from_str(FIXTURE).expect("failed to parse realworld_habits.json");
    let tz = TimeZone::get(&fixture.timezone).expect("failed to load fixture timezone");

    let (start_h, start_m) = parse_hhmm(&fixture.start_time);
    let start_date = Date::strptime("%Y-%m-%d", &fixture.start_date)
        .expect("failed to parse fixture start_date");
    let start_dt = start_date.at(start_h as i8, start_m as i8, 0, 0);
    let now = Point::from_timestamp(
        tz.to_timestamp(start_dt)
            .expect("failed to convert fixture start datetime to timestamp"),
        5,
    );

    let mut store = HabitStore::new();
    for habit in fixture.habits {
        let (h, m) = parse_hhmm(&habit.start_time);
        let start_time = TimeOfDay::new(h, m).expect("failed to build fixture TimeOfDay");
        let duration = NormalDist::from_minutes(
            Minutes(habit.duration_avg_minutes),
            Minutes(habit.duration_sigma_minutes),
        );

        store.add(Habit {
            recurrence: habit.recurrence,
            start_time,
            tz: tz.clone(),
            duration,
            deadline_slots: None,
            parallelizable: habit.parallelizable,
            allows_parallel: habit.allows_parallel,
            abandonability: habit.abandonability,
            fixed: habit.fixed,
        });
    }

    let mut group = c.benchmark_group("habit_generate");
    group.sample_size(10);

    let base = fixture.horizon_days;
    for days in [base, base * 2, base * 4] {
        let label = format!("generate realworld habits ({}d)", days);
        let until = now + days * 288;
        group.bench_function(&label, |b| {
            b.iter(|| {
                let tasks = store.generate(now, until);
                black_box(tasks);
            })
        });
    }

    group.finish();
}

criterion_group!(benches, bench_habit_generate);
criterion_main!(benches);
