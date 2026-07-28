//! SA Solver (sa_lns) と Priority Solver (alns_search) の解を比較して HTML レポートを出力する。
//!
//! 実行:
//!   cargo run --example compare_solvers -p takusu-core
//!
//! 環境変数:
//!   TAKUSU_COMPARE_FIXTURE  (default: "7d")
//!   TAKUSU_COMPARE_SEEDS    (default: 1)
//!   TAKUSU_COMPARE_OUTPUT   (default: "compare_solvers.html")

mod common;

use std::collections::BTreeMap;
use std::time::Instant;

use takusu_core::{ParallelMode, Plan, Planner, Point, Slots, Task, TaskPlacement};

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

fn main() {
    let fixture_name = env_or("TAKUSU_COMPARE_FIXTURE", "7d");
    let seeds: u64 = env_or("TAKUSU_COMPARE_SEEDS", "1")
        .parse()
        .expect("TAKUSU_COMPARE_SEEDS must be a non-negative integer");
    let output_path = env_or("TAKUSU_COMPARE_OUTPUT", "compare_solvers.html");

    let planner = build_fixture(&fixture_name);
    let task_count = planner.tasks().len();
    if task_count == 0 {
        eprintln!("fixture {} has no tasks", fixture_name);
        return;
    }

    eprintln!(
        "[compare] fixture={} seeds={} tasks={}",
        fixture_name, seeds, task_count
    );

    let mut sa_runs = Vec::with_capacity(seeds as usize);
    let mut alns_runs = Vec::with_capacity(seeds as usize);

    for seed in 0..seeds {
        let start = Instant::now();
        let plan = planner.plan_with_seed(seed);
        let elapsed = start.elapsed().as_secs_f64() * 1000.0;
        sa_runs.push((seed, plan, elapsed));

        let start = Instant::now();
        let plan = planner.plan_alns_with_seed(seed);
        let elapsed = start.elapsed().as_secs_f64() * 1000.0;
        alns_runs.push((seed, plan, elapsed));
    }

    let sa_runs: Vec<Run> = sa_runs
        .into_iter()
        .map(|(seed, plan, elapsed)| {
            let metrics = metrics(&planner, &plan);
            Run {
                seed,
                plan,
                elapsed_ms: elapsed,
                metrics,
            }
        })
        .collect();
    let alns_runs: Vec<Run> = alns_runs
        .into_iter()
        .map(|(seed, plan, elapsed)| {
            let metrics = metrics(&planner, &plan);
            Run {
                seed,
                plan,
                elapsed_ms: elapsed,
                metrics,
            }
        })
        .collect();

    let best_sa = sa_runs
        .iter()
        .max_by(|a, b| a.metrics.score.total_cmp(&b.metrics.score))
        .expect("at least one SA run");
    let best_alns = alns_runs
        .iter()
        .max_by(|a, b| a.metrics.score.total_cmp(&b.metrics.score))
        .expect("at least one priority run");

    let (global_min, global_max) = global_range(&[&best_sa.plan, &best_alns.plan]);

    let html = render_html(
        &fixture_name,
        seeds,
        task_count,
        &planner,
        &sa_runs,
        &alns_runs,
        best_sa,
        best_alns,
        global_min,
        global_max,
    );

    std::fs::write(&output_path, html).expect("failed to write HTML");
    eprintln!("[compare] wrote {}", output_path);
    println!("{}", output_path);
}

struct Run {
    seed: u64,
    plan: Plan,
    elapsed_ms: f64,
    metrics: Metrics,
}

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
}

fn build_fixture(name: &str) -> Planner {
    match name {
        "small" => common::build_planner_small(),
        "stress_30d" => common::build_stress_30d(),
        "stress_30d_dense" => common::build_stress_30d_dense(),
        "stress_30d_mixed" => common::build_stress_30d_mixed(),
        "stress_90d" => common::build_stress_90d(),
        "7d" => common::build_planner_7d(),
        "14d" => common::build_planner(),
        "30d" => common::build_planner_30d(),
        _ => panic!(
            "unknown fixture: {}. choose one of: small, stress_30d, stress_30d_dense, stress_30d_mixed, stress_90d, 7d, 14d, 30d",
            name
        ),
    }
}

fn metrics(planner: &Planner, plan: &Plan) -> Metrics {
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
            let Some(a) = planner.tasks().get(a_p.task_id) else {
                continue;
            };
            let Some(b) = planner.tasks().get(b_p.task_id) else {
                continue;
            };
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
    }
}

fn sleep_shortage(planner: &Planner, plan: &Plan) -> (usize, i64) {
    let slots_per_day = 24 * 60 / planner.per() as i64;
    let sleep = planner.sleep_config();
    if !sleep.enabled || plan.schedules.is_empty() {
        return (0, 0);
    }
    let plan_start = plan.schedules.iter().map(|p| p.start.0).min().unwrap();
    let plan_end = plan.schedules.iter().map(|p| p.end.0).max().unwrap();
    let first_day = sleep.day_start
        + (plan_start - sleep.day_start).div_euclid(slots_per_day) * slots_per_day
        - slots_per_day;
    let sleep_len = sleep.end - sleep.start;
    let mut shortage_days = 0;
    let mut shortage_slots = 0;
    let mut day_start = first_day;
    while day_start + sleep.start <= plan_end {
        let window_start = day_start + sleep.start;
        let window_end = day_start + sleep.end;
        let occupied: Vec<_> = plan
            .schedules
            .iter()
            .map(|p| (p.start.0.max(window_start), p.end.0.min(window_end)))
            .filter(|(s, e)| s < e)
            .collect();
        let got = (sleep_len - union_length(&occupied)).max(0);
        if !occupied.is_empty() && got < 36 {
            shortage_days += 1;
            shortage_slots += 36 - got;
        }
        day_start += slots_per_day;
    }
    (shortage_days, shortage_slots)
}

fn daily_maximum_excess(planner: &Planner, plan: &Plan) -> i64 {
    let slots_per_day = 24 * 60 / planner.per() as i64;
    let mut loads = BTreeMap::<i64, Vec<(i64, i64)>>::new();
    for p in &plan.schedules {
        let mut cursor = p.start.0;
        while cursor < p.end.0 {
            let day = cursor.div_euclid(slots_per_day);
            let day_end = (day + 1) * slots_per_day;
            let segment_end = p.end.0.min(day_end);
            loads.entry(day).or_default().push((cursor, segment_end));
            cursor = segment_end;
        }
    }
    let maximum = planner.workload().maximum_slots_per_day;
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

fn global_range(plans: &[&Plan]) -> (i64, i64) {
    let min = plans
        .iter()
        .filter(|p| !p.schedules.is_empty())
        .map(|p| p.schedules.iter().map(|p| p.start.0).min().unwrap())
        .min()
        .unwrap_or(0);
    let max = plans
        .iter()
        .filter(|p| !p.schedules.is_empty())
        .map(|p| p.schedules.iter().map(|p| p.end.0).max().unwrap())
        .max()
        .unwrap_or(min + 1);
    (min, max)
}

#[allow(clippy::too_many_arguments)]
fn render_html(
    fixture_name: &str,
    seeds: u64,
    task_count: usize,
    planner: &Planner,
    sa_runs: &[Run],
    alns_runs: &[Run],
    best_sa: &Run,
    best_alns: &Run,
    global_min: i64,
    global_max: i64,
) -> String {
    let mut out = String::new();

    out.push_str("<!DOCTYPE html>\n<html lang=\"ja\">\n<head>\n");
    out.push_str("<meta charset=\"utf-8\" />\n");
    out.push_str("<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\" />\n");
    out.push_str(&format!(
        "<title>SA vs Priority Solver - {}</title>\n",
        escape_html(fixture_name)
    ));
    out.push_str(STYLE);
    out.push_str("</head>\n<body>\n");

    out.push_str("<div class=\"container\">\n");
    out.push_str("<h1>SA Solver vs Priority Solver</h1>\n");
    out.push_str(&format!(
        "<p class=\"subtitle\">fixture: <b>{}</b> &middot; seeds: {} &middot; tasks: {}</p>\n",
        escape_html(fixture_name),
        seeds,
        task_count
    ));

    out.push_str("<div class=\"cards\">\n");
    summary_card(&mut out, "SA (sa_lns)", best_sa);
    summary_card(&mut out, "Priority (alns_search)", best_alns);
    out.push_str("</div>\n");

    if sa_runs.len() > 1 {
        out.push_str("<h2>per-seed comparison</h2>\n");
        out.push_str("<div class=\"table-wrap\">\n");
        out.push_str("<table class=\"data\">\n");
        out.push_str("<tr><th>seed</th><th>SA score</th><th>SA ms</th><th>Priority score</th><th>Priority ms</th><th>delta</th><th>winner</th></tr>\n");
        for i in 0..sa_runs.len() {
            let sa = &sa_runs[i];
            let pr = &alns_runs[i];
            let delta = sa.metrics.score - pr.metrics.score;
            let winner = if sa.metrics.score > pr.metrics.score {
                "SA"
            } else if sa.metrics.score < pr.metrics.score {
                "Priority"
            } else {
                "tie"
            };
            let delta_class = if delta > 0.0 {
                "good"
            } else if delta < 0.0 {
                "bad"
            } else {
                ""
            };
            out.push_str(&format!(
                "<tr><td>{}</td><td>{:.3}</td><td>{:.2}</td><td>{:.3}</td><td>{:.2}</td><td class=\"{}\">{:+.3}</td><td>{}</td></tr>\n",
                sa.seed,
                sa.metrics.score,
                sa.elapsed_ms,
                pr.metrics.score,
                pr.elapsed_ms,
                delta_class,
                delta,
                winner
            ));
        }
        out.push_str("</table>\n</div>\n");
    }

    out.push_str("<h2>best solution timeline</h2>\n");
    out.push_str("<div class=\"timelines\">\n");
    timeline(
        &mut out,
        planner,
        "SA",
        &best_sa.plan,
        global_min,
        global_max,
    );
    timeline(
        &mut out,
        planner,
        "Priority",
        &best_alns.plan,
        global_min,
        global_max,
    );
    out.push_str("</div>\n");

    out.push_str("<h2>metrics</h2>\n");
    out.push_str("<div class=\"table-wrap\">\n");
    out.push_str("<table class=\"data\">\n");
    out.push_str("<tr><th>metric</th><th>SA</th><th>Priority</th></tr>\n");
    metric_row(
        &mut out,
        "score",
        best_sa.metrics.score,
        best_alns.metrics.score,
        true,
    );
    metric_row_usize(
        &mut out,
        "missing",
        best_sa.metrics.missing,
        best_alns.metrics.missing,
        false,
    );
    metric_row_usize(
        &mut out,
        "duplicates",
        best_sa.metrics.duplicates,
        best_alns.metrics.duplicates,
        false,
    );
    metric_row_i64(
        &mut out,
        "overlap slots",
        best_sa.metrics.overlap_slots,
        best_alns.metrics.overlap_slots,
        false,
    );
    metric_row_i64(
        &mut out,
        "dependency violation slots",
        best_sa.metrics.dependency_slots,
        best_alns.metrics.dependency_slots,
        false,
    );
    metric_row_i64(
        &mut out,
        "start violation slots",
        best_sa.metrics.start_slots,
        best_alns.metrics.start_slots,
        false,
    );
    metric_row_usize(
        &mut out,
        "sleep shortage days",
        best_sa.metrics.sleep_shortage_days,
        best_alns.metrics.sleep_shortage_days,
        false,
    );
    metric_row_i64(
        &mut out,
        "sleep shortage slots",
        best_sa.metrics.sleep_shortage_slots,
        best_alns.metrics.sleep_shortage_slots,
        false,
    );
    metric_row_i64(
        &mut out,
        "daily maximum excess",
        best_sa.metrics.daily_maximum_excess,
        best_alns.metrics.daily_maximum_excess,
        false,
    );
    metric_row(
        &mut out,
        "latency (ms)",
        best_sa.elapsed_ms,
        best_alns.elapsed_ms,
        false,
    );
    out.push_str("</table>\n</div>\n");

    out.push_str("<h2>per-task detail</h2>\n");
    out.push_str("<div class=\"table-wrap\">\n");
    out.push_str("<table class=\"data detail\">\n");
    out.push_str("<tr>");
    out.push_str("<th rowspan=\"2\">id</th>");
    out.push_str("<th rowspan=\"2\">avg</th>");
    out.push_str("<th rowspan=\"2\">deadline</th>");
    out.push_str("<th colspan=\"4\">SA</th>");
    out.push_str("<th colspan=\"4\">Priority</th>");
    out.push_str("</tr><tr>");
    out.push_str("<th>start</th><th>end</th><th>on time</th><th>deps ok</th>");
    out.push_str("<th>start</th><th>end</th><th>on time</th><th>deps ok</th>");
    out.push_str("</tr>\n");

    for task in planner.tasks() {
        let sa = best_sa.plan.schedules.iter().find(|p| p.task_id == task.id);
        let pr = best_alns
            .plan
            .schedules
            .iter()
            .find(|p| p.task_id == task.id);
        let deadline_label = fmt_relative(task.end.0, global_min);
        out.push_str("<tr>");
        out.push_str(&format!("<td>{}</td>", task.id));
        out.push_str(&format!(
            "<td>{}</td>",
            fmt_duration(task.cost_estimate.avg as i64)
        ));
        out.push_str(&format!("<td>{}</td>", deadline_label));
        task_cells(&mut out, planner, task, sa, &best_sa.plan, global_min);
        task_cells(&mut out, planner, task, pr, &best_alns.plan, global_min);
        out.push_str("</tr>\n");
    }
    out.push_str("</table>\n</div>\n");

    out.push_str("</div>\n</body>\n</html>\n");
    out
}

fn summary_card(out: &mut String, label: &str, run: &Run) {
    out.push_str("<div class=\"card\">\n");
    out.push_str(&format!("<h3>{}</h3>\n", escape_html(label)));
    out.push_str(&format!(
        "<div class=\"big\">{:.3}</div>\n",
        run.metrics.score
    ));
    out.push_str("<div class=\"muted\">best score</div>\n");
    out.push_str("<div class=\"stats\">\n");
    out.push_str(&format!("<span>missing: {}</span>", run.metrics.missing));
    out.push_str(&format!(
        "<span>overlap: {}</span>",
        run.metrics.overlap_slots
    ));
    out.push_str(&format!("<span>time: {:.2} ms</span>", run.elapsed_ms));
    out.push_str("</div>\n");
    out.push_str("</div>\n");
}

fn timeline(out: &mut String, planner: &Planner, label: &str, plan: &Plan, min: i64, max: i64) {
    let slots_per_day = 24 * 60 / planner.per() as i64;
    let span = (max - min).max(1) as f64;
    let day_interval = ((max - min) / slots_per_day / 30 + 1).max(1);

    let mut sorted: Vec<_> = plan.schedules.to_vec();
    sorted.sort_by(|a, b| a.start.0.cmp(&b.start.0).then(b.end.0.cmp(&a.end.0)));

    let mut lanes: Vec<Vec<usize>> = Vec::new();
    let mut lane_ends: Vec<i64> = Vec::new();
    for (i, item) in sorted.iter().enumerate() {
        let start = item.start.0;
        let mut placed = false;
        for (lane_idx, last_end) in lane_ends.iter_mut().enumerate() {
            if *last_end <= start {
                lanes[lane_idx].push(i);
                *last_end = item.end.0;
                placed = true;
                break;
            }
        }
        if !placed {
            lanes.push(vec![i]);
            lane_ends.push(item.end.0);
        }
    }

    let lane_height = 28i64;
    let total_height = lanes.len() as i64 * lane_height + 32;

    out.push_str("<div class=\"timeline-panel\">\n");
    out.push_str(&format!("<h3>{}</h3>\n", escape_html(label)));
    out.push_str("<div class=\"timeline\" style=\"height:");
    out.push_str(&total_height.to_string());
    out.push_str("px\">\n");

    let mut day = day_interval;
    while min + day * slots_per_day <= max {
        let pos = min + day * slots_per_day;
        let left = (pos - min) as f64 / span * 100.0;
        out.push_str(&format!(
            "<div class=\"grid-line\" style=\"left:{:.2}%\"></div>\n",
            left
        ));
        out.push_str(&format!(
            "<div class=\"grid-label\" style=\"left:{:.2}%\">+{}d</div>\n",
            left, day
        ));
        day += day_interval;
    }

    for (lane_idx, lane) in lanes.iter().enumerate() {
        for &sched_idx in lane {
            let p = sorted[sched_idx];
            let task = &planner.tasks()[p.task_id];
            let left = (p.start.0 - min) as f64 / span * 100.0;
            let width = ((p.end.0 - p.start.0) as f64 / span * 100.0).max(0.3);
            let top = lane_idx as i64 * lane_height + 24;
            let color = task_color(p.task_id);
            let show_label = width > 2.0;
            let label = if show_label {
                format!(
                    "{} {}–{}",
                    p.task_id,
                    fmt_relative(p.start.0, min),
                    fmt_relative(p.end.0, min)
                )
            } else {
                String::new()
            };
            let title = format!(
                "task {}: {} – {} (avg {} dur {})",
                p.task_id,
                fmt_relative(p.start.0, min),
                fmt_relative(p.end.0, min),
                fmt_duration(task.cost_estimate.avg as i64),
                fmt_duration(p.end.0 - p.start.0)
            );
            let border = if task.fixed {
                "1px solid #111827"
            } else {
                "none"
            };
            out.push_str(&format!(
                "<div class=\"bar\" style=\"left:{:.2}%;width:{:.2}%;top:{}px;background:{};border:{}\" title=\"{}\">{}</div>\n",
                left,
                width,
                top,
                color,
                border,
                escape_html(&title),
                escape_html(&label)
            ));
        }
    }

    out.push_str("</div>\n</div>\n");
}

fn task_cells(
    out: &mut String,
    planner: &Planner,
    task: &Task,
    sched: Option<&TaskPlacement>,
    plan: &Plan,
    origin: i64,
) {
    let mut by_id: Vec<Option<(Point, Point)>> = vec![None; planner.tasks().len()];
    for p in &plan.schedules {
        if let Some(slot) = by_id.get_mut(p.task_id) {
            *slot = Some((p.start, p.end));
        }
    }

    match sched {
        Some(sp) => {
            let on_time = sp.end.0 <= task.end.0;
            let mut deps_ok = true;
            for dep in &task.depends {
                if let Some((_, dep_end)) = by_id.get(*dep).and_then(|slot| *slot) {
                    if dep_end.0 > sp.start.0 {
                        deps_ok = false;
                        break;
                    }
                } else {
                    deps_ok = false;
                    break;
                }
            }
            let start_ok = task
                .start
                .map(|min_start| sp.start.0 >= min_start.0)
                .unwrap_or(true);
            if !start_ok {
                deps_ok = false;
            }
            out.push_str(&format!(
                "<td>{}</td><td>{}</td><td class=\"{}\">{}</td><td class=\"{}\">{}</td>",
                fmt_relative(sp.start.0, origin),
                fmt_relative(sp.end.0, origin),
                if on_time { "ok" } else { "bad" },
                if on_time { "✓" } else { "✗" },
                if deps_ok { "ok" } else { "bad" },
                if deps_ok { "✓" } else { "✗" }
            ));
        }
        None => {
            out.push_str("<td>—</td><td>—</td><td class=\"bad\">✗</td><td>—</td>");
        }
    }
}

fn metric_row(out: &mut String, name: &str, a: f64, b: f64, higher_is_better: bool) {
    let cmp = a.total_cmp(&b);
    let a_class = match cmp {
        std::cmp::Ordering::Greater if higher_is_better => "good",
        std::cmp::Ordering::Less if higher_is_better => "bad",
        std::cmp::Ordering::Less if !higher_is_better => "good",
        std::cmp::Ordering::Greater if !higher_is_better => "bad",
        _ => "",
    };
    let b_class = match cmp {
        std::cmp::Ordering::Greater if higher_is_better => "bad",
        std::cmp::Ordering::Less if higher_is_better => "good",
        std::cmp::Ordering::Less if !higher_is_better => "bad",
        std::cmp::Ordering::Greater if !higher_is_better => "good",
        _ => "",
    };
    out.push_str(&format!(
        "<tr><td>{}</td><td class=\"{}\">{:.3}</td><td class=\"{}\">{:.3}</td></tr>\n",
        escape_html(name),
        a_class,
        a,
        b_class,
        b
    ));
}

fn metric_row_i64(out: &mut String, name: &str, a: i64, b: i64, higher_is_better: bool) {
    metric_row(out, name, a as f64, b as f64, higher_is_better);
}

fn metric_row_usize(out: &mut String, name: &str, a: usize, b: usize, higher_is_better: bool) {
    metric_row(out, name, a as f64, b as f64, higher_is_better);
}

fn fmt_relative(slot: i64, origin: i64) -> String {
    let total_minutes = Slots(slot - origin).to_minutes().0;
    let day = total_minutes.div_euclid(1440);
    let rem = total_minutes.rem_euclid(1440);
    let h = rem / 60;
    let m = rem % 60;
    if day == 0 {
        format!("{:02}:{:02}", h, m)
    } else {
        format!(
            "{}{}d {:02}:{:02}",
            if day > 0 { "+" } else { "" },
            day,
            h,
            m
        )
    }
}

fn fmt_duration(slots: i64) -> String {
    let minutes = Slots(slots).to_minutes().0;
    if minutes >= 60 {
        format!("{}h{:02}m", minutes / 60, minutes % 60)
    } else {
        format!("{}m", minutes)
    }
}

fn task_color(id: usize) -> String {
    let hue = (id.wrapping_mul(47)) % 360;
    format!("hsl({}, 70%, 60%)", hue)
}

fn escape_html(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#x27;")
}

const STYLE: &str = r#"<style>
:root { --bg: #f8fafc; --card: #ffffff; --text: #111827; --muted: #64748b; --ok: #16a34a; --bad: #dc2626; --accent: #2563eb; }
* { box-sizing: border-box; }
body { font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, "Hiragino Sans", "Noto Sans JP", sans-serif; background: var(--bg); color: var(--text); margin: 0; padding: 0; }
.container { max-width: 1400px; margin: 0 auto; padding: 32px 24px; }
h1 { font-size: 2rem; margin: 0 0 8px; }
h2 { font-size: 1.25rem; margin: 32px 0 16px; border-bottom: 2px solid #e2e8f0; padding-bottom: 8px; }
h3 { font-size: 1rem; margin: 0 0 12px; }
.subtitle { color: var(--muted); margin: 0 0 24px; }
.cards { display: grid; grid-template-columns: repeat(auto-fit, minmax(280px, 1fr)); gap: 16px; margin-bottom: 24px; }
.card { background: var(--card); border-radius: 12px; padding: 20px; box-shadow: 0 1px 3px rgba(0,0,0,0.08); }
.card .big { font-size: 2rem; font-weight: 700; color: var(--accent); }
.card .muted { color: var(--muted); margin-bottom: 12px; }
.card .stats { display: flex; flex-wrap: wrap; gap: 8px; }
.card .stats span { background: #f1f5f9; border-radius: 6px; padding: 4px 8px; font-size: 0.875rem; }
.table-wrap { overflow-x: auto; background: var(--card); border-radius: 12px; padding: 12px; box-shadow: 0 1px 3px rgba(0,0,0,0.08); }
table.data { width: 100%; border-collapse: collapse; font-size: 0.9rem; }
table.data th, table.data td { padding: 8px 12px; text-align: left; border-bottom: 1px solid #e2e8f0; }
table.data th { color: var(--muted); font-weight: 600; }
table.data tr:last-child td { border-bottom: none; }
table.data.detail th[colspan] { text-align: center; background: #f8fafc; }
table.data.detail td { text-align: center; }
.ok { color: var(--ok); font-weight: 600; }
.bad { color: var(--bad); font-weight: 600; }
.good { color: var(--ok); font-weight: 600; }
.timelines { display: grid; grid-template-columns: repeat(auto-fit, minmax(600px, 1fr)); gap: 16px; }
.timeline-panel { background: var(--card); border-radius: 12px; padding: 16px; box-shadow: 0 1px 3px rgba(0,0,0,0.08); }
.timeline { position: relative; border-left: 1px solid #cbd5e1; border-right: 1px solid #cbd5e1; background: linear-gradient(to right, #f1f5f9 1px, transparent 1px); background-size: 4.166% 100%; }
.grid-line { position: absolute; top: 0; bottom: 0; width: 1px; background: #94a3b8; opacity: 0.4; }
.grid-label { position: absolute; top: 4px; font-size: 0.75rem; color: var(--muted); transform: translateX(-50%); }
.bar { position: absolute; height: 22px; border-radius: 4px; font-size: 0.7rem; line-height: 22px; padding: 0 4px; overflow: hidden; white-space: nowrap; color: #111827; box-shadow: inset 0 0 0 1px rgba(255,255,255,0.3); cursor: default; }
@media (max-width: 800px) { .timelines { grid-template-columns: 1fr; } }
</style>"#;
