use criterion::{Criterion, criterion_group, criterion_main};
use rand::rngs::StdRng;
use rand::{Rng, RngExt, SeedableRng};
use std::hint::black_box;
use takusu_core::{
    NormalDist, ParallelMode, Planner, PlannerConfig, Point, SleepConfig, Solver, Task,
};

fn generate_tasks(rng: &mut impl Rng, count: usize) -> Planner {
    generate_tasks_with(
        rng,
        count,
        PlannerConfig::new(Point(0), SleepConfig::disabled()),
        0.2,
        0.2,
        false,
        2,
    )
}

fn generate_tasks_with(
    rng: &mut impl Rng,
    count: usize,
    config: PlannerConfig,
    parallelizable_prob: f64,
    allows_parallel_prob: f64,
    fixed: bool,
    max_deps: usize,
) -> Planner {
    let mut planner = Planner::new(config);

    for i in 0..count {
        let start_slot = rng.random_range(0..=500);
        let deadline_slot = start_slot + rng.random_range(20..400);
        let avg = rng.random_range(1..=20);
        let sigma = rng.random_range(0..=10);

        let mut depends = Vec::new();
        if i >= 2 {
            let dep_count = rng.random_range(0..=max_deps).min(i);
            let mut possible: Vec<usize> = (0..i).collect();
            for _ in 0..dep_count {
                if possible.is_empty() {
                    break;
                }
                let idx = rng.random_range(0..possible.len());
                depends.push(possible.remove(idx));
            }
        }

        planner
            .add(Task {
                id: 0,
                start: Some(Point(start_slot as i64)),
                end: Point(deadline_slot as i64),
                cost_estimate: NormalDist::new(avg as u64, sigma as u64),
                depends,
                parallel_mode: ParallelMode::from_bools(
                    rng.random_bool(parallelizable_prob),
                    rng.random_bool(allows_parallel_prob),
                ),
                abandonability: rng.random::<f64>().into(),
                fixed,
                habit_group: None,
            })
            .unwrap();
    }

    planner
}

fn bench_plan_25(c: &mut Criterion) {
    let mut rng = StdRng::seed_from_u64(42);
    let planner = generate_tasks(&mut rng, 25);

    let mut group = c.benchmark_group("plan");
    group.sample_size(10);

    group.bench_function("plan 25 tasks", |b| {
        b.iter(|| {
            let plan = planner.plan();
            black_box(plan);
        })
    });

    group.finish();
}

fn bench_plan_partial_25(c: &mut Criterion) {
    let mut rng = StdRng::seed_from_u64(42);
    let planner = generate_tasks(&mut rng, 25);

    let plan = planner.plan();
    let pinned: Vec<_> = plan.schedules.iter().take(5).cloned().collect();

    let mut group = c.benchmark_group("plan_partial");
    group.sample_size(10);

    group.bench_function("plan_partial 25 tasks (5 pinned)", |b| {
        b.iter(|| {
            let plan = planner.plan_partial(&pinned);
            black_box(plan);
        })
    });

    group.finish();
}

fn bench_plan_100(c: &mut Criterion) {
    let mut rng = StdRng::seed_from_u64(44);
    let planner = generate_tasks(&mut rng, 100);

    let mut group = c.benchmark_group("plan");
    group.sample_size(10);

    group.bench_function("plan 100 tasks", |b| {
        b.iter(|| {
            let plan = planner.plan();
            black_box(plan);
        })
    });

    group.finish();
}

fn bench_evaluate(c: &mut Criterion) {
    let mut rng = StdRng::seed_from_u64(42);
    let planner = generate_tasks(&mut rng, 25);
    let plan = planner.plan();

    let mut group = c.benchmark_group("evaluate");
    group.sample_size(100);

    group.bench_function("evaluate 25-task plan", |b| {
        b.iter(|| {
            let score = takusu_core::evaluate::evaluate(&planner, &plan, 0.0, 1.0);
            black_box(score);
        })
    });

    group.finish();
}

fn bench_plan_range_25(c: &mut Criterion) {
    let mut rng = StdRng::seed_from_u64(42);
    let planner = generate_tasks(&mut rng, 25);
    let plan = planner.plan();
    let range = takusu_core::RescheduleRange {
        from: Point(200),
        until: Point(700),
    };

    let mut group = c.benchmark_group("plan_range");
    group.sample_size(10);

    group.bench_function("plan_in_range 25 tasks", |b| {
        b.iter(|| {
            let plan = planner.plan_in_range(&range, &plan.schedules, &[]);
            black_box(plan);
        })
    });

    group.finish();
}

fn bench_plan_many_parallel(c: &mut Criterion) {
    let mut rng = StdRng::seed_from_u64(46);
    let planner = generate_tasks_with(
        &mut rng,
        100,
        PlannerConfig::new(Point(0), SleepConfig::disabled()),
        0.9,
        0.1,
        false,
        2,
    );

    let mut group = c.benchmark_group("plan");
    group.sample_size(10);

    group.bench_function("plan 100 mostly-parallel tasks", |b| {
        b.iter(|| {
            let plan = planner.plan();
            black_box(plan);
        })
    });

    group.finish();
}

fn bench_plan_many_fixed(c: &mut Criterion) {
    let mut rng = StdRng::seed_from_u64(47);
    let planner = generate_tasks_with(
        &mut rng,
        100,
        PlannerConfig::new(Point(0), SleepConfig::disabled()),
        0.2,
        0.2,
        true,
        2,
    );

    let mut group = c.benchmark_group("plan");
    group.sample_size(10);

    group.bench_function("plan 100 fixed tasks", |b| {
        b.iter(|| {
            let plan = planner.plan();
            black_box(plan);
        })
    });

    group.finish();
}

fn bench_plan_many_dependencies(c: &mut Criterion) {
    let mut rng = StdRng::seed_from_u64(48);
    let planner = generate_tasks_with(
        &mut rng,
        100,
        PlannerConfig::new(Point(0), SleepConfig::disabled()),
        0.2,
        0.2,
        false,
        8,
    );

    let mut group = c.benchmark_group("plan");
    group.sample_size(10);

    group.bench_function("plan 100 tasks with many dependencies", |b| {
        b.iter(|| {
            let plan = planner.plan();
            black_box(plan);
        })
    });

    group.finish();
}

fn bench_alns_25(c: &mut Criterion) {
    let mut rng = StdRng::seed_from_u64(42);
    let planner = generate_tasks_with(
        &mut rng,
        25,
        PlannerConfig {
            solver: Solver::Priority,
            seed: Some(42),
            ..PlannerConfig::new(Point(0), SleepConfig::disabled())
        },
        0.2,
        0.2,
        false,
        2,
    );

    let mut group = c.benchmark_group("alns");
    group.sample_size(10);

    group.bench_function("alns 25 tasks", |b| {
        b.iter(|| {
            let plan = planner.plan();
            black_box(plan);
        })
    });

    group.finish();
}

fn bench_alns_100(c: &mut Criterion) {
    let mut rng = StdRng::seed_from_u64(44);
    let planner = generate_tasks_with(
        &mut rng,
        100,
        PlannerConfig {
            solver: Solver::Priority,
            seed: Some(44),
            ..PlannerConfig::new(Point(0), SleepConfig::disabled())
        },
        0.2,
        0.2,
        false,
        2,
    );

    let mut group = c.benchmark_group("alns");
    group.sample_size(10);

    group.bench_function("alns 100 tasks", |b| {
        b.iter(|| {
            let plan = planner.plan();
            black_box(plan);
        })
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_plan_25,
    bench_plan_partial_25,
    bench_plan_100,
    bench_plan_many_parallel,
    bench_plan_many_fixed,
    bench_plan_many_dependencies,
    bench_evaluate,
    bench_plan_range_25,
    bench_alns_25,
    bench_alns_100,
);
criterion_main!(benches);
