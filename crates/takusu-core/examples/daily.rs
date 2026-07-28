//! Example: 1日のスケジュールを 8 タスクで構築
//!
//! - 依存関係: 買い物前にメニューを決める、資料作成は調査後
//! - 並列: 洗濯しながら読書、移動中にメール
//! - 諦めやすさ: 筋トレは deadline 超過してもやる（abandonability 高 = ペナルティ軽い）
//! - 睡眠: 22:00-06:00 を保護
//!
//! 実行: cargo run --example daily

use std::collections::HashMap;

use takusu_core::{NormalDist, Planner, Point, SleepConfig, Slots, Task};

fn fmt_time(slot: i64) -> String {
    let total_minutes = Slots(slot).to_minutes().0;
    let hours = total_minutes / 60;
    let minutes = total_minutes % 60;
    format!("{:02}:{:02}", hours % 24, minutes)
}

fn name_of<'a>(ids: &'a HashMap<&str, usize>, id: &usize) -> &'a str {
    ids.iter()
        .find(|(_, v)| **v == *id)
        .map(|(k, _)| *k)
        .unwrap_or("?")
}

fn fmt_duration(slots: i64) -> String {
    let total_minutes = Slots(slots).to_minutes().0;
    if total_minutes >= 60 {
        format!("{}h{:02}m", total_minutes / 60, total_minutes % 60)
    } else {
        format!("{}m", total_minutes)
    }
}

fn main() {
    // 06:00 起点 = slot 72 (6*12)
    let now = Point(72);

    let mut planner = Planner::new(now, SleepConfig::recommended());

    // ── タスク定義 ──────────────────────────────────────────────
    //
    // ID を明示する。add() の戻り値で実際の ID が返るが、
    // 依存関係の記述には予めわかっている値を使う。
    // ここでは 0 から順に追加される前提で書く。

    let mut ids = HashMap::new();

    // 0: 朝食 (30分, 確定)
    ids.insert(
        "朝食",
        planner
            .add(Task {
                id: 0,
                start: Some(Point(72)),               // 06:00 以降
                end: Point(96),                       // 08:00 までに
                cost_estimate: NormalDist::new(6, 1), // 30分 ±5分
                depends: vec![],
                parallelizable: false,
                allows_parallel: false,
                abandonability: 0.1.into(), // // ほぼ諦めない
                fixed: false,
                habit_group: None,
            })
            .unwrap(),
    );

    // 1: 洗濯 (60分, やや不安定)
    ids.insert(
        "洗濯",
        planner
            .add(Task {
                id: 0,
                start: Some(Point(72)),
                end: Point(192),                       // 16:00 までに
                cost_estimate: NormalDist::new(12, 3), // 60分 ±15分
                depends: vec![],
                parallelizable: false,
                allows_parallel: true, // 洗濯中に他のことできる
                abandonability: 0.2.into(),
                fixed: false,
                habit_group: None,
            })
            .unwrap(),
    );

    // 2: 読書 (45分, 洗濯と並行可)
    ids.insert(
        "読書",
        planner
            .add(Task {
                id: 0,
                start: Some(Point(72)),
                end: Point(192),
                cost_estimate: NormalDist::new(9, 2),
                depends: vec![],
                parallelizable: true, // 他のタスク中でも可
                allows_parallel: false,
                abandonability: 0.6.into(), // // まあできなくてもいい
                fixed: false,
                habit_group: None,
            })
            .unwrap(),
    );

    // 3: 調査 (90分, 安定)
    let survey_id = planner
        .add(Task {
            id: 0,
            start: Some(Point(96)),                // 08:00 以降
            end: Point(168),                       // 14:00 までに
            cost_estimate: NormalDist::new(18, 1), // 90分 ±5分
            depends: vec![],
            parallelizable: false,
            allows_parallel: false,
            abandonability: 0.1.into(),
            fixed: false,
            habit_group: None,
        })
        .unwrap();
    ids.insert("調査", survey_id);

    // 4: 資料作成 (60分, 調査に依存)
    ids.insert(
        "資料作成",
        planner
            .add(Task {
                id: 0,
                start: Some(Point(96)),
                end: Point(216), // 18:00 までに
                cost_estimate: NormalDist::new(12, 2),
                depends: vec![survey_id], // 調査が終わってから
                parallelizable: false,
                allows_parallel: false,
                abandonability: 0.2.into(),
                fixed: false,
                habit_group: None,
            })
            .unwrap(),
    );

    // 5: 買い物リスト (15分, 依存なし)
    let list_id = planner
        .add(Task {
            id: 0,
            start: Some(Point(72)),
            end: Point(168),
            cost_estimate: NormalDist::new(3, 1), // 15分 ±5分
            depends: vec![],
            parallelizable: false,
            allows_parallel: false,
            abandonability: 0.3.into(),
            fixed: false,
            habit_group: None,
        })
        .unwrap();
    ids.insert("買い物リスト", list_id);

    // 6: 買い物 (60分, リストに依存)
    ids.insert(
        "買い物",
        planner
            .add(Task {
                id: 0,
                start: Some(Point(168)),               // 14:00 以降
                end: Point(264),                       // 22:00 までに
                cost_estimate: NormalDist::new(12, 4), // 60分 ±20分
                depends: vec![list_id],
                parallelizable: false,
                allows_parallel: false,
                abandonability: 0.2.into(),
                fixed: false,
                habit_group: None,
            })
            .unwrap(),
    );

    // 7: 筋トレ (40分, deadline 超過してもやる)
    ids.insert(
        "筋トレ",
        planner
            .add(Task {
                id: 0,
                start: Some(Point(72)),
                end: Point(216), // 18:00 目標
                cost_estimate: NormalDist::new(8, 2),
                depends: vec![],
                parallelizable: false,
                allows_parallel: false,
                abandonability: 0.8.into(), // // deadline 超過を大きく許容
                fixed: false,
                habit_group: None,
            })
            .unwrap(),
    );

    // 8: 移動＋メール (移動中にメール, 45分)
    ids.insert(
        "移動",
        planner
            .add(Task {
                id: 0,
                start: Some(Point(168)),
                end: Point(264),
                cost_estimate: NormalDist::new(9, 2), // 45分 ±10分
                depends: vec![],
                parallelizable: false,
                allows_parallel: true, // 移動中にメールできる
                abandonability: 0.1.into(),
                fixed: false,
                habit_group: None,
            })
            .unwrap(),
    );

    ids.insert(
        "メール",
        planner
            .add(Task {
                id: 0,
                start: Some(Point(168)),
                end: Point(264),
                cost_estimate: NormalDist::new(3, 1), // 15分 ±5分
                depends: vec![],
                parallelizable: true, // 他のタスク(移動)中にできる
                allows_parallel: false,
                abandonability: 0.3.into(),
                fixed: false,
                habit_group: None,
            })
            .unwrap(),
    );

    // ── スケジューリング ──────────────────────────────────────

    let plan = planner.plan();
    let tasks = planner.tasks();

    // ── 表示 ──────────────────────────────────────────────────

    println!("╔══════════════════════════════════════════════════╗");
    println!("║  1 日のスケジュール (06:00 〜)                  ║");
    println!("║  睡眠: 22:00-06:00 (保護)                      ║");
    println!("╚══════════════════════════════════════════════════╝");
    println!();

    let mut sorted: Vec<_> = plan.schedules.iter().collect();
    sorted.sort_by_key(|p| p.start.0);

    for p in &sorted {
        let name = name_of(&ids, &p.task_id);
        let task = &tasks[p.task_id];
        let dur = p.end.0 - p.start.0;
        let sigma = task.cost_estimate.sigma;

        let deadline_flag = if p.end > task.end { " ⚠ over" } else { "" };
        let sigma_str = if sigma > 0 {
            format!(" σ={}", sigma)
        } else {
            String::new()
        };
        let parallel = if task.parallelizable || task.allows_parallel {
            let mut tags = vec![];
            if task.allows_parallel {
                tags.push("host");
            }
            if task.parallelizable {
                tags.push("guest");
            }
            format!(" [{}]", tags.join("+"))
        } else {
            String::new()
        };

        println!(
            "  {}  {:>6}  {:>5} → {:<5}  (推定 {}){}{}{}",
            if p.end <= task.end { "✓" } else { "△" },
            name,
            fmt_time(p.start.0),
            fmt_time(p.end.0),
            fmt_duration(dur),
            sigma_str,
            parallel,
            deadline_flag,
        );
    }

    // 依存関係チェック
    println!();
    println!("依存関係:");
    for task in planner.tasks() {
        for dep in &task.depends {
            let from = name_of(&ids, dep);
            let to = name_of(&ids, &task.id);
            let dep_end = plan.task_end(*dep);
            let self_start = plan.task_start(task.id);
            let ok = match (dep_end, self_start) {
                (Some(de), Some(ss)) if de <= ss => "✓",
                _ => "✗",
            };
            println!(
                "  {} {} → {}  ({} ends {}, starts {})",
                ok,
                from,
                to,
                from,
                dep_end.map(|p| fmt_time(p.0)).unwrap_or_default(),
                self_start.map(|p| fmt_time(p.0)).unwrap_or_default(),
            );
        }
    }

    // 並列チェック
    println!();
    println!("時間重複 (並列実行):");
    for i in 0..sorted.len() {
        let a_p = sorted[i];
        for b_p in sorted.iter().skip(i + 1) {
            if a_p.end <= b_p.start || b_p.end <= a_p.start {
                continue;
            }
            let a_name = name_of(&ids, &a_p.task_id);
            let b_name = name_of(&ids, &b_p.task_id);
            let task_a = &tasks[a_p.task_id];
            let task_b = &tasks[b_p.task_id];
            let parallel_ok = (task_a.allows_parallel && task_b.parallelizable)
                || (task_b.allows_parallel && task_a.parallelizable);
            let mark = if parallel_ok { "✓" } else { "⚠" };
            println!(
                "  {} {} ⊗ {}  ({}–{} ⊗ {}–{})",
                mark,
                a_name,
                b_name,
                fmt_time(a_p.start.0),
                fmt_time(a_p.end.0),
                fmt_time(b_p.start.0),
                fmt_time(b_p.end.0),
            );
        }
    }
}
