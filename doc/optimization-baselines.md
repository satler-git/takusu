# Optimization Baselines

This file records stable baseline numbers for `takusu-core` performance work.

- Record numbers **before** starting an experiment.
- Use the same machine / load conditions when comparing.
- The `realworld` Criterion bench can be noisy on shared machines; prefer the
  release examples + `time ./target/release/examples/profile` for a stable
  wall-clock baseline.

## 2026-07-18: `main` before evaluate scratch-buffer optimization

- `cargo run -p takusu-core --example score_check` (debug):
  - score `-1844.372500`
  - total `2.837s`
  - mean `28.370907 µs`
- `cargo run -p takusu-core --example score_check --release`:
  - score `-1844.372500`
  - total `0.193s`
  - mean `1.931344 µs`
- `time ./target/release/examples/profile` (20 full `plan()` calls):
  - real `~3.51s` (three runs: 3.514s, 3.472s, 3.538s)
- `plan_in_range` over 14d fixture, 20 calls (manual `range_check` example):
  - total `1.758s`
  - mean `87.889 ms`

## 2026-07-18: after evaluate scratch-buffer + inline union optimization

- `cargo run -p takusu-core --example score_check` (debug):
  - score `-1844.372500`
  - total `2.350s`
  - mean `23.498439 µs`
- `cargo run -p takusu-core --example score_check --release`:
  - score `-1844.372500`
  - total `0.154s`
  - mean `1.541711 µs`
- `time ./target/release/examples/profile` (20 full `plan()` calls):
  - real `~3.45s` (three runs: 3.599s, 3.430s, 3.328s)
- `plan_in_range` over 14d fixture, 20 calls:
  - total `1.580s`
  - mean `79.0 ms`

## 2026-07-18: current `@` (kpzytzys 8e01c02a) before allocator experiments

- `cargo run -p takusu-core --example score_check` (debug):
  - score `-1844.372500`
  - total `2.268s`
  - mean `22.678947 µs`
- `cargo run -p takusu-core --example score_check --release`:
  - score `-1844.372500`
  - total `0.159s`
  - mean `1.594242 µs`
- `time ./target/release/examples/profile` (20 full `plan()` calls):
  - real `~3.11s` (three runs: 3.227s, 3.135s, 2.980s)
- `cargo bench -p takusu-core --bench realworld`:
  - `plan realworld habits (7d)`: `40.865 ms`
  - `plan realworld habits (30d)`: `749.92 ms`
  - `plan_partial realworld habits (14d, 5 pinned)`: `241.08 ms`
  - `plan_in_range realworld habits (14d, days 2-7)`: `84.368 ms`

## 2026-07-18: with `jemalloc` feature (`--features jemalloc`)

- `cargo run -p takusu-core --example score_check` (debug, `CFLAGS='-O2 -U_FORTIFY_SOURCE'`):
  - score `-1844.372500`
  - total `1.534s`
  - mean `15.343852 µs`
- `cargo run -p takusu-core --example score_check --release`:
  - score `-1844.372500`
  - total `0.126s`
  - mean `1.261039 µs`
- `time ./target/release/examples/profile` (20 full `plan()` calls):
  - real `~2.06s` (three runs: 2.135s, 2.000s, 2.036s)
- `cargo bench -p takusu-core --bench realworld --features jemalloc`:
  - `plan realworld habits (7d)`: `27.430 ms`
  - `plan realworld habits (30d)`: `474.38 ms`
  - `plan_partial realworld habits (14d, 5 pinned)`: `148.36 ms`
  - `plan_in_range realworld habits (14d, days 2-7)`: `53.667 ms`

### Notes

- `jemalloc` (via `tikv-jemallocator` 0.6.0) is now the **default** global allocator for `takusu-core`.
- `mimalloc` 0.1.50 was tried but removed; it did not outperform `jemalloc` and is no longer an option.
- A workspace `Cargo.toml` profile override (`[profile.dev|test.package."tikv-jemalloc-sys"] opt-level = 3`) fixes the `_FORTIFY_SOURCE` warnings-as-errors in Nix dev shells with newer glibc.
- arm64 build verified: `cargo ndk -t aarch64-linux-android check -p takusu-core` succeeds with the default `jemalloc` feature.

## 2026-07-18: release profile tuning (`codegen-units = 1`, `lto = "thin"`)

Compared `opt-level = "s"`, `"z"`, and `3` for `takusu-core` release builds.
All runs use `jemalloc` default.

- `opt-level = 3` (speed):
  - `cargo run -p takusu-core --example score_check --release`: `0.106s`, `1.056395 µs`
  - `time ./target/release/examples/profile` (20 calls): `~1.79s` (1.827s, 1.790s, 1.765s)
  - `cargo bench -p takusu-core --bench realworld`:
    - `plan realworld habits (7d)`: `25.464 ms`
    - `plan realworld habits (30d)`: `474.81 ms`
    - `plan_partial realworld habits (14d, 5 pinned)`: `142.47 ms`
    - `plan_in_range realworld habits (14d, days 2-7)`: `49.816 ms`
- `opt-level = "s"` (size):
  - `score_check` release: `0.135s`, `1.347508 µs`
  - `profile` 20 calls: `~2.43s` (2.445s, 2.431s, 2.412s)
- `opt-level = "z"` (aggressive size):
  - `score_check` release: `0.185s`, `1.850095 µs`
  - `profile` 20 calls: `~4.53s` (4.481s, 4.507s, 4.638s)

### Notes

- `opt-level = 3` is the fastest across all measured `takusu-core` workloads.
- `"s"` and `"z"` regress runtime, with `"z"` being dramatically slower.
- Final workspace `Cargo.toml` uses `codegen-units = 1`, `lto = "thin"`, `opt-level = 3`.

## 2026-07-18: current `@` (`mznorlwlozxntznprlxprrpzzrtrmoqp`) on top of `ee39af0` before this optimization pass

- `cargo run -p takusu-core --example score_check` (debug):
  - score `-1844.372500`
  - total `1.631s`
  - mean `16.314832 µs`
- `cargo run -p takusu-core --example score_check --release`:
  - score `-1844.372500`
  - total `0.115s`
  - mean `1.154685 µs`
- `time ./target/release/examples/profile` (20 full `plan()` calls):
  - real `1.857s`
- `cargo bench -p takusu-core --bench realworld`:
  - `plan realworld habits (7d)`: `29.246 ms`
  - `plan realworld habits (30d)`: `480.08 ms`
  - `plan_partial realworld habits (14d, 5 pinned)`: `135.17 ms`
  - `plan_in_range realworld habits (14d, days 2-7)`: `49.867 ms`

## 2026-07-18: after habit score scratch-buffer and group-sort optimization (change `m b`)

- `cargo run -p takusu-core --example score_check` (debug):
  - score `-1844.372500`
  - total `1.527s`
  - mean `15.272834 µs`
- `cargo run -p takusu-core --example score_check --release`:
  - score `-1844.372500`
  - total `0.105s`
  - mean `1.053581 µs`
- `time ./target/release/examples/profile` (20 full `plan()` calls):
  - wall-clock was too noisy during this session (system load varied between `1.6s` and `9s`) to report a stable final value; prefer the Criterion `realworld` bench below.
- `cargo bench -p takusu-core --bench realworld`:
  - `plan realworld habits (7d)`: `21.551 ms`
  - `plan realworld habits (30d)`: `430.51 ms`
  - `plan_partial realworld habits (14d, 5 pinned)`: `136.81 ms`
  - `plan_in_range realworld habits (14d, days 2-7)`: `48.520 ms`

## 2026-07-19: current `@` (`xmwoozvv 25bc00d3`) before optimization pass

- `cargo run -p takusu-core --example score_check` (debug):
  - score `-1844.372500`
  - total `2.340s`
  - mean `23.400908 µs`
- `cargo run -p takusu-core --example score_check --release`:
  - score `-1844.372500`
  - total `0.139s`
  - mean `1.386401 µs`
- `time ./target/release/examples/profile` (20 full `plan()` calls):
  - real `2.401s`
- `cargo bench -p takusu-core --bench realworld`:
  - `plan realworld habits (7d)`: `37.299 ms`
  - `plan realworld habits (30d)`: `899.64 ms`
  - `plan_partial realworld habits (14d, 5 pinned)`: `257.93 ms`
  - `plan_in_range realworld habits (14d, days 2-7)`: `75.925 ms`

## 2026-07-19: after monotonic union cursor + merged index/range build + faster parallel loop

- `cargo run -p takusu-core --example score_check` (debug):
  - score `-1844.372500`
  - total `1.447s`
  - mean `14.472389 µs`
- `cargo run -p takusu-core --example score_check --release`:
  - score `-1844.372500`
  - total `0.088s`
  - mean `0.879013 µs`
- `time ./target/release/examples/profile` (20 full `plan()` calls):
  - real `1.984s`
- `cargo bench -p takusu-core --bench realworld`:
  - `plan realworld habits (7d)`: `25.516 ms`
  - `plan realworld habits (30d)`: `468.73 ms`
  - `plan_partial realworld habits (14d, 5 pinned)`: `170.76 ms`
  - `plan_in_range realworld habits (14d, days 2-7)`: `56.721 ms`

## 2026-07-19: current parent before failed experiment pass (`d7a0d49`)

- `cargo run -p takusu-core --example score_check` (debug):
  - score `-1844.372500`
  - total `1.645s`
  - mean `16.447747 µs`
- `cargo run -p takusu-core --example score_check --release`:
  - score `-1844.372500`
  - total `0.084s`
  - mean `0.836737 µs`
- `time ./target/release/examples/profile` (20 full `plan()` calls):
  - real `~2.11s` (three runs: 2.284s, 2.012s, 2.029s)
- `cargo bench -p takusu-core --bench realworld`:
  - `plan realworld habits (7d)`: `22.064 ms`
  - `plan realworld habits (30d)`: `438.27 ms`
  - `plan_partial realworld habits (14d, 5 pinned)`: `142.35 ms`
  - `plan_in_range realworld habits (14d, days 2-7)`: `49.176 ms`

## 2026-07-25: habit 階層探索 (`tulqklsntssv`) — A/B vs parent `zvtlpmzkqtzz`

habit group を anchor (共有時刻帯) 単位の上位変数として扱う階層探索を導入。
SA 近傍 (`neighbor_habit_anchor` / `neighbor_habit_exception`) と ALNS
(`DestroyOperator::HabitGroup` + `RepairMode::HabitAnchor`) に追加。
公開 API は不変 (`Vec<Task>` + `habit_group` のまま、core 内部で再構築)。

同じマシン・背靠背で親コミット (baseline) と比較。`cargo bench -p takusu-core --bench realworld`:

| metric | parent (baseline) | naive 実装 | 最適化後 | 最適化後 / baseline |
|--------|------------------:|-----------:|---------:|--------------------:|
| `plan realworld habits (7d)` | `25.365 ms` | `42.643 ms` | `20.190 ms` | **0.80x** |
| `plan realworld habits (30d)` | `404.26 ms` | `656.72 ms` | `334.94 ms` | **0.83x** |
| `plan_partial realworld habits (14d, 5 pinned)` | `151.77 ms` | `210.00 ms` | `127.61 ms` | **0.84x** |
| `plan_in_range realworld habits (14d, days 2-7)` | `41.129 ms` | `77.451 ms` | `47.125 ms` | 1.15x |

### Notes

- **naive 実装は 1.4–1.9x の regression** だった。主因は SA ホットループ内で
  近傍生成のたびに habit グルーピングを再構築し、`FxHashMap` / `Vec` /
  `FxHashSet` を allocate していたこと。
- 修正: habit のグループ所属は solve 中で不変なので、`HabitIndex` として
  **solve ごとに 1 回だけ** `habit::build_index` で事前計算し、近傍生成では
  静的 member リストと現在配置から anchor を求めるようリファクタ。
  ホットループ内の HashMap allocate を除去。
- 修正後は 7d/30d/partial で baseline より高速、range のみ +15%。
  anchor move が複数 occurrence を 1 move で一貫配置するため、個別 shift 数回分の
  探索を 1 反復でこなせることが寄与していると考えられる。
- 確率配分: shift 17% / swap 17% / duration 14% / reorder 14% / repair_depend 14% /
  habit_anchor 10% / habit_exception 6% / lns 8%。habit がない fixture では
  habit 分岐は shift へ fallback し iteration を無駄にしない。
- 品質 benchmark (doc Phase 0) は未整備のため、habit 一貫性の定量比較は未実施。
  既存 nextest (144 tests) / clippy / fmt は clean。
- レビュー対応で `HabitGroup` に member 判定用 `FxHashSet` を持たせ O(1) 化、
  anchor 計算を O(k²) の厳密 1-median から O(k) の円周平均近似
  (`circular_mean_nearest`) へ変更、ALNS `DestroyOperator::HabitGroup` も
  `HabitIndex` を再利用。再 bench でも上記と同程度の latency
  (7d ~22ms / 30d ~326ms / partial ~117ms / range ~46ms)、
  品質 (score・hard violation) も baseline と一致を確認。
