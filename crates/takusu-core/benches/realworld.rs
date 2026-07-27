use criterion::{Criterion, criterion_group, criterion_main};
use std::hint::black_box;
use std::time::Duration;
use takusu_core::{NormalDist, Planner, Point, RescheduleRange, SleepConfig, Task};

#[derive(serde::Deserialize)]
struct Fixture {
    now: i64,
    sleep: SleepFixture,
    tasks: Vec<TaskFixture>,
}

#[derive(serde::Deserialize)]
struct SleepFixture {
    day_start: i64,
    start: i64,
    end: i64,
    enabled: bool,
}

#[derive(serde::Deserialize)]
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

const FIXTURE_7D: &str = include_str!("fixtures/realworld_tasks_7d.json");
const FIXTURE_14D: &str = include_str!("fixtures/realworld_tasks.json");
const FIXTURE_30D: &str = include_str!("fixtures/realworld_tasks_30d.json");

fn build_planner(fixture: &str) -> (Planner, i64) {
    let fixture: Fixture = serde_json::from_str(fixture).unwrap();

    let sleep = SleepConfig {
        day_start: fixture.sleep.day_start,
        start: fixture.sleep.start,
        end: fixture.sleep.end,
        enabled: fixture.sleep.enabled,
    };

    let mut planner = Planner::new(Point(fixture.now), sleep);

    for t in fixture.tasks {
        planner
            .add(Task {
                id: 0,
                start: t.start.map(Point),
                end: Point(t.end),
                cost_estimate: NormalDist::new(t.avg, t.sigma),
                depends: t.depends,
                parallelizable: t.parallelizable,
                allows_parallel: t.allows_parallel,
                abandonability: t.abandonability.into(),
                fixed: t.fixed,
                habit_group: t.habit_group,
            })
            .unwrap();
    }

    (planner, fixture.now)
}

fn bench_realworld_plan(c: &mut Criterion) {
    let mut group = c.benchmark_group("realworld");
    group.sample_size(10);
    group.measurement_time(Duration::from_secs(30));

    // Keep only the scale endpoints (7d and 30d) for full-plan benchmarks.
    // The 14d fixture is still exercised by plan_partial and plan_in_range below.

    group.bench_function("plan realworld habits (7d)", |b| {
        let (planner, _) = build_planner(FIXTURE_7D);
        b.iter(|| {
            let plan = planner.plan();
            black_box(plan);
        })
    });

    group.bench_function("plan realworld habits (30d)", |b| {
        let (planner, _) = build_planner(FIXTURE_30D);
        b.iter(|| {
            let plan = planner.plan();
            black_box(plan);
        })
    });

    group.finish();
}

fn bench_realworld_partial(c: &mut Criterion) {
    let (planner, _) = build_planner(FIXTURE_14D);
    let base_plan = planner.plan();
    let pinned: Vec<_> = base_plan.schedules.iter().take(5).cloned().collect();

    let mut group = c.benchmark_group("realworld_partial");
    group.sample_size(10);
    group.measurement_time(Duration::from_secs(30));

    group.bench_function("plan_partial realworld habits (14d, 5 pinned)", |b| {
        b.iter(|| {
            let plan = planner.plan_partial(&pinned);
            black_box(plan);
        })
    });

    group.finish();
}

fn bench_realworld_range(c: &mut Criterion) {
    let (planner, now) = build_planner(FIXTURE_14D);
    let base_plan = planner.plan();
    let range = RescheduleRange {
        from: Point(now + 2 * 288),
        until: Point(now + 7 * 288),
    };

    let mut group = c.benchmark_group("realworld_range");
    group.sample_size(10);
    group.measurement_time(Duration::from_secs(30));

    group.bench_function("plan_in_range realworld habits (14d, days 2-7)", |b| {
        b.iter(|| {
            let plan = planner.plan_in_range(&range, &base_plan.schedules, &[]);
            black_box(plan);
        })
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_realworld_plan,
    bench_realworld_partial,
    bench_realworld_range,
);
criterion_main!(benches);
