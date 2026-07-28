use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use rayon::prelude::*;

use takusu_core::ParallelMode;

mod common;

const DEFAULT_SEEDS: u64 = 100;

#[derive(Clone, Copy)]
struct Metrics {
    score: f64,
    missing: usize,
    duplicates: usize,
    overlap_slots: i64,
    dependency_slots: i64,
    start_slots: i64,
    sleep_shortage_days: usize,
    sleep_shortage_slots: i64,
    daily_maximum_excess: i64,
    elapsed_ms: f64,
}

type FixtureBuilder = (&'static str, fn() -> takusu_core::Planner);

fn main() {
    let seeds = std::env::var("TAKUSU_QUALITY_SEEDS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(DEFAULT_SEEDS);
    let only_fixture = std::env::var("TAKUSU_QUALITY_FIXTURE").ok();
    let alns_solver = std::env::var("TAKUSU_QUALITY_SOLVER").as_deref() == Ok("alns");
    let merge_path = std::env::var("TAKUSU_QUALITY_MERGE")
        .or_else(|_| std::env::var("merge"))
        .ok();

    let fixtures: [FixtureBuilder; 8] = [
        ("small", common::build_planner_small),
        ("stress_30d", common::build_stress_30d),
        ("stress_30d_dense", common::build_stress_30d_dense),
        ("stress_30d_mixed", common::build_stress_30d_mixed),
        ("stress_90d", common::build_stress_90d),
        ("7d", common::build_planner_7d),
        ("14d", common::build_planner),
        ("30d", common::build_planner_30d),
    ];

    let mut completed_modes = std::collections::HashSet::<(String, String)>::new();
    let mut merge_has_header = false;
    if let Some(path) = &merge_path
        && let Ok(content) = std::fs::read_to_string(path)
    {
        for line in content.lines() {
            let mut parts = line.split('\t');
            if let (Some(fixture), Some(mode)) = (parts.next(), parts.next()) {
                if fixture == "fixture" && mode == "mode" {
                    merge_has_header = true;
                } else {
                    completed_modes.insert((fixture.to_string(), mode.to_string()));
                }
            }
        }
    }

    let fixture_count = fixtures.len();
    let active_fixtures: Vec<_> = fixtures
        .iter()
        .filter(|(name, _)| {
            only_fixture
                .as_deref()
                .is_none_or(|fixture| fixture == *name)
        })
        .filter(|(name, _)| *name != "stress_90d" || only_fixture.as_deref() == Some("stress_90d"))
        .copied()
        .collect();

    // ALNS solver は Stage 1 prototype で full solve のみ対応。
    // partial / range は roadmap で production 採用条件に含まれるが、
    // 現時点では未実装（doc/plan/priority-decoder-production-roadmap.md 参照）。
    let modes: &[&str] = if alns_solver {
        &["full"]
    } else {
        &["full", "partial", "range"]
    };
    let planned_runs: Vec<_> = active_fixtures
        .iter()
        .flat_map(|(name, _)| modes.iter().map(|mode| (*name, *mode)))
        .filter(|(name, mode)| !completed_modes.contains(&(name.to_string(), mode.to_string())))
        .collect();

    let active_mode_count = active_fixtures.len() * modes.len();
    let skipped_mode_count = active_mode_count - planned_runs.len();
    let total_runs = planned_runs.len() as u64 * seeds;
    eprintln!(
        "[quality] {} active fixture(s), {} planned mode(s), {} seeds/mode, {} total runs ({} mode(s) skipped from merge)",
        active_fixtures.len(),
        planned_runs.len(),
        seeds,
        total_runs,
        skipped_mode_count
    );

    // Print header unless merging with a file that already contains one.
    if merge_path.is_none() || !merge_has_header {
        println!(
            "fixture\tmode\tcount\tscore_median\tscore_p10\tmissing_mean\tduplicates_mean\toverlap_mean\tdependency_mean\tstart_mean\tsleep_days_mean\tsleep_slots_mean\tdaily_max_excess_mean\tlatency_p50_ms\tlatency_p95_ms"
        );
    }

    let overall_completed = AtomicU64::new(0);

    for (fixture_index, (name, builder)) in fixtures.iter().enumerate() {
        let fixture_index = fixture_index + 1;
        if only_fixture
            .as_deref()
            .is_some_and(|fixture| fixture != *name)
        {
            continue;
        }
        if *name == "stress_90d" && only_fixture.as_deref() != Some("stress_90d") {
            eprintln!(
                "[quality] fixture {}/{}: stress_90d skipped by default (use TAKUSU_QUALITY_FIXTURE=stress_90d to run)",
                fixture_index, fixture_count
            );
            continue;
        }

        let modes_to_run: Vec<_> = modes
            .iter()
            .filter(|mode| !completed_modes.contains(&(name.to_string(), mode.to_string())))
            .copied()
            .collect();
        if modes_to_run.is_empty() {
            eprintln!(
                "[quality] fixture {}/{}: {name}: all modes present in merge file, skipping",
                fixture_index, fixture_count
            );
            continue;
        }

        eprintln!(
            "[quality] starting fixture {}/{}: {}",
            fixture_index, fixture_count, name
        );

        let planner = builder();
        eprintln!("[quality] {name}: computing baseline plan (seed 0)...");
        let baseline = if alns_solver {
            planner.plan_alns_with_seed(0)
        } else {
            planner.plan_with_seed(0)
        };
        eprintln!("[quality] {name}: baseline completed");

        let pinned: Vec<_> = baseline.schedules.iter().take(5).copied().collect();
        let range = takusu_core::RescheduleRange {
            from: takusu_core::Point::from_raw(planner.tasks()[0].end.0 / 4),
            until: takusu_core::Point::from_raw(planner.tasks()[0].end.0 * 3 / 4),
        };

        if modes_to_run.contains(&"full") {
            report(
                name,
                "full",
                &planner,
                seeds,
                fixture_index,
                fixture_count,
                &overall_completed,
                total_runs,
                |seed| {
                    if alns_solver {
                        planner.plan_alns_with_seed(seed)
                    } else {
                        planner.plan_with_seed(seed)
                    }
                },
            );
        } else {
            eprintln!("[quality] {name}/full: already present in merge file, skipping");
        }

        if modes_to_run.contains(&"partial") {
            report(
                name,
                "partial",
                &planner,
                seeds,
                fixture_index,
                fixture_count,
                &overall_completed,
                total_runs,
                |seed| planner.plan_partial_with_seed(&pinned, seed),
            );
        } else {
            eprintln!("[quality] {name}/partial: already present in merge file, skipping");
        }

        if modes_to_run.contains(&"range") {
            report(
                name,
                "range",
                &planner,
                seeds,
                fixture_index,
                fixture_count,
                &overall_completed,
                total_runs,
                |seed| planner.plan_in_range_with_seed(&range, &baseline.schedules, &[], seed),
            );
        } else {
            eprintln!("[quality] {name}/range: already present in merge file, skipping");
        }
    }

    eprintln!(
        "[quality] all fixtures completed ({} / {} runs)",
        overall_completed.load(Ordering::Relaxed),
        total_runs
    );
}

#[allow(clippy::too_many_arguments)]
fn report(
    fixture: &str,
    mode: &str,
    planner: &takusu_core::Planner,
    seeds: u64,
    fixture_index: usize,
    fixture_count: usize,
    overall_completed: &AtomicU64,
    overall_total: u64,
    make_plan: impl Fn(u64) -> takusu_core::Plan + Sync,
) {
    let start = Instant::now();
    let progress_interval = if rayon::current_num_threads() <= 1 {
        1
    } else {
        (seeds / 20).max(1)
    };
    let overall_start = overall_completed.load(Ordering::Relaxed);
    eprintln!(
        "[quality] {fixture}/{mode}: 0/{seeds} (fixture {fixture_index}/{fixture_count}, overall {overall_start}/{overall_total})"
    );
    let completed = AtomicU64::new(0);
    let rows: Vec<_> = (0..seeds)
        .into_par_iter()
        .map(|seed| {
            let plan_start = Instant::now();
            let plan = make_plan(seed);
            let elapsed_ms = plan_start.elapsed().as_secs_f64() * 1_000.0;
            let row = Metrics {
                elapsed_ms,
                ..metrics(planner, &plan)
            };
            let done = completed.fetch_add(1, Ordering::Relaxed) + 1;
            let overall = overall_completed.fetch_add(1, Ordering::Relaxed) + 1;
            if done == 1 || done == seeds || done.is_multiple_of(progress_interval) {
                let elapsed = start.elapsed().as_secs_f64();
                let eta = if done > 0 {
                    let rate = elapsed / done as f64;
                    rate * (seeds - done) as f64
                } else {
                    0.0
                };
                eprintln!(
                    "[quality] {fixture}/{mode}: {done}/{seeds} (fixture {fixture_index}/{fixture_count}, overall {overall}/{overall_total}, elapsed {elapsed:.1}s, eta {eta:.1}s)"
                );
            }
            row
        })
        .collect();

    let scores = rows.iter().map(|m| m.score).collect::<Vec<_>>();
    let latencies = rows.iter().map(|m| m.elapsed_ms).collect::<Vec<_>>();
    println!(
        "{fixture}\t{mode}\t{}\t{:.3}\t{:.3}\t{:.3}\t{:.3}\t{:.3}\t{:.3}\t{:.3}\t{:.3}\t{:.3}\t{:.3}\t{:.3}\t{:.3}",
        rows.len(),
        percentile(scores.clone(), 0.50),
        percentile(scores, 0.10),
        mean(rows.iter().map(|m| m.missing as f64)),
        mean(rows.iter().map(|m| m.duplicates as f64)),
        mean(rows.iter().map(|m| m.overlap_slots as f64)),
        mean(rows.iter().map(|m| m.dependency_slots as f64)),
        mean(rows.iter().map(|m| m.start_slots as f64)),
        mean(rows.iter().map(|m| m.sleep_shortage_days as f64)),
        mean(rows.iter().map(|m| m.sleep_shortage_slots as f64)),
        mean(rows.iter().map(|m| m.daily_maximum_excess as f64)),
        percentile(latencies.clone(), 0.50),
        percentile(latencies, 0.95),
    );
}

fn metrics(planner: &takusu_core::Planner, plan: &takusu_core::Plan) -> Metrics {
    let mut by_id = vec![None; planner.tasks().len()];
    let mut duplicates = 0;
    for p in &plan.schedules {
        if let Some(slot) = by_id.get_mut(p.task_id) {
            if slot.is_some() {
                duplicates += 1;
            }
            *slot = Some((p.start, p.end));
        }
    }

    let missing = by_id.iter().filter(|slot| slot.is_none()).count();
    let mut overlap_slots = 0;
    for (i, a_p) in plan.schedules.iter().enumerate() {
        for b_p in plan.schedules.iter().skip(i + 1) {
            if a_p.start.0 >= b_p.end.0 || b_p.start.0 >= a_p.end.0 {
                continue;
            }
            let a = &planner.tasks()[a_p.task_id];
            let b = &planner.tasks()[b_p.task_id];
            if !ParallelMode::can_overlap(a.parallel_mode, b.parallel_mode)
            {
                overlap_slots += a_p.end.0.min(b_p.end.0) - a_p.start.0.max(b_p.start.0);
            }
        }
    }

    let mut dependency_slots = 0;
    let mut start_slots = 0;
    for task in planner.tasks() {
        let Some((start, _)) = by_id.get(task.id).and_then(|slot| *slot) else {
            continue;
        };
        if let Some(min_start) = task.start {
            start_slots += (min_start.0 - start.0).max(0);
        }
        for dependency in &task.depends {
            if let Some((_, dependency_end)) = by_id.get(*dependency).and_then(|slot| *slot) {
                dependency_slots += (dependency_end.0 - start.0).max(0);
            }
        }
    }

    let (sleep_shortage_days, sleep_shortage_slots) = sleep_shortage(planner, plan);
    let daily_maximum_excess = daily_maximum_excess(planner, plan);
    Metrics {
        score: takusu_core::evaluate::evaluate(planner, plan, 0.0, 1.0),
        missing,
        duplicates,
        overlap_slots,
        dependency_slots,
        start_slots,
        sleep_shortage_days,
        sleep_shortage_slots,
        daily_maximum_excess,
        elapsed_ms: 0.0,
    }
}

fn sleep_shortage(planner: &takusu_core::Planner, plan: &takusu_core::Plan) -> (usize, i64) {
    let sleep = planner.sleep_config();
    if !sleep.enabled() || plan.schedules.is_empty() {
        return (0, 0);
    }
    let slots_per_day = 288;
    let plan_start = plan.schedules.iter().map(|p| p.start.0).min().unwrap();
    let plan_end = plan.schedules.iter().map(|p| p.end.0).max().unwrap();
    let first_day = sleep.day_start()
        + (plan_start - sleep.day_start()).div_euclid(slots_per_day) * slots_per_day
        - slots_per_day;
    let sleep_len = sleep.end() - sleep.start();
    let mut shortage_days = 0;
    let mut shortage_slots = 0;
    let mut day_start = first_day;
    while day_start + sleep.start() <= plan_end {
        let window_start = day_start + sleep.start();
        let window_end = day_start + sleep.end();
        let occupied = plan
            .schedules
            .iter()
            .map(|p| (p.start.0.max(window_start), p.end.0.min(window_end)))
            .filter(|(start, end)| start < end)
            .collect::<Vec<_>>();
        let got = (sleep_len - union_length(&occupied)).max(0);
        if !occupied.is_empty() && got < 36 {
            shortage_days += 1;
            shortage_slots += 36 - got;
        }
        day_start += slots_per_day;
    }
    (shortage_days, shortage_slots)
}

fn daily_maximum_excess(planner: &takusu_core::Planner, plan: &takusu_core::Plan) -> i64 {
    let mut loads = std::collections::BTreeMap::<i64, Vec<(i64, i64)>>::new();
    for p in &plan.schedules {
        let mut cursor = p.start.0;
        while cursor < p.end.0 {
            let day = cursor.div_euclid(288);
            let day_end = (day + 1) * 288;
            let segment_end = p.end.0.min(day_end);
            loads.entry(day).or_default().push((cursor, segment_end));
            cursor = segment_end;
        }
    }
    let maximum = planner.workload().maximum_slots_per_day();
    if maximum <= 0 {
        return 0;
    }
    loads
        .values()
        .map(|segments| (union_length(segments) - maximum).max(0))
        .sum()
}

fn union_length(segments: &[(i64, i64)]) -> i64 {
    let mut sorted = segments.to_vec();
    sorted.sort_unstable();
    let Some(&(mut start, mut end)) = sorted.first() else {
        return 0;
    };
    let mut total = 0;
    for &(next_start, next_end) in &sorted[1..] {
        if next_start > end {
            total += end - start;
            start = next_start;
        }
        end = end.max(next_end);
    }
    total + end - start
}

fn mean(values: impl Iterator<Item = f64>) -> f64 {
    let values: Vec<_> = values.collect();
    values.iter().sum::<f64>() / values.len() as f64
}

fn percentile(mut values: Vec<f64>, percentile: f64) -> f64 {
    values.sort_by(f64::total_cmp);
    let index = ((values.len() - 1) as f64 * percentile).round() as usize;
    values[index]
}
