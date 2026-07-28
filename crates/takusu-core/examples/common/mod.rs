use takusu_core::{NormalDist, ParallelMode, Planner, PlannerConfig, Point, SleepConfig, Task};

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

#[allow(dead_code)]
pub fn build_planner() -> Planner {
    build_planner_from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/benches/fixtures/realworld_tasks.json"
    )))
}

#[allow(dead_code)]
pub fn build_planner_7d() -> Planner {
    build_planner_from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/benches/fixtures/realworld_tasks_7d.json"
    )))
}

#[allow(dead_code)]
pub fn build_planner_30d() -> Planner {
    build_planner_from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/benches/fixtures/realworld_tasks_30d.json"
    )))
}

#[allow(dead_code)]
pub fn build_planner_small() -> Planner {
    build_planner_from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/benches/fixtures/quality_small.json"
    )))
}

#[allow(dead_code)]
pub fn build_stress_30d() -> Planner {
    build_planner_from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/benches/fixtures/stress_30d.json"
    )))
}

#[allow(dead_code)]
pub fn build_stress_30d_dense() -> Planner {
    build_planner_from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/benches/fixtures/stress_30d_dense.json"
    )))
}

#[allow(dead_code)]
pub fn build_stress_30d_mixed() -> Planner {
    build_planner_from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/benches/fixtures/stress_30d_mixed.json"
    )))
}

#[allow(dead_code)]
pub fn build_stress_90d() -> Planner {
    build_planner_from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/benches/fixtures/stress_90d.json"
    )))
}

#[allow(dead_code)]
pub fn build_planner_from_str(input: &str) -> Planner {
    let fixture: Fixture = serde_json::from_str(input).unwrap();

    let sleep = SleepConfig {
        day_start: fixture.sleep.day_start,
        start: fixture.sleep.start,
        end: fixture.sleep.end,
        enabled: fixture.sleep.enabled,
    };

    let mut planner = Planner::new(PlannerConfig::new(Point(fixture.now), sleep));
    for t in fixture.tasks {
        planner
            .add(Task {
                id: 0,
                start: t.start.map(Point),
                end: Point(t.end),
                cost_estimate: NormalDist::new(t.avg, t.sigma),
                depends: t.depends,
                parallel_mode: ParallelMode::from_bools(t.parallelizable, t.allows_parallel),
                abandonability: t.abandonability.into(),
                fixed: t.fixed,
                habit_group: t.habit_group,
            })
            .unwrap();
    }
    planner
}
