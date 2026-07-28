# takusu コード品質改善候補まとめ

このドキュメントは、現時点のコードベースを独立して監査し、次の 4 つの基準に該当する問題を整理したものである。各項目は「問題の要約」「現在の型・実装」「推奨される修正」「修正の重み（小/中/大）」「該当箇所」を含む。

**監査基準**

1. Rust の作法に従っていなくて hack となっている
2. 型が適切に使用されておず型安全性を損ない、手動でいちいちバリデーションによって保証している
3. Trait で綺麗に抽象化できるのに、されていない
4. 今後の拡張性を損なう技術的負債

**対象外・留意事項**

- 単純なエラーメッセージ `String` や、自由入力の `title` / `description` / `body` フィールドは対象外とする。
- 既存の newtype（`Minutes` / `Slots` / `TaskPlacement` / `TimeWindow` / `Abandonability`）や `enum_label!` マクロ、`TypedTool` trait など、すでに型安全化が進んでいる箇所は対象外とする。
- 大規模な crate 分離は推奨しない。型と trait の変更に絞る。

---

## 1. 評価関数の重み定数が 17 個の `const f64` に散らばっている（`takusu-core`）

### 1.1 評価重みを一つの struct に集約できていない

- **問題の要約**: `evaluate.rs` の先頭に `W_EARLY` / `W_LATE` / `W_START` / `W_DEPEND_BASE` 等、17 個の重み定数がフラットに並んでいる。チューニング時に影響範囲が読めず、`Planner` が重みを受け取る経路もないため、実験ごとにコードを書き換えるしかない。
- **現在の型**: `const f64`（17 個）+ `const i64`（`MIN_SLEEP` / `STABILITY_RANGE`）
- **推奨型**:
  - `EvaluationWeights` struct を定義し、`Default` を実装する
  - `Planner` に `weights: EvaluationWeights` フィールドを追加し、`Planner::new` の引数か setter で差し替え可能にする
  - `evaluate_with_scratch` は `&EvaluationWeights` を受け取る
- **修正の重み**: 中
- **該当箇所**:
  - `crates/takusu-core/src/evaluate.rs:86-116`（重み定数群）
  - `crates/takusu-core/src/evaluate.rs:118-124`（`evaluate` が `Planner` から重みを引いていない）
  - `crates/takusu-core/src/evaluate.rs:135-156`（`evaluate_with_scratch` のシグネチャ）

### 1.2 SA の温度・反復パラメータもハードコード

- **問題の要約**: 焼きなましの初期温度 `t0`、冷却率 `alpha = 0.93`、`t_min = t0 * 1e-4`、`iter_per_temp = task_count * 30`、`STAGNATION_LIMIT = 3`、`LONG_SHIFT_ONE_IN = 5` がすべてマジックナンバーとして `anneal.rs` に埋め込まれている。タスク規模や要件が変わっても調整できない。
- **現在の型**: リテラル / ローカル `const`
- **推奨型**: `SaConfig` struct（`t0_factor` / `alpha` / `t_min_factor` / `iter_per_temp_factor` / `stagnation_limit` / `long_shift_one_in`）を定義し、`Planner` 経由で差し替え可能にする
- **修正の重み**: 中
- **該当箇所**:
  - `crates/takusu-core/src/anneal.rs:1219-1222`（`STAGNATION_LIMIT` / `LONG_SHIFT_ONE_IN`）
  - `crates/takusu-core/src/anneal.rs:1251-1254`（`t0` / `alpha` / `t_min` / `iter_per_temp`）
  - `crates/takusu-core/src/anneal.rs:1773-1796`（近傍選択の確率範囲 `0..=16` / `17..=33` 等）

### 1.3 近傍選択の確率範囲がマジックレンジ

- **問題の要約**: SA の近傍選択が `match rng.gen_range(0..=100) { 0..=16 => shift, 17..=33 => swap, ... }` のようにハードコードされた範囲で分岐している。確率を変えるには複数の範囲を手動で再計算する必要があり、合計が 100 からずれるとパニックする。
- **現在の型**: `match rng.gen_range(0..=100) { 範囲リテラル => ... }`
- **推奨型**: 重み付き選択（`WeightedIndex` または累積閾値の配列）を `SaConfig` から構築する
- **修正の重み**: 小
- **該当箇所**:
  - `crates/takusu-core/src/anneal.rs:1773-1796`

---

## 2. `Task` の並行実行フラグが `bool` 3 つで組み合わせを検証していない（`takusu-core`）

- **問題の要約**: `Task` は `parallelizable: bool`（他タスク実行中に動ける）、`allows_parallel: bool`（自タスク実行中に他を許す）、`fixed: bool`（開始時刻固定）の 3 つの bool を持つ。意味のある組み合わせは限られるが、型では表現されておらず、各呼び出し点で `if task.parallelizable && !task.allows_parallel` のように手動で判定している。
- **現在の型**: `bool` × 3
- **推奨型**:
  - `ParallelMode` enum（`Exclusive` / `Guest` / `Host` / `Bidirectional`）で `parallelizable` と `allows_parallel` を一つにする
  - `fixed` は `FixedPolicy` enum（`Movable` / `Pinned`）または別フィールドのままでよい
- **修正の重み**: 中
- **該当箇所**:
  - `crates/takusu-core/src/lib.rs:307-311`（`parallelizable` / `allows_parallel` / `fixed` フィールド）
  - `crates/takusu-core/src/evaluate.rs`（並行違反の判定ロジック）
  - `crates/takusu-core/src/anneal.rs`（近傍操作での `fixed` チェック）

---

## 3. SA の近傍操作関数が 8 種類でほぼ同じ構造を繰り返している（`takusu-core`）

- **問題の要約**: `neighbor_shift_into` / `neighbor_swap_into` / `neighbor_duration_into` / `neighbor_reorder_into` と、それぞれの `_at_into` 版が、固定タスクのスキップ・所要時間計算・配置更新の骨組みを共有している。コアの変異ロジックだけが異なるが、trait で抽出されていないため、新しい近傍を追加すると骨組みごとコピペすることになる。full と partial の間にも同じ重複がある。
- **現在の型**: 個別の `fn`（シグネチャは同じだが trait で束ねられていない）
- **推奨型**:
  - `NeighborOperator` trait を定義し、`apply` / `apply_at` を持たせる
  - 共通骨組み（fixed チェック・所要時間・配置更新）を trait の default method またはヘルパーに集約する
  - 近傍の追加は trait impl を書くだけで済むようにする
- **修正の重み**: 大
- **該当箇所**:
  - `crates/takusu-core/src/anneal.rs:1871-2082`（近傍操作関数群）
  - `crates/takusu-core/src/anneal.rs:1773-1796`（確率分岐も近傍ごとにハードコード）

---

## 4. `evaluate_with_scratch` が scratch バッファ 3 つを呼び出し元で管理させている（`takusu-core`）

- **問題の要約**: `evaluate_with_scratch` は `sorted: &mut Vec<Placement>` / `index: &mut Vec<Option<TimeWindow>>` / `habit_entries: &mut Vec<HabitGroupAnchor>` の 3 つの mutable バッファを引数に取る。SA ループ内で毎回同じ 3 つを渡す boilerplate が繰り返され、バッファのサイズやクリアタイミングも呼び出し側の責任になっている。
- **現在の型**: `&mut Vec<...>` × 3 を関数引数で受け渡し
- **推奨型**:
  - `EvaluationContext` struct が 3 つのバッファを所有し、`evaluate(&mut self, planner, schedules, temperature, t0) -> f64` を提供する
  - SA ループは `EvaluationContext::new(capacity)` で一度確保し、ループ内で再利用する
- **修正の重み**: 中
- **該当箇所**:
  - `crates/takusu-core/src/evaluate.rs:135-156`（関数シグネチャ）
  - `crates/takusu-core/src/anneal.rs:1260-1268`（呼び出し元でのバッファ確保）
  - `crates/takusu-core/src/anneal.rs:1297-1305`（ループ内での再利用）

---

## 5. `TabuList` のキーが `(usize, i64, i64)` の生タプル（`takusu-core`）

- **問題の要約**: タブーリストのキーが `(task_id, start, duration)` のタプルで、各要素の意味が型から読めない。`start` は `Point`、`duration` は `Slots` に対応するはずだが、生 `i64` のため取り違えが検出できない。
- **現在の型**: `(usize, i64, i64)`
- **推奨型**:
  ```rust
  #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
  struct TabuKey {
      task_id: usize,
      start: Point,
      duration: Slots,
  }
  ```
- **修正の重み**: 小
- **該当箇所**:
  - `crates/takusu-core/src/anneal.rs:63-92`（`TabuList` の `entries` / `set`）

---

## 6. `Solver` 選択が `match` のハードコードで拡張できない（`takusu-core`）

- **問題の要約**: `solve` / `solve_partial` が `match planner.solver { Sa => ..., Priority => ..., Auto => ... }` で分岐している。新しい solver を追加するには enum と全分岐を書き換える必要があり、外部から solver を差し込むこともできない。`Auto` のフォールバックロジックも関数内に埋め込まれている。
- **現在の型**: `match` on `Solver` enum
- **推奨型**:
  - `Solver` trait を定義し、`fn solve(&self, planner: &Planner, pinned: &[TaskPlacement]) -> Plan` を持たせる
  - `SaSolver` / `PrioritySolver` / `AutoSolver` をそれぞれ impl する
  - `Planner` は `Box<dyn Solver>` または `Arc<dyn Solver>` を持つ
- **修正の重み**: 中
- **該当箇所**:
  - `crates/takusu-core/src/solver.rs:27-33`（`solve`）
  - `crates/takusu-core/src/solver.rs:46-53`（`solve_partial`）
  - `crates/takusu-core/src/solver.rs:155-182`（`Auto` のフォールバック）

---

## ~~7. `RepairMode` の 7 バリアントが巨大 `match` で分岐している（`takusu-core`）~~ FIXED

- **問題の要約**: `decoder.rs` の `RepairMode` は 7 バリアント（`EarliestFit` / `LatestFit` / `DeadlineFit` 等）を持ち、デコーダが `match mode { ... }` で各配置戦略の関数に分岐している。各戦略はシグネチャが同じだが trait で抽象化されておらず、戦略の追加は `match` アームの追加を意味する。
- **現在の型**: `match` on `RepairMode` enum
- **推奨型**:
  - `PlacementStrategy` trait を定義し、`place_task(&self, planner, schedules, input, task_id, index, dependents) -> (Point, Point, Option<PlacementFailure>)` を持たせる
  - `RepairMode` の各バリアントを `PlacementStrategy` impl に変換する
- **修正の重み**: 中
- **該当箇所**:
  - `crates/takusu-core/src/decoder.rs:15-28`（`RepairMode` enum）
  - `crates/takusu-core/src/decoder.rs:647-668`（`match` 分岐）
  - `crates/takusu-core/src/decoder.rs:288-421`（各配置戦略関数）
- **修正**: `PlacementStrategy` trait (`select_and_place -> (usize, Point, Point, Option<PlacementFailure>)`) を定義し、7 つの unit struct (`EarliestStrategy` 等) で実装。`RepairMode::strategy()` が `&'static dyn PlacementStrategy` を返し、`decode` のループ本体は単一の `select_and_place` + `record_placement` に統一。戦略の追加は `PlacementStrategy` impl の追加のみで済む。

---

## 8. `placement.rs` が thread-local の `RefCell` バッファを使っている（`takusu-core`）

- **問題の要約**: `placement.rs` が `thread_local! { static DAY_INTERVALS: RefCell<Vec<...>> }` のように thread-local の可変バッファを使っている。パフォーマンス上の理由はあるが、Rust のイディオムから外れており、バッファの汚染がテストに影響するリスクがある。呼び出し間で暗黙に状態が共有されるため、関数の振る舞いが引数だけから決まらない。
- **現在の型**: `thread_local! { static X: RefCell<Vec<...>> }`
- **推奨型**:
  - scratch バッファを引数または `PlacementContext` struct で明示的に渡す（`evaluate.rs` の `evaluate_with_scratch` と同じ方針）
  - または、バッファのクリアタイミングを明示する API にする
- **修正の重み**: 中
- **該当箇所**:
  - `crates/takusu-core/src/placement.rs:53-65`（thread-local 定義）
  - `crates/takusu-core/src/placement.rs:148-186`（`day_load_with_candidate` での利用）

---

## 9. `NormalDist` / `SleepConfig` / `WorkloadConfig` の不変条件が型で保証されていない（`takusu-core`）

- **問題の要約**: `NormalDist { avg: u64, sigma: u64 }` は `sigma >= 0` が型で保証されるが、`avg` と `sigma` の関係（例: `sigma` が極端に大きいと無意味）は未検証。`SleepConfig` は `enabled: bool` と `start` / `end` を持つが、`enabled == true` のとき `end > start` であることがコンストラクタで検証されていない。`WorkloadConfig` は `comfortable_slots_per_day <= maximum_slots_per_day` が未検証で、`evaluate.rs` がこの不変条件を暗黙に前提としている。
- **現在の型**: `pub` フィールドの struct（コンストラクタが検証しない）
- **推奨型**:
  - フィールドを `pub` から private にし、`new()` で不変条件を検証する
  - `SleepConfig` は `enum SleepMode { Disabled, Enabled { day_start, start, end } }` にすると `enabled` フラグとフィールドの存在が一致する
  - `WorkloadConfig::new` で `comfortable <= maximum` を検証する
- **修正の重み**: 小
- **該当箇所**:
  - `crates/takusu-core/src/lib.rs:136-140`（`NormalDist`）
  - `crates/takusu-core/src/lib.rs:164-194`（`SleepConfig`）
  - `crates/takusu-core/src/lib.rs:246-271`（`WorkloadConfig`）
  - `crates/takusu-core/src/evaluate.rs:509-512`（`comfortable <= maximum` を前提とするコード）

---

## 10. `Planner` の設定が 8 個のフィールド + 個別 setter で初期化が冗長（`takusu-core`）

- **問題の要約**: `Planner` は `solver` / `time_budget` / `seed` / `warm_start` / `workload` / `sleep` / `now` / `per` の 8 フィールドを持ち、`Planner::new(now, sleep)` で生成した後に `with_solver` / `with_time_budget` / `with_seed` / `with_warm_start` / `with_workload` を個別に呼ぶ。設定の追加や必須/任意の区別が型で表現されておらず、setter の呼び忘れがコンパイル時に検出できない。
- **現在の型**: `Planner::new` + 個別 `with_*` setter
- **推奨型**:
  - `PlannerConfig` struct を定義し、`Planner::new(config: PlannerConfig)` で一度に渡す
  - または builder pattern で `PlannerBuilder` を提供し、必須フィールドを型レベルで強制する
- **修正の重み**: 中
- **該当箇所**:
  - `crates/takusu-core/src/lib.rs:438-460`（`Planner` struct）
  - `crates/takusu-core/src/lib.rs:467-480`（`Planner::new`）
  - `crates/takusu-core/src/lib.rs:560-578`（setter 群）

---

## 11. `app.rs` が 3745 行の god module で責務が混在している（`takusu-local-lib`）

- **問題の要約**: `app.rs` はバリデーション関数（`validate_*` / `parse_*` 16 個）、データ変換、入出力 struct 定義、`TakusuApp` の全メソッド、テストを 1 ファイルに詰め込んでいる。スケジュール生成、ハビット同期、依存関係解析、進捗管理がすべて `TakusuApp` のメソッドとして並んでおり、関心ごとごとのモジュール分割がない。
- **現在の型**: 単一ファイル 3745 行
- **推奨型**:
  - `validators.rs`（`validate_*` / `parse_*` 関数群）
  - `schedule.rs`（スケジュール生成・プレビュー・再スケジュール）
  - `habit_sync.rs`（ハビットタスク同期）
  - `dependency.rs`（依存グラフ解析）
  - `app.rs` は `TakusuApp` の高レベルオーケストレーションのみ
- **修正の重み**: 大
- **該当箇所**:
  - `crates/takusu-local-lib/src/app.rs:1-3745`（全体）

---

## 12. `validate_*` / `parse_*` 関数が struct から切り離されている（`takusu-local-lib`）

- **問題の要約**: `app.rs` に `validate_minutes` / `validate_title` / `validate_recurrence` / `validate_skill` / `validate_memory` / `validate_steps` / `validate_hhmm` / `validate_timezone` / `validate_task_datetimes` / `validate_scheduled_span_dates` / `parse_sleep` / `parse_workload` 等、16 個の standalone 関数が並んでいる。それぞれが特定の入力 struct に対するバリデーションだが、struct に紐付いていないため、呼び出し忘れが起きてもコンパイラが検出できない。
- **現在の型**: `fn validate_xxx(input: &XxxInput) -> Result<(), AppError>`
- **推奨型**:
  - `Validate` trait を定義し、`fn validate(&self) -> Result<(), AppError>` を持たせる
  - `CreateSkill` / `CreateMemory` / `HabitStepInput` / `CreateTask` 等の入力 struct に `Validate` を impl する
  - `parse_sleep` / `parse_workload` は `FromStr` または専用 newtype のコンストラクタに移す
- **修正の重み**: 中
- **該当箇所**:
  - `crates/takusu-local-lib/src/app.rs:32-47`（`parse_hhmm`）
  - `crates/takusu-local-lib/src/app.rs:52-83`（`validate_minutes`）
  - `crates/takusu-local-lib/src/app.rs:90-94`（`validate_title`）
  - `crates/takusu-local-lib/src/app.rs:100-153`（`parse_recurrence` / `validate_recurrence` / `validate_skill`）
  - `crates/takusu-local-lib/src/app.rs:156-237`（`validate_memory` / `validate_hhmm` / `validate_steps`）
  - `crates/takusu-local-lib/src/app.rs:264-305`（`validate_timezone` / `parse_settings_timezone` / `validate_task_datetimes`）
  - `crates/takusu-local-lib/src/app.rs:425-533`（`validate_scheduled_span_dates` / `parse_calendar_date` / `parse_sleep` / `parse_workload`）

---

## 13. `RescheduleInput.mode` / `SchedulePreviewInput.mode` が `String`（`takusu-local-lib`）

- **問題の要約**: スケジュール再生成の `mode` が `"full"` / `"tasks"` / `"range"` の文字列で、`match input.mode.as_str()` で分岐している。`sleep` も `"recommended"` / `"disabled"` / `"HH:MM-HH:MM"` の文字列で、`parse_sleep` が都度解釈している。
- **現在の型**: `String`
- **推奨型**:
  - `ScheduleMode` enum（`Full` / `Tasks` / `Range`）
  - `SleepInput` enum（`Recommended` / `Disabled` / `Custom { start: TimeOfDay, end: TimeOfDay }`）
- **修正の重み**: 小
- **該当箇所**:
  - `crates/takusu-local-lib/src/app.rs:724, 729, 734, 739, 744`（入力 struct の `mode` / `sleep` フィールド）
  - `crates/takusu-local-lib/src/app.rs:472-495`（`parse_sleep`）
  - `crates/takusu-local-lib/src/app.rs:2210, 2338`（`match input.mode.as_str()`）

---

## 14. `TaskQuery.status` が `Option<String>` で `"overdue"` を特別扱いしている（`takusu-local-lib` / `takusu-storage`）

- **問題の要約**: `TaskQuery.status` が `Option<String>` で、`storage_sqlite.rs` が `if v == "overdue"` だけ特別ルートを持ち、それ以外は `WHERE status = ?` に流す。`"overdue"` は `TaskStatus` enum に存在しない疑似状態で、文字列だからこそ混入できる。
- **現在の型**: `Option<String>`
- **推奨型**:
  - `Option<TaskStatus>` にし、`overdue` は別フィールド（`include_overdue: bool` または `overdue_only: bool`）で表現する
- **修正の重み**: 小
- **該当箇所**:
  - `crates/takusu-storage/src/model.rs:184`（`TaskQuery.status`）
  - `crates/takusu-local-lib/src/storage_sqlite.rs:480-488`（`"overdue"` 特別扱い）

---

## 15. `resolve_task_id` が SQLite / Workers の両方に 60 行ずつ重複している（`takusu-local-lib`）

- **問題の要約**: `SqliteStorage` と `WorkersStorage` がそれぞれ `resolve_task_id` を持ち、`#` prefix / `hN#M` 形式 / numeric display_id / UUID prefix / full UUID の 5 パターンを同じ順序で処理している。ロジックが完全に重複しているが、`Storage` trait にも共通モジュールにも抽出されていない。
- **現在の型**: 各 backend の private `fn resolve_task_id`
- **推奨型**:
  - 共通モジュール（例: `task_id.rs`）に `resolve_task_id(input: &str, lookup: impl Fn(&str) -> Option<String>) -> Result<String, AppError>` を定義する
  - 各 backend は lookup クロージャ（display_id → UUID / prefix → UUID）だけを提供する
- **修正の重み**: 中
- **該当箇所**:
  - `crates/takusu-local-lib/src/storage_sqlite.rs:2895-2956`
  - `crates/takusu-local-lib/src/storage_workers.rs:912-973`

---

## 16. `WorkersStorage` の HTTP リクエストメソッドが 4 + 2 個に分かれている（`takusu-local-lib`）

- **問題の要約**: `WorkersStorage` が `request` / `request_body` / `request_body_empty` / `request_no_body` と、それぞれの `_idempotent` 版の計 6 メソッドを持つ。違いは body の有無とレスポンスを JSON で parse するか空にするかだけで、リトライ・認証ヘッダ・エラーハンドリングの骨組みは同じ。
- **現在の型**: 個別の `fn request_*`
- **推奨型**:
  - `RequestBody` enum（`None` / `Json(Value)`）と `ResponseMode` enum（`Json` / `Empty`）を取り、単一の `send_request<T>` に集約する
- **修正の重み**: 中
- **該当箇所**:
  - `crates/takusu-local-lib/src/storage_workers.rs:75-165`（`request` 系 4 メソッド）
  - `crates/takusu-local-lib/src/storage_workers.rs:854-910`（`_idempotent` 系 2 メソッド）

---

## 17. `AppError` の全バリアントが `String` を持つ（`takusu-local-lib`）

- **問題の要約**: `AppError` は `NotFound(String)` / `BadRequest(String)` / `Conflict { message: String }` / `Internal(String)` の 4 バリアントで、すべて `String` を持つ。呼び出し側がエラーの種類をプログラム的に判定するには文字列を parse するしかなく、`match` しても具体的な原因は取れない。
- **現在の型**: `String` のみ
- **推奨型**:
  - `BadRequest` と `Conflict` に構造化バリアントを追加する（例: `BadRequest(InvalidMinutes)` / `BadRequest(CycleDetected)` / `BadRequest(Other(String))`）
  - `Other(String)` は構造化できないエラーのフォールバック
- **修正の重み**: 中
- **該当箇所**:
  - `crates/takusu-local-lib/src/error.rs:4-14`

---

## 18. `execute_proposed_change` が `(TargetKind, ChangeOperation)` の 20+ アーム `match` で巨大（`takusu-agent`）

- **問題の要約**: `lib.rs` の `execute_proposed_change` が `(TargetKind::Task, ChangeOperation::Create)` / `(TargetKind::Task, ChangeOperation::Update)` / ... の 20 以上のアームを `match` で持ち、各アームが `serde_json::from_value::<CreateTask>(args.clone())` のように手動で `Value` から型付き引数に変換している。新しい操作を追加するにはこの中央関数の `match` を書き換える必要があり、拡張性が閉じている。
- **現在の型**: `match (change.target.kind, change.operation) { ... }`（20+ アーム）
- **推奨型**:
  - `ChangeExecutor` trait を定義し、`fn execute(&self, client, target_id, args: Value, operation_id) -> Result<ExecutionResult, AgentError>` を持たせる
  - `(TargetKind, ChangeOperation)` ごとに `ChangeExecutor` impl を作り、レジストリ（`HashMap<(TargetKind, ChangeOperation), Box<dyn ChangeExecutor>)`）で dispatch する
  - 新しい操作は `ChangeExecutor` impl を追加するだけで済む
- **修正の重み**: 大
- **該当箇所**:
  - `crates/takusu-agent/src/lib.rs:1914-2250`（`match` 本体）
  - `crates/takusu-agent/src/lib.rs:1919-2224`（各アームの `serde_json::from_value` 呼び出し 16 箇所）

---

## 19. `Tool::call` が `Value` を受け取り、`TypedTool` への橋渡しが blanket impl でしかない（`takusu-agent`）

- **問題の要約**: `Tool` trait は object-safe のために `async fn call(&self, args: Value)` を持ち、`TypedTool` が associated type `Params` を提供する構造になっている。しかし `execute_proposed_change` は `Tool::call` を経由せず `Value` を直接 `serde_json::from_value` で変換しており、`TypedTool` の型安全性が change 実行パスに届いていない。`ProposedChange.before` / `after` / `arguments` も `Option<Value>` のままで、change の種類ごとの引数型が表現されていない。
- **現在の型**: `Value` / `Option<Value>`
- **推奨型**:
  - `ChangeHandler` trait を定義し、`type Args: DeserializeOwned` を持たせる
  - `ProposedChange` をジェネリックにするか、`arguments` を `ChangeHandler` の `Args` に紐付ける
  - `execute_proposed_change` は `ChangeHandler` 経由で `Args` として受け取る
- **修正の重み**: 大
- **該当箇所**:
  - `crates/takusu-agent/src/tool.rs:369-374`（`Tool` trait）
  - `crates/takusu-agent/src/tool.rs:415-458`（`TypedTool` trait）
  - `crates/takusu-agent/src/tool.rs:278-282`（`ProposedChange.before` / `after` / `arguments` が `Option<Value>`）
  - `crates/takusu-agent/src/lib.rs:1919-2224`（`serde_json::from_value` の直接呼び出し 16 箇所）

---

## 20. ツール出力が `json!` マクロで手組みされている（`takusu-agent`）

- **問題の要約**: `skill_json` / `memory_json` / `task_json` / `progress_output` 等のツール出力関数が `json!({ "slug": ..., "name": ..., ... })` で `Value` を組み立てている。フィールド名の typo がコンパイル時に検出されず、出力 struct の形がコードから読めない。
- **現在の型**: `serde_json::Value`（`json!` マクロで構築）
- **推奨型**:
  - 各ツールに `#[derive(Serialize)] struct SkillResponse { slug, name, ... }` のような出力 struct を定義する
  - `From<SkillRow> for SkillResponse` を実装し、`json!` を排除する
- **修正の重み**: 中
- **該当箇所**:
  - `crates/takusu-agent/src/tools/skills.rs:199-207`（`skill_json`）
  - `crates/takusu-agent/src/tools/memory.rs:29-43`（`memory_json`）
  - `crates/takusu-agent/src/tools/takusu/common.rs:382-422`（`task_json`）
  - `crates/takusu-agent/src/tools/progress.rs:176-204`（`progress_output`）

---

## 21. ツールの JSON Schema が `json!` マクロで手書きされている（`takusu-agent`）

- **問題の要約**: `MutationKind::schema()` が `json!({ "type": "object", "properties": { ... }, "required": [...] })` でスキーマを返している。`TypedTool` の `Params` から `schemars` で自動生成する経路があるにもかかわらず、mutation 系ツールは手書きスキーマを使い続けており、`Params` struct との整合性が保証されない。
- **現在の型**: `serde_json::Value`（`json!` マクロ）
- **推奨型**:
  - `schemars` を workspace 依存に追加し、`TypedTool::Params` に `JsonSchema` を要求する
  - `parameters_schema` は `schemars::schema_for!(T::Params)` で自動生成する
  - 手書きスキーマを削除する
- **修正の重み**: 中
- **該当箇所**:
  - `crates/takusu-agent/src/tools/takusu/mutation.rs:52-72`（`habit_step_schema`）
  - `crates/takusu-agent/src/tools/takusu/mutation.rs:185-326`（`MutationKind::schema`）

---

## 22. ツール名・操作名が文字列リテラルで散らばっている（`takusu-agent`）

- **問題の要約**: ツール名（`"create_task"` / `"update_task"` / `"delete_task"` 等）と操作名が各ツールの `name()` メソッドで文字列リテラルとして返されている。レジストリの lookup も文字列キーで行われるため、ツール名の typo がコンパイル時に検出されない。
- **現在の型**: `&'static str` リテラル
- **推奨型**:
  - `ToolName` enum（`CreateTask` / `UpdateTask` / ...）を定義し、`as_str() -> &'static str` を持たせる
  - `ToolRegistry` のキーを `ToolName` にする（または `&'static str` だが `ToolName::as_str()` 経由でのみ構築）
- **修正の重み**: 小
- **該当箇所**:
  - `crates/takusu-agent/src/tools/takusu/mutation.rs:87-127`（`MutationKind::name` / `description`）
  - `crates/takusu-agent/src/tool.rs`（`ToolRegistry` の文字列キー lookup）

---

## 23. ツール登録が中央関数で手動登録（`takusu-agent`）

- **問題の要約**: `tools/takusu/mod.rs` の `register_tools` が各ツールを `registry.register(Box::new(...))` で個別に登録している。新しいツールを追加するにはこの中央関数を書き換える必要があり、モジュールごとの自己登録ができない。ツールの追加忘れもコンパイラが検出しない。
- **現在の型**: 中央 `fn register_tools(registry: &mut ToolRegistry)`
- **推奨型**:
  - `ToolModule` trait を定義し、`fn register(&self, registry: &mut ToolRegistry)` を持たせる
  - `inventory` crate 等で自己登録するか、`build.rs` でモジュール一覧を生成する
  - または、各モジュールの `register_tools` をマクロで自動収集する
- **修正の重み**: 中
- **該当箇所**:
  - `crates/takusu-agent/src/tools/takusu/mod.rs:27-53`（中央 `register_tools`）
  - `crates/takusu-agent/src/tools/skills.rs:101-112`（`register_tools`）
  - `crates/takusu-agent/src/tools/memory.rs:84-96`（`register_tools`）

---

## 24. `build_stt` / `build_tts` が `config.backend.as_str()` で文字列マッチ（`takusu-agent`）

- **問題の要約**: `audio.rs` の `build_stt` / `build_tts` が `match config.backend.as_str() { "sherpa" => ..., _ => default }` のように文字列マッチで backend を選択している。`audio_config.rs` の `SttConfig.backend` / `TtsConfig.backend` が `String` で、未知の値がフォールバックするだけでコンパイル時に網羅性が保証されない。`model` / `provider` / `voice_id` / `language` も `String` のままで、`takusu-audio` 側に既存の `TtsBackend` / `SherpaOnnxModel` enum があるのに使われていない。
- **現在の型**: `String`（`backend` / `model` / `provider` / `voice_id` / `language`）
- **推奨型**:
  - `SttConfig.backend` を `SttBackend` enum にする（`takusu-audio` 側で新規定義）
  - `SttConfig.model` を既存の `SherpaOnnxModel` enum にする
  - `SttConfig.provider` を `ExecutionProvider` enum（`Cpu` / `Cuda` / `CoreMl`）にする
  - `TtsConfig.backend` を既存の `TtsBackend` enum にする
  - `voice_id` / `language` を newtype（`VoiceId` / `LanguageCode`）にする
  - `build_stt` / `build_tts` の `match` が網羅的になり、backend 追加時にコンパイルエラーで検知できる
- **修正の重み**: 中
- **該当箇所**:
  - `crates/takusu-agent/src/audio_config.rs:14, 16, 20, 26, 72, 78, 80`（`String` フィールド群）
  - `crates/takusu-agent/src/audio.rs:227-267`（`build_stt` の文字列マッチ）
  - `crates/takusu-agent/src/audio.rs:272-297`（`build_tts` の文字列マッチ）

---

## 25. `SttBackend` enum が存在せず、`TtsBackend` と非対称（`takusu-audio`）

- **問題の要約**: `takusu-audio` に `TtsBackend` enum（`Cartesia` / `Android`）が存在するが、対応する `SttBackend` enum がない。そのため CLI が Sherpa 固有のパラメータを `Transcribe` / `Listen` コマンドにハードコードし、STT backend の追加が困難になっている。
- **現在の型**: `TtsBackend` enum のみ存在、STT 側は文字列
- **推奨型**:
  - `SttBackend` enum（`Sherpa` / 将来的なバリアント）を `stt.rs` に定義する
  - `SttConfig` struct を `takusu-audio` 側に定義し、`SttBackend` / `SherpaOnnxModel` / `ExecutionProvider` を持たせる
  - CLI は `SttConfig` を経由して backend に依存しないパラメータ渡しをする
- **修正の重み**: 大
- **該当箇所**:
  - `crates/takusu-audio/src/tts.rs:16-21`（既存 `TtsBackend`）
  - `crates/takusu-audio/src/stt.rs`（`SttBackend` なし）
  - `crates/takusu-audio-cli/src/main.rs:40-102`（CLI への Sherpa パラメータのハードコード）

---

## 26. Cartesia の `container` / `encoding` / `emotion` が `String`（`takusu-audio`）

- **問題の要約**: `CartesiaOutputFormat.container` / `encoding` が `String` で、`output_format_for_request` が文字列マッチで分岐している。`CartesiaGenerationConfig.emotion` も `Option<String>` で、Cartesia API が定める固定値（`neutral` / `happy` / `sad` / `angry`）以外が渡せる。
- **現在の型**: `String` / `Option<String>`
- **推奨型**:
  - `CartesiaContainer` enum（`Wav` / `Raw` / `Mp3`）
  - `CartesiaEncoding` enum（`PcmS16Le` / `PcmF32Le` / `PcmMulaw` / `PcmAlaw`）
  - `CartesiaEmotion` enum（`Neutral` / `Happy` / `Sad` / `Angry`）
- **修正の重み**: 小
- **該当箇所**:
  - `crates/takusu-audio/src/cartesia.rs:20-22`（`CartesiaOutputFormat.container` / `encoding`）
  - `crates/takusu-audio/src/cartesia.rs:79`（`emotion`）
  - `crates/takusu-audio/src/cartesia.rs:287-296`（`output_format_for_request` の文字列マッチ）

---

## 27. API key / URL が生 `String` で流出リスクがある（`takusu-audio`）

- **問題の要約**: `TtsConfig.api_key` / `url`、`CartesiaSonicConfig.api_key` / `url` が生 `String` で、`Debug` 出力やログにそのまま表示される。URL も `String` のため、不正な形式の URL が実行時まで検出されない。
- **現在の型**: `String` / `Option<String>`
- **推奨型**:
  - `ApiKey` newtype（`Debug` 実装でマスク表示）
  - `url::Url` または `EndpointUrl` newtype（構築時に形式検証）
- **修正の重み**: 小
- **該当箇所**:
  - `crates/takusu-audio/src/tts.rs:59-60`（`TtsConfig.url` / `api_key`）
  - `crates/takusu-audio/src/cartesia.rs:85-89`（`CartesiaSonicConfig.api_key` / `url`）

---

## 28. `SttError` が全バリアント `String` を持つ（`takusu-audio`）

- **問題の要よ**: `SttError` が `Connection(String)` / `Server(String)` / `Other(String)` で、すべて文字列を保持する。HTTP ステータスコードや原因エラー型が失われるため、呼び出し側がエラーの種類をプログラム的に判定できない。
- **現在の型**: `String` のみ
- **推奨型**:
  - `Api { status: u16, code: String, message: String }` バリアントを追加する
  - `reqwest::Error` 等の原因エラーを `#[source]` で保持する
  - `Other(String)` は構造化できないエラーのフォールバック
- **修正の重み**: 小
- **該当箇所**:
  - `crates/takusu-audio/src/stt.rs:7-14`

---

## 29. サンプルレート `16000` が 6 箇所以上にハードコード（`takusu-audio` / `takusu-audio-cli`）

- **問題の要約**: Sherpa-ONNX が要求する 16kHz が `record.rs` / `hush.rs` / `main.rs` の 6 箇所以上にリテラル `16000` で埋め込まれている。`i16` → `f32` 正規化の `32768.0` もマジックナンバー。将来別のサンプルレートをサポートする場合、全箇所を書き換える必要がある。
- **現在の型**: リテラル `16000` / `32768.0`
- **推奨型**:
  - `const SHERPA_SAMPLE_RATE: u32 = 16000;` を共通モジュールに定義する
  - `const I16_MAX_F32: f32 = 32768.0;` を定義する
  - `RecordConfig` に `target_sample_rate: Option<u32>` を追加し、デフォルトは `SHERPA_SAMPLE_RATE` にする
- **修正の重み**: 小
- **該当箇所**:
  - `crates/takusu-audio/src/record.rs:90, 134-135`
  - `crates/takusu-audio/src/hush.rs:59`
  - `crates/takusu-audio-cli/src/main.rs:152, 213, 283, 475, 484`

---

## 30. CLI が WAV I/O とリサンプリングをライブラリから複製している（`takusu-audio-cli`）

- **問題の要約**: `takusu-audio-cli` が `read_wav` / `write_wav` / `to_mono` / リサンプリングを独自実装している。`takusu-audio/src/record.rs` にも `mix_to_mono` / `resample` / `normalize` があり、ロジックが重複している。ライブラリ側を修正しても CLI 側に伝播しない。
- **現在の型**: CLI の private `fn read_wav` / `write_wav` / `to_mono`
- **推奨型**:
  - `takusu-audio` に `pub fn read_wav(path) -> Result<Vec<f32>, AudioError>` / `pub fn write_wav(path, samples, sample_rate) -> Result<(), AudioError>` を追加する
  - `mix_to_mono` / `resample` / `normalize` を pub にする
  - CLI はライブラリの関数を呼ぶようにする
- **修正の重み**: 中
- **該当箇所**:
  - `crates/takusu-audio-cli/src/main.rs:404-504`（`read_wav` / `write_wav` / `to_mono`）
  - `crates/takusu-audio/src/record.rs:143-187`（`mix_to_mono` / `resample` / `normalize`）

---

## 31. クレートのレイヤー違反とドメイン型の 3 crate 重複（`takusu-storage` / `takusu-client` / `takusu-worker`）

> **関連 issue**: [takusu-dev/takusu#1163](https://github.com/takusu-dev/takusu/issues/1163)「依存関係のレイヤー整理」

### 31.1 レイヤー違反

- **問題の要約**: Issue #1163 の方針「Ln は L(n-1) 以下にしか依存できない」に対し、現在の依存グラフに矛盾がある。とくに `takusu-util`（L1 想定）が `takusu-search` に依存し、`takusu-storage`（L1 想定、`contract` に rename）が `takusu-util` に依存している。L1 内での依存は方針違反である。
- **現在の依存グラフ**:
  ```
  L0: takusu-search, takusu-ical, takusu-audio（workspace dep なし）
  L1: takusu-util (→ search), takusu-storage (→ util), takusu-core (→ util), takusu-client (→ util)
  L2: takusu-habit (→ core, util), google-cal (→ client)
  L3: takusu-agent (→ audio, client, core, habit, util), takusu-local-lib (→ core, ical, habit, storage, util, google-cal)
  L4: takusu-local (→ local-lib, util, storage), takusu-tui (→ local-lib, storage, habit, util)
  L5: takusu-web (→ local, local-lib, storage), takusu-cli (→ ほぼ全部), takusu-android (→ local, agent, audio, client, local-lib, storage)
  特殊: takusu-worker (wasm, → util のみ), takusu-audio-cli (→ audio)
  ```
- **矛盾**:
  1. `takusu-util` → `takusu-search`: util が L1 のはずが別 crate に依存。`takusu-search` を util に統合するか、search も L1 に含める必要がある。
  2. `takusu-storage` → `takusu-util`: issue は両者を L1（contract）に置くことを想定。L1 内依存を許さないなら、`util` を L0、`contract`（旧 storage）を L1 に分けるべき。
- **推奨されるレイヤー構成**:
  ```
  L0 (foundation):
    takusu-util（takusu-search を統合）

  L1 (contract):
    takusu-storage（→ rename: takusu-contract, depends on L0）

  L2 (primitive):
    takusu-core   → L0
    takusu-ical   → L0
    takusu-audio  → 依存なし（独立）
    takusu-client → L0
    takusu-worker → L0: util, L1: contract（D1 ストレージ実装）

  L3 (domain):
    takusu-habit  → L2: core, L0: util
    google-cal    → L2: client

  L4 (integration):
    takusu-agent     → L2/L3
    takusu-local-lib → L1/L2/L3

  L5 (app):
    takusu-local, takusu-tui, takusu-web, takusu-cli, takusu-android, takusu-audio-cli
  ```
- **各レイヤーの根拠**:
  - L0: 純粋なユーティリティ・型定義。workspace crate に依存しない
  - L1: ドメイン型と `Storage` trait の契約。L0 のみに依存
  - L2: 計算・通信のプリミティブ。L0/L1 のみに依存。`takusu-worker` は D1 ストレージ実装としてここに配置
  - L3: ドメインロジック。L2 以下に依存
  - L4: 統合層。複数の L2/L3 を組み合わせる
  - L5: アプリケーション。L4 以下を束ねる
- **解決すべき点**:
  - `takusu-search` は `takusu-util` に統合する（独立 crate にする理由が薄い）
  - `takusu-storage` は `takusu-contract` に rename し、ドメイン型と `Storage` trait の定義のみを残す。実装（SQLite / Workers）は L4 の `takusu-local-lib` と L2 の `takusu-worker` が持つ
  - `takusu-worker` は wasm 制約があるが、`takusu-contract`（L1）が `sqlx` を optional feature で持つなら L1 に依存しても wasm ビルドに影響しない
  - `takusu-cli` → `takusu-tui` の依存は L5 内のため方針違反だが許容範囲とする
- **修正の重み**: 大

### 31.2 workspace `Cargo.toml` の依存集中

- **問題の要約**: すべての外部依存が workspace root の `Cargo.toml` に集中している。`axum` / `sqlx` / `tower-http` / `sentry` / `config` / `schemars` / `petgraph` / `rust-embed` 等の少数 crate しか使わない依存も workspace に置かれたままで、メンテ性を損ねている。
- **推奨**:
  - 広く使われるもの（`thiserror` / `serde` / `serde_json` / `jiff` / `uuid` / `sha2` / `tracing` / `async-trait` / `rand`）は workspace 残置
  - 少数 crate しか使わないもの（`axum` / `sqlx` / `tower-http` / `sentry` / `config` / `schemars` / `petgraph` / `rust-embed` / `mime_guess` / `futures-util` / `toml` / `tracing-subscriber` / `web-time` 等）は個別 crate の `Cargo.toml` に移す
  - `sqlx` は feature が crate ごとに異なるため、とくに個別化すべき
- **修正の重み**: 中

### 31.3 ドメイン型の 3 crate 重複

- **問題の要約**: `TaskRow` / `CreateTask` / `UpdateTask` / `HabitRow` / `CreateHabit` / `ScheduleRow` / `ScheduleEntry` / `TokenRow` / `SettingsRow` / `SkillRow` / `MemoryRow` / `GoogleCalSettingsRow` / `ProgressEventRow` / `RecordProgress` 等のドメイン型が、`takusu-storage/src/model.rs` / `takusu-client/src/lib.rs` / `takusu-worker/src/models.rs` の 3 ファイルでほぼ同一の定義が重複している。フィールド追加時に 3 箇所を手動で同期する必要があり、ずれると実行時まで気づかない。
- **現在の型**: 3 crate に重複する `struct` 定義
- **推奨型**:
  - L1 の `takusu-contract`（旧 `takusu-storage`）にドメイン型を集約する
  - `#[derive(sqlx::FromRow)]` は feature flag で付与し、wasm 版は derive しない
  - `takusu-client` / `takusu-worker` は共有型を re-export する
  - wasm ターゲットの `takusu-worker` でもビルドできるよう、`takusu-contract` の依存を `takusu-util`（L0）のみに保つ
- **修正の重み**: 大
- **該当箇所**:
  - `crates/takusu-storage/src/model.rs:10-180`（`TaskRow` / `CreateTask` / `UpdateTask`）
  - `crates/takusu-client/src/lib.rs:1236-1357`（同上）
  - `crates/takusu-worker/src/models.rs:11-133`（同上）
  - `crates/takusu-storage/src/model.rs:195-304`（`HabitRow` / `CreateHabit` / `UpdateHabit`）
  - `crates/takusu-client/src/lib.rs:1372-1451`（同上）
  - `crates/takusu-worker/src/models.rs:136-215`（同上）
  - `crates/takusu-storage/src/model.rs:461-480`（`ScheduleRow` / `ScheduleEntry`）
  - `crates/takusu-client/src/lib.rs:1624-1636`（同上）
  - `crates/takusu-worker/src/models.rs:314-333`（同上）
  - 他、`SettingsRow` / `TokenRow` / `MemoryRow` / `SkillRow` / `GoogleCalSettingsRow` / `ProgressEventRow` も同様

---

## 32. `depends` / `depends_on` / `schedule` が JSON 文字列で保存されている（`takusu-storage` / `takusu-worker`）

- **問題の要約**: `TaskRow.depends` / `HabitStepRow.depends_on` が `Vec<String>` を JSON 文字列化した `String` で保存されている。`ScheduleRow.schedule` も `Vec<ScheduleEntry>` の JSON 文字列。読み出し側が毎回 `serde_json::from_str` で parse し、書き込み側が `serde_json::to_string` で serialize する。parse 失敗が実行時まで検出されず、型安全性がない。
- **現在の型**: `String`（JSON 文字列）
- **推奨型**:
  - `Vec<String>` / `Vec<ScheduleEntry>` を直接フィールドにし、`sqlx::types::Json` または serde アダプタで DB との往復を処理する
  - または `DependencyList(Vec<TaskId>)` / `ScheduleData(Vec<ScheduleEntry>)` newtype を定義し、serde アダプタで JSON 文字列との互換性を保つ
- **修正の重み**: 中
- **該当箇所**:
  - `crates/takusu-storage/src/model.rs:20`（`TaskRow.depends`）
  - `crates/takusu-storage/src/model.rs:354`（`HabitStepRow.depends_on`）
  - `crates/takusu-storage/src/model.rs:465`（`ScheduleRow.schedule`）
  - `crates/takusu-worker/src/models.rs:21, 259, 318`（同上）
  - `crates/takusu-worker/src/handlers/tasks.rs:184, 485`（`serde_json::to_string`）
  - `crates/takusu-worker/src/handlers/schedule.rs:26, 148`（同上）

---

## 33. `SimilarTaskRow.similarity` が `"dice:0.xxx"` の文字列（`takusu-storage` / `takusu-worker`）

- **問題の要約**: 類似度スコアが `"dice:0.85"` のような文字列で保存されている。数値比較や集計ができず、metric 名とスコアが一つの文字列に詰め込まれているため、取り出す側が split して parse する必要がある。
- **現在の型**: `String`
- **推奨型**:
  ```rust
  struct Similarity { metric: SimilarityMetric, score: f64 }
  enum SimilarityMetric { Dice, /* ... */ }
  ```
- **修正の重み**: 小
- **該当箇所**:
  - `crates/takusu-storage/src/model.rs:689`
  - `crates/takusu-worker/src/models.rs:520`

---

## 34. `MemoryRow.source` が `String` で未検証（`takusu-storage`）

- **問題の要約**: Memory の `source` が `String` で、`"user_confirmed"` / `"llm_proposed"` 等の固定値のはずが型で保証されていない。
- **現在の型**: `String`
- **推奨型**: `MemorySource` enum（`UserConfirmed` / `LlmProposed` / `SystemGenerated`）
- **修正の重み**: 小
- **該当箇所**:
  - `crates/takusu-storage/src/model.rs:619`

---

## 35. Worker の `COALESCE` hack で SQLite と D1 の挙動が不一致（`takusu-worker` / `takusu-local-lib`）

- **問題の要約**: D1（Cloudflare Workers）が prepared statement で `NULL` を bind できないため、`UPDATE tasks SET title=COALESCE(?1, title), ...` のパターンで `None` を bind すると旧値が保持される。一方 SQLite は `description` / `quantity_total` / `quantity_unit` について `CASE WHEN '' THEN NULL` / `CASE WHEN 0 THEN NULL` の workaround を持ち、空文字や 0 で NULL クリアができる。D1 にはこれらの workaround がないため、同じ API を叩いても backend によってクリア可否が変わる。
- **現在の型**: `COALESCE(?, column)` の SQL + 一部 `CASE WHEN` workaround（SQLite のみ）
- **方針**: クリアできるべきなのは `start_at`（`Option<Option<Timestamp>>` で既に両 backend 対応済み）と、SQLite が workaround で対応している `description` / `quantity_total` / `quantity_unit` のみ。これらは SQLite の挙動に D1 をそろえる。それ以外のフィールドはクリア不可でよく、`COALESCE` のままでよい。
- **推奨型**:
  - D1 側の `description` / `quantity_total` / `quantity_unit` に SQLite と同じ `CASE WHEN` workaround を追加し、両 backend の挙動を一致させる
  - `start_at` の `Option<Option<Timestamp>>` + `CASE WHEN` パターンは既に両 backend で一致しているためそのまま
  - それ以外のフィールドは `COALESCE` のままでクリア不可を維持する（仕様）
  - `UpdateBuilder` ヘルパーを定義し、`CASE WHEN` を使うフィールドと `COALESCE` のみのフィールドを型で区別し、handler 間の SQL 重複を排除する
- **修正の重み**: 中
- **該当箇所**:
  - `crates/takusu-worker/src/handlers/tasks.rs:356-363`（D1 側 `COALESCE` UPDATE + `start_at` の `CASE WHEN`）
  - `crates/takusu-worker/src/handlers/habits.rs:112`（D1 側 `COALESCE` UPDATE）
  - `crates/takusu-worker/src/handlers/skills.rs:146`（D1 側 `COALESCE` UPDATE）
  - `crates/takusu-local-lib/src/storage_sqlite.rs:715-737`（SQLite 側 `CASE WHEN` workaround 3 フィールド）

---

## 36. UUID prefix 解決を削除し、UUID はユーザーに露出させない（`takusu-worker` / `takusu-local-lib`）

- **問題の要約**: `resolve_task_id` が 5 パターンの ID 解決を受け付けるうち、UUID prefix（`-` を含まない文字列で全行 fetch + `starts_with`）は `LIKE ? || '%'` または全行 fetch で実装されている。`_` / `%` が含まれるとパターンインジェクションが起き、インデックスも効かない。しかし UUID prefix マッチは「ユーザーが UUID の先頭数文字をコピペした」場合の救済措置であり、主役の UX は display ID（`#42` / `h1#3`）と数値 display ID である。CLI / Agent / TUI はすべて display ID を使うよう設計されており、UUID はエラー時以外はユーザーに見せない方針がとれる。
- **現在の型**: `LIKE ? || '%'` または全行 fetch + `starts_with`（UUID prefix フォールバック）
- **方針**: UUID prefix マッチを削除する。UUID はエラー以外でユーザーに露出させない。`resolve_task_id` は display ID（`#42` / `h1#3` / 数値）と完全 UUID のみを受け付ける。
- **推奨型**:
  - `resolve_task_id` / `resolve_habit_id` / `resolve_task_id_for_memory` から UUID prefix フォールバック（`starts_with` / `LIKE`）を削除する
  - UUID prefix 入力は「該当なし」エラーを返す
  - `display_id` カラムに index を張り、display ID 解決を等価検索にする
  - UUID は内部 ID のままで API レスポンスからも除去を検討する（クライアントは display ID で参照する）
- **修正の重み**: 小
- **該当箇所**:
  - `crates/takusu-worker/src/handlers/tasks.rs:647-653`（全行 fetch + `starts_with` フォールバック）
  - `crates/takusu-worker/src/handlers/habits.rs:292-306`（同上）
  - `crates/takusu-worker/src/handlers/memory.rs:97`（`LIKE` prefix match）
  - `crates/takusu-local-lib/src/storage_sqlite.rs:2895-2956`（SQLite 側 `resolve_task_id` の UUID prefix フォールバック）
  - `crates/takusu-local-lib/src/storage_workers.rs:912-973`（Workers client 側 `resolve_task_id` の UUID prefix フォールバック）

---

## 37. `serde_json::Value` が D1 query の結果型に使われている（`takusu-worker`）

- **問題の要約**: `memory.rs` の一部の query が `Vec<serde_json::Value>` を返し、`v["id"].as_str()` のように動的フィールドアクセスをしている。フィールド名の typo が実行時まで検出されず、型情報が失われる。
- **現在の型**: `Vec<serde_json::Value>`
- **推奨型**:
  - 各 query に専用の result struct（例: `TaskIdRow { id: String }`）を定義し、`sqlx::FromRow` または `serde::Deserialize` で型付きで受け取る
- **修正の重み**: 小
- **該当箇所**:
  - `crates/takusu-worker/src/handlers/memory.rs:62, 78, 90, 98`

---

## 38. ID 解決ロジックが worker handler 3 箇所に重複（`takusu-worker`）

- **問題の要約**: `resolve_task_id` / `resolve_habit_id` / `resolve_task_id_for_memory` がそれぞれ handler 内に実装され、display_id / UUID prefix / full UUID のパターン処理が重複している。`takusu-local-lib` の 15 項とも同じロジックだが、こちらは wasm 環境のため共通モジュールの切り出しがやや複雑。
- **現在の型**: 各 handler の private `fn resolve_*_id`
- **推奨型**:
  - `takusu-worker` 内に `id_resolver` モジュールを作り、共通ロジックを抽出する
  - lookup クロージャを渡す設計にすれば、DB アクセスの違いだけ各 handler が提供できる
- **修正の重み**: 中
- **該当箇所**:
  - `crates/takusu-worker/src/handlers/tasks.rs:591-661`（`resolve_task_id`）
  - `crates/takusu-worker/src/handlers/habits.rs:270-307`（`resolve_habit_id`）
  - `crates/takusu-worker/src/handlers/memory.rs:47-117`（`resolve_task_id_for_memory`）

---

## 39. `Client` のエラーハンドリングが 30 箇所以上で同じパターンを繰り返す（`takusu-client`）

- **問題の要約**: `Client` の各メソッドが `let status = resp.status().as_u16(); if status >= 400 { let body = resp.text().await.unwrap_or_default(); return Err(ClientError::Api { status, body }); }` の同じブロックを 30 回以上繰り返している。
- **現在の型**: 各メソッド内に inline されたエラーチェック
- **推奨型**:
  - `async fn handle_response(resp: reqwest::Response) -> Result<reqwest::Response, ClientError>` を定義し、全メソッドが `let resp = handle_response(resp).await?` で統一する
- **修正の重み**: 小
- **該当箇所**:
  - `crates/takusu-client/src/lib.rs`（30 箇所以上、代表: `:112` 付近から開始）

---

## 40. `Client` の一部メソッドが `serde_json::Value` を返す（`takusu-client`）

- **問題の要約**: `preview_schedule` / `get_oauth_url` / `oauth_callback` / `trigger_sync` が `Result<serde_json::Value, ClientError>` を返す。レスポンスの形がコードから読めず、呼び出し側が `v["field"].as_str()` の動的アクセスを強制される。
- **現在の型**: `serde_json::Value`
- **推奨型**:
  - 各エンドポイントに専用の response struct を定義し、`#[derive(Deserialize)]` で型付き受け取りをする
- **修正の重み**: 小
- **該当箇所**:
  - `crates/takusu-client/src/lib.rs:698`（`preview_schedule`）
  - `crates/takusu-client/src/lib.rs:903`（`get_oauth_url`）
  - `crates/takusu-client/src/lib.rs:923`（`oauth_callback`）
  - `crates/takusu-client/src/lib.rs:943`（`trigger_sync`）

---

## 41. CLI の enum 相当引数が `String` で手動 parse（`takusu-cli`）

- **問題の要約**: CLI の `TaskCommands::Status { status: String }` / `HabitCommands::Create { window: Option<String> }` / `MemoryCommands::Create { kind: String }` / `ScheduleCommands::Reschedule { mode: String }` 等が `String` で、実行時に手動で parse している。clap の `ValueEnum` derive を使えばコンパイル時に検証できる。
- **現在の型**: `String` / `Option<String>`
- **推奨型**:
  - 対応する enum（`TaskStatus` / `WindowMode` / `MemoryKind` / `ScheduleMode`）に `#[derive(clap::ValueEnum)]` を付ける
  - CLI の引数型を enum に変更する
- **修正の重み**: 中
- **該当箇所**:
  - `crates/takusu-cli/src/main.rs:524`（`TaskCommands::Status`）
  - `crates/takusu-cli/src/main.rs:597, 628, 642, 665, 674, 705`（`HabitCommands` の `recurrence` / `window`）
  - `crates/takusu-cli/src/main.rs:238, 242, 269, 271`（`MemoryCommands` の `kind` / `subject_type`）
  - `crates/takusu-cli/src/main.rs:826`（`ScheduleCommands::Reschedule`）

---

## 42. `display_rich.rs` と `display_simple.rs` がロジックを重複（`takusu-cli`）

- **問題の要約**: `display_rich.rs` と `display_simple.rs` が `display_task_detail` / `display_tasks` / `display_habits` / `display_habit_detail` / `display_schedule` / `display_tokens` / `display_skills` の同名関数を持ち、データ変換ロジック（status マーカー、progress フォーマット、habit label lookup）が重複している。違いは出力が comfy-table か plain text かだけ。
- **現在の型**: 2 ファイルに重複する `fn display_*`
- **推奨型**:
  - `DisplayFormatter` trait を定義し、`fn format_task` / `fn format_habit` / ... を持たせる
  - 共通データ変換を `display_common.rs` に抽出する
  - `RichFormatter` / `SimpleFormatter` が出力レンダリングだけを担当する
- **修正の重み**: 大
- **該当箇所**:
  - `crates/takusu-cli/src/display_rich.rs`（全体）
  - `crates/takusu-cli/src/display_simple.rs`（全体）

---

## 43. `editor.rs` が key:value テキストを手動 parse（`takusu-cli`）

- **問題の要約**: `$EDITOR` で開くバッファが `title: ...\nstatus: ...\nstart_at: ...` の ad-hoc なテキスト形式で、`parse_edited_task` が行ごとに `splitn(':')` して各フィールドを文字列として取り出し、再 parse している。フィールド名の typo や形式の崩れが実行時まで検出されない。
- **現在の型**: 手書きテキスト parse
- **推奨型**:
  - バッファ形式を TOML または JSON にし、`serde` で往復させる
  - または `EditTaskForm` struct を定義し、`Serialize` / `Deserialize` でバッファとの変換をする
- **修正の重み**: 中
- **該当箇所**:
  - `crates/takusu-cli/src/editor.rs:21-83`（`format_task_for_editing`）
  - `crates/takusu-cli/src/editor.rs:85-273`（`parse_edited_task`）
  - `crates/takusu-cli/src/editor.rs:275-445`（habit 版）

---

## 44. JWT `Claims.iat` / `exp` が `i64`（Unix 秒）（`takusu-util`）

- **問題の要約**: JWT の `Claims` が `iat: i64` / `exp: Option<i64>` を持ち、Unix 秒を生の整数で保持する。`TokenRow` 側は ISO 8601 文字列で、表現が統一されていない。`iat` / `exp` の意味が型から読めず、秒とミリ秒の取り違えも検出できない。
- **現在の型**: `i64`（Unix 秒）
- **推奨型**: `jiff::Timestamp`（serde アダプタで ISO / Unix 両対応）
- **修正の重み**: 中
- **該当箇所**:
  - `crates/takusu-util/src/jwt.rs:52-55`（`Claims.iat` / `exp`）
  - `crates/takusu-util/src/jwt.rs:111-116, 204-211, 249-250, 267, 275-276`（変換ロジック）

---

## 45. `IcalTask.start_at` / `end_at` が `String`（`takusu-ical`）

- **問題の要約**: iCal parser が `IcalTask { start_at: String, end_at: String }` を返し、呼び出し側が再度 parse する。parser 内部では `Timestamp` に変換してから文字列に戻しているため、型情報がわざわざ落とされている。
- **現在の型**: `String`（ISO 8601）
- **推奨型**: `jiff::Timestamp`
- **修正の重み**: 中
- **該当箇所**:
  - `crates/takusu-ical/src/lib.rs:46-52`（`IcalTask` struct）
  - `crates/takusu-ical/src/lib.rs:208-292, 429-441`（`format_ical_date` と呼び出し側）

---

## 46. `TAKUSU_WORKERS_URL` の `|` split hack（`takusu-local` / `takusu-cli`）

- **問題の要約**: `TAKUSU_WORKERS_URL` が `|` 区切りで複数 URL を保持し、`split('|').next()` で最初の URL を取り出す hack が `takusu-local/src/main.rs` と `takusu-cli/src/main.rs` の両方にある。config crate の env separator と `://` が衝突するための回避策だが、2 番目以降の URL は未使用・未ドキュメントで、`|` が URL に含まれると壊れる。
- **現在の型**: `String` を `split('|')` で分割
- **推奨型**:
  - `WorkerUrls` newtype（`Vec<Url>` を内包）を定義し、env からの parse をカプセル化する
  - または config crate の separator を変えず、`TAKUSU_WORKERS_URL_0` / `TAKUSU_WORKERS_URL_1` の添え字方式にする
- **修正の重み**: 中
- **該当箇所**:
  - `crates/takusu-local/src/main.rs:50`
  - `crates/takusu-cli/src/main.rs:1114`

---

## 47. Local handler の `operation_id` 抽出が複数 handler に重複（`takusu-local`）

- **問題の要約**: `operation_id` ヘッダから idempotency key を取り出す関数が `handlers/task.rs` と `handlers/memory.rs` に同じ実装で重複している。
- **現在の型**: 各 handler の private `fn operation_id`
- **推奨型**: `handlers/common.rs` または `handlers/util.rs` に共通関数として抽出する
- **修正の重み**: 小
- **該当箇所**:
  - `crates/takusu-local/src/handlers/task.rs:14-19`
  - `crates/takusu-local/src/handlers/memory.rs:9-14`

---

## 48. `Storage` trait の query パラメータが stringly-typed（`takusu-storage`）

- **問題の要約**: `TaskQuery` / `MemoryQuery` が `status: Option<String>` / `kind: Option<String>` / `subject_type: Option<String>` / `from: Option<String>` / `until: Option<String>` を持つ。`Storage` trait の境界で型安全性が失われ、各実装が文字列を再 parse する。`from` / `until` はタイムスタンプなのに文字列で、日付形式の違いを各実装が独自に処理している。
- **現在の型**: `Option<String>` 各種
- **推奨型**:
  - `status: Option<TaskStatus>` / `kind: Option<MemoryKind>` / `subject_type: Option<SubjectType>`
  - `from: Option<jiff::Timestamp>` / `until: Option<jiff::Timestamp>`
- **修正の重み**: 中
- **該当箇所**:
  - `crates/takusu-storage/src/model.rs:183-192`（`TaskQuery`）
  - `crates/takusu-storage/src/model.rs:664-676`（`MemoryQuery`）

---

## 49. `update_task` が 150 行で複数責務を抱えている（`takusu-local-lib`）

- **問題の要約**: `storage_sqlite.rs` の `update_task` が単一トランザクション内で、フィールドバリデーション、日時バリデーション、依存関係解決、ステータス遷移、作業セッション清理、数量更新をすべて行っている。1 関数の行数が 150 を超え、各責務のテストが独立できない。
- **現在の型**: 単一の巨大 `fn update_task`
- **推奨型**:
  - `validate_task_update(body, existing)` / `update_task_fields(tx, id, body)` / `handle_status_transition(tx, id, old, new)` / `cleanup_work_sessions(tx, id)` に分割する
  - 各ヘルパーを個別にテスト可能にする
- **修正の重み**: 中
- **該当箇所**:
  - `crates/takusu-local-lib/src/storage_sqlite.rs:656-809`

---

## 50. `validate_scheduled_span_dates` / `parse_calendar_date` が `app.rs` と `storage_sqlite.rs` に重複（`takusu-local-lib`）

- **問題の要約**: `validate_scheduled_span_dates` と `parse_calendar_date` が `app.rs` と `storage_sqlite.rs` の両方に同じ実装で存在する。
- **現在の型**: 2 箇所の重複 `fn`
- **推奨型**: `date_utils.rs` 等の共通モジュールに抽出する
- **修正の重み**: 小
- **該当箇所**:
  - `crates/takusu-local-lib/src/app.rs:425-470`
  - `crates/takusu-local-lib/src/storage_sqlite.rs:3016-3035`

---

## 51. `WorkersStorage` の API path が `format!` で 28 箇所に散らばっている（`takusu-local-lib`）

- **問題の要約**: `storage_workers.rs` が `format!("/api/tasks/{}", id)` / `format!("/api/habits/{}/steps/{}", habit_id, step_id)` 等、API path を `format!` で構築している箇所が 28 箇所ある。path の変更が困難で、URL encode の漏れも起きやすい。
- **現在の型**: `format!("/api/...")` リテラル
- **推奨型**:
  - path 定数を定義し、`fn task_path(id: &str) -> String` のようなヘルパー関数経由で構築する
  - URL encode をヘルパー内で統一する
- **修正の重み**: 小
- **該当箇所**:
  - `crates/takusu-local-lib/src/storage_workers.rs:252, 321, 335, 345, 355, 367, 380, 389, 398, 409, 426, 451, 468, 488, 499, 548, 603, 620, 633, 642, 650, 672, 741, 757, 773, 789, 798, 811`

---

## 52. `HushConfig` が手書き INI parser で文字列キーを lookup している（`takusu-audio`）

- **問題の要約**: `HushConfig::load_from_dir` が INI ファイルを `HashMap<String, String>` に手動で parse し、`"df/sr"` / `"hush/target_rms"` 等の文字列キーで値を取り出している。キーの typo が実行時まで検出されず、型情報がない。
- **現在の型**: `HashMap<String, String>` + 文字列キー lookup
- **推奨型**:
  - `HushConfig` struct に `#[derive(Deserialize)]` を付け、`serde_ini` 等の INI deserializer で直接構築する
  - または `configparser` crate を使う
- **修正の重み**: 中
- **該当箇所**:
  - `crates/takusu-audio/src/hush.rs:77-129`

---

## 53. エラー型の実装不統一と循環参照（ワークスペース全体）

> **関連**: [`.devin/docs/code-style.md`](../.devin/docs/code-style.md) 「Uses `thiserror` for error types」

### 53.1 `thiserror` 未使用の手動 `impl std::error::Error` が 4 型ある

- **問題の要約**: ワークスペースのコード規約は `thiserror` の使用を定めているが、4 つのエラー型が `Display` / `Error` / `From` を手動で実装している。`thiserror` に統一すれば `#[from]` / `#[error("...")]` でボイラープレートが消え、`From` 変換の付け忘れも防げる。
- **該当型**:
  - `takusu-util::QuantityError`（`crates/takusu-util/src/quantity.rs`）: `Negative(i64)` / `Overflow` の enum。手動 `Display` / `Error`
  - `takusu-util::JwtError`（`crates/takusu-util/src/jwt.rs`）: 11 バリアント。手動 `Display` / `Error`。バリアント数が多く手動実装の維持コストが高い
  - `takusu-client::ClientError`（`crates/takusu-client/src/lib.rs`）: `Http(reqwest::Error)` / `Api { status, body }`。`From<reqwest::Error>` も手動。`#[from]` で自動化できる
  - `takusu-agent::InvalidArgsError`（`crates/takusu-agent/src/tool.rs`）: `{ field: Option<String>, reason: String }` の構造体。`From<String>` / `From<&str>` も手動
- **推奨**:
  - 4 型とも `#[derive(Debug, thiserror::Error)]` に移行する
  - `ClientError::Http` は `#[from] reqwest::Error` にする
  - `InvalidArgsError` は `#[error("invalid argument {field:?}: {reason}")]` にする
  - `JwtError` の `InvalidAudience` / `InvalidIssuer` は `#[error("invalid audience: expected {expected}, got {actual}")]` のように derive する
- **修正の重み**: 小

### 53.2 `HttpError` が `std::error::Error` を実装しない

- **問題の要約**: `takusu-local::HttpError` は `AppError` の newtype wrapper だが、`axum::response::IntoResponse` のみを実装し、`std::error::Error` を実装しない。エラー型として扱えず、`?` で伝播できない。`AppError` から `HttpError` への変換は `From` で行われるが、逆方向や他のエラー型との連携ができない。
- **該当箇所**: `crates/takusu-local/src/error.rs`
- **推奨**:
  - `HttpError` に `#[derive(Debug, thiserror::Error)]` を付ける
  - `#[error(transparent)]` で内側の `AppError` に委譲する
  - `From<AppError>` は `#[from]` か `#[transparent]` で維持する
- **修正の重み**: 小

### 53.3 `AgentError` と `AudioError` が循環参照の形を持つ

- **問題の要約**: `takusu-agent::AudioError` が `Agent(AgentError)` バリアントを持ち、`AgentError` は `Tool(ToolError)` / `Client(ClientError)` / `Llm(LlmError)` を持つ。一方で `takusu-agent::AgentError` に `Audio` バリアントはないため、見た目の循環は解けているが、階層関係が不明確。`AudioError` が `AgentError` を包含する設計は、本来 `AgentError` が上位の統合エラーであることと矛盾する。
- **現在の階層**:
  ```
  AgentError (L4 統合)
   ├─ Llm(LlmError)
   ├─ Tool(ToolError)
   └─ Client(ClientError)

  AudioError (L4 統合のサブドメイン)
   ├─ Record(String) / Transcribe(String) / Tts(String) / Play(String)
   ├─ UnsupportedBackend(String) / Timeout
   └─ Agent(AgentError)  ← 上位を包含
  ```
- **問題点**:
  - `AudioError::Agent` は `AgentError` のサブセット（`Llm` / `Tool` / `Client` / `TooManyToolCalls`）を再包 wrap する。元のエラー型情報は失われず残るが、`AgentError` を受け取った呼び出し側がそれを `AudioError` に wrap して再び上位に戻すと、エラーの発生元とハンドリング元の関係が反転する
  - `takusu-agent::audio` モジュールが `AgentError` に依存するため、`AgentError` にバリアントを追加すると `AudioError` の意味論も変わる
- **推奨**:
  - `AudioError::Agent` バリアントを削除する
  - 代わりに `AudioError` を `AgentError` のバリアントにする: `AgentError::Audio(AudioError)`
  - `audio` モジュールの呼び出し元は `Result<_, AudioError>` を返し、それを `AgentError::Audio` で wrap して伝播する
  - これで `AgentError` が唯一の統合エラーとなり、`AudioError` はその下位のドメインエラーになる
- **修正の重み**: 中
- **該当箇所**:
  - `crates/takusu-agent/src/lib.rs`（`AgentError` 定義）
  - `crates/takusu-agent/src/audio.rs`（`AudioError::Agent` バリアント）

---

## 54. OpenAPI spec が存在せず、TS と Rust の型が手動同期（#1182）

> **関連 issue**: [takusu-dev/takusu#1182](https://github.com/takusu-dev/takusu/issues/1182)「Code first OpenAPI def」

### 54.1 現状

- **問題の要約**: ドメイン型が 3 箇所に手動重複しており、OpenAPI spec もコード生成も存在しない。フィールド追加時に 3 箇所を手動で同期する必要がある。
- **重複箇所**:
  - `crates/takusu-storage/src/model.rs`（1001 行、DB モデル）
  - `crates/takusu-client/src/lib.rs`（1977 行、`// Types (mirrors server model.rs)`）
  - `ts/takusu-client/src/types.ts`（539 行、`// Types mirroring takusu-client/src/lib.rs and takusu-storage/src/model.rs`）
- **現状の依存**: `schemars = "1"` が workspace dep にあるが `takusu-agent` の LLM ツールスキーマ生成にしか使われていない。Axum（`takusu-local`）にも Cloudflare Worker（`takusu-worker`）にも OpenAPI 生成フレームワークはない。

### 54.2 `aide` vs `axum-openapi3` の比較

| 項目 | `aide` | `axum-openapi3` |
|---|---|---|
| スキーマ derive | `schemars::JsonSchema` | `utoipa::ToSchema` |
| 既存 dep との整合 | `schemars` が既に workspace に入っている。`takusu-agent` も使用 | `utoipa` は新規導入。`ToSchema` と `JsonSchema` の二重管理 |
| Axum 統合 | `ApiRouter` に差し替え。`route` / `api_route` の使い分けで段階移行可 | `endpoint` マクロ + `AddRoute` trait |
| ネストルート | OK | **不可**（README に明記） |
| 複数サーバー | OK | **不可**（global `Mutex` で spec をキャッシュ） |
| レスポンス型 | `IntoApiResponse` なら任意 | **`Json` のみ** |
| 成熟度 | High reputation、snippet 1011、dependents 40 | dependents 0、制約多め |

- **採用**: `aide`
- **理由**:
  1. `schemars` が既に workspace に入っており、`takusu-agent` の LLM ツールスキーマと同じ `JsonSchema` derive で一貫する。`utoipa` を入れると `ToSchema` と `JsonSchema` の二重管理になる
  2. `takusu-local/src/router.rs:148` が `.nest("/api", api)` を使っており、ネストルート不可の `axum-openapi3` は確実に除外される
  3. `route` を `api_route` に変えたものだけが OpenAPI に載るため、段階的移行ができる
  4. `axum-openapi3` の「1 プロセス 1 サーバー」「`Json` レスポンスのみ」制約が実運用に合わない

### 54.3 バージョンの注意

- `aide` 0.15.x（stable）は `schemars ^0.9.0` を要求する。ワークスペースは `schemars = "1"` のため **バージョン衝突する**
- `aide` 0.16.0-alpha.4（2026-04-14 公開）は `schemars ^1.0.4` を要求し、ワークスペースと互換する
- **リスク**: 0.16 は alpha。ただし 2025-11 から 8 ヶ月開発が続いており、alpha.4 は 23k ダウンロード。stable 待ちの場合は #1182 の着手を延期する必要がある
- **代替案**: `schemars` を 0.9 に下げることはできない（`takusu-agent` が `schemars 1` に依存しているため）

### 54.4 推奨される実装方針

1. #31 完了後、`takusu-contract`（L1）の全ドメイン型に `#[derive(schemars::JsonSchema)]` を付ける（1 箇所のみ）
2. `takusu-local` に `aide 0.16` を導入し、`Router` を `ApiRouter` に、`route` を `api_route` に差し替える
3. `openapi-typescript` で OpenAPI spec から `ts/takusu-client/src/types.ts` を自動生成する
4. `takusu-worker` は Axum ではないが、型は `takusu-contract` を共有するため OpenAPI spec は `takusu-local` から生成したもので両方をカバーできる
5. CI で spec を再生成し、差分があれば fail する仕組みを入れる
- **修正の重み**: 大
- **前提**: #31（ドメイン型の集約）が完了していること

---

## 修正の進め方

### 原則

- **1 項目 1 PR**（ただし下記の「同 PR でやるべき組み合わせ」を除く）
- **各 PR は単独で完結**し、既存の動作を破壊しない
- **順序依存**があるものは前の PR が merge されてから着手
- **同一ファイルを触る** PR は並行できない（merge conflict が確実）

### 同 PR でやるべき組み合わせ

| 組み合わせ | 理由 |
|---|---|
| #24 + #25 | `SttBackend` enum を作る（#25）と `build_stt` の文字列マッチ（#24）が同じファイルで密接に絡む |
| #19 + #20 + #21 | `TypedTool` trait の改善（#19）がツール出力（#20）と JSON Schema（#21）の前提になる。3 つは `takusu-agent/src/tools/` の同じファイル群を触る |
| #53.1 + #53.2 + #53.3 | エラー型の統一。3 つとも `thiserror` 移行で、別々にやると `Cargo.toml` の `thiserror` feature 管理が衝突する |

### フェーズ構成

#### Phase 1: 独立・小修正（即並行可能）

他の issue とファイル衝突しない。どの順序でも、どれから始めてもよい。

| Issue | Crate | 重み | 備考 |
|---|---|---|---|
| #1 | takusu-core | 小 | `lib.rs` の weight const。#10 と同じファイル |
| #2 | takusu-core | 小 | `model.rs` の Task flags |
| #3 | takusu-core | 中 | `neighborhood.rs` |
| #4 | takusu-core | 中 | `evaluation.rs` |
| #5 | takusu-core | 小 | `tabu.rs` |
| #6 | takusu-core | 中 | `solver.rs` |
| #7 | takusu-core | 中 | `repair.rs` |
| #8 | takusu-core | 中 | `placement.rs` |
| #9 | takusu-core | 中 | `config.rs` |
| #10 | takusu-core | 小 | `lib.rs` の Planner config。#1 と同じファイルなので #1 の後に |
| #30 | takusu-audio-cli | 小 | WAV I/O 重複。独立 |
| #41 | takusu-cli | 小 | enum 引数の String parse |
| #42 | takusu-cli | 中 | display_rich/display_simple 重複 |
| #43 | takusu-cli | 小 | editor.rs key:value parse |
| #45 | takusu-ical | 小 | IcalTask String → 型付き |
| #46 | takusu-local / cli | 小 | `TAKUSU_WORKERS_URL` の `\|` split hack |
| #47 | takusu-local | 小 | operation_id 抽出の重複 |
| #52 | takusu-audio | 中 | HushConfig INI parser |

#### Phase 2: クレート内で順序が必要なもの

| Issue | Crate | 重み | 前提 | 備考 |
|---|---|---|---|---|
| #53.1 | ワークスペース全体 | 小 | なし | `thiserror` 移行（4 型）。#44 と同じファイル |
| #53.2 | takusu-local | 小 | なし | `HttpError` に `std::error::Error` 実装 |
| #53.3 | takusu-agent | 中 | なし | `AudioError::Agent` 削除 → `AgentError::Audio` に逆転 |
| #44 | takusu-util | 小 | #53.1 | JWT Claims i64 → 型付き。#53.1 が jwt.rs を thiserror 化した後 |
| #22 | takusu-agent | 小 | なし | ツール名・操作名の文字列リテラル集約 |
| #23 | takusu-agent | 中 | なし | ツール登録の自動化。#22 と同じファイル群だが順不同 |
| #18 | takusu-agent | 中 | なし | `execute_proposed_change` の巨大 match |
| #26 | takusu-audio | 小 | なし | Cartesia container/encoding/emotion |
| #27 | takusu-audio | 小 | なし | API key/URL の String 型 |
| #29 | takusu-audio | 小 | なし | sample rate 16000 ハードコード |
| #48 | takusu-storage | 中 | なし | Storage trait の query パラメータ型付き化 |
| #16 | takusu-local-lib | 小 | なし | WorkersStorage HTTP メソッド統合 |
| #17 | takusu-local-lib | 小 | なし | AppError の String 型改善 |

#### Phase 3: 同一ファイル群で順序が必要

| Issue | Crate | 重み | 前提 | 備考 |
|---|---|---|---|---|
| #19+#20+#21 | takusu-agent | 大 | なし | TypedTool / ツール出力 / JSON Schema。1 PR |
| #24+#25 | takusu-audio / agent | 中 | #53.3 | SttBackend enum + build_stt。#53.3 が audio.rs を触った後 |
| #28 | takusu-audio | 小 | #24+#25 | SttError の String → 構造化。stt.rs を #24+#25 が触った後 |
| #39 | takusu-client | 中 | なし | Client エラーハンドリングの重複。#31 と同じファイル |
| #40 | takusu-client | 中 | #39 | Client の serde_json::Value 返却。#39 と同じファイル |
| #35 | takusu-worker / local-lib | 中 | なし | COALESCE hack。#31 と同じファイル |
| #36 | takusu-worker / local-lib | 中 | なし | UUID prefix 削除 |
| #37 | takusu-worker | 中 | #36 | serde_json::Value in D1 query。#36 が ID 解決を単純化した後 |
| #38 | takusu-worker | 中 | #36 | ID 解決重複。#36 と同じ handler |
| #32 | takusu-storage / worker | 中 | なし | depends/depends_on/schedule JSON → 型付き |
| #33 | takusu-storage / worker | 小 | なし | SimilarTaskRow.similarity 文字列 |
| #34 | takusu-storage | 小 | なし | MemoryRow.source String |
| #12 | takusu-local-lib | 中 | なし | validate/parse 関数の struct 統合 |
| #13 | takusu-local-lib | 小 | なし | RescheduleInput.mode String → enum |
| #49 | takusu-local-lib | 中 | なし | update_task 150 行分割 |
| #50 | takusu-local-lib | 小 | なし | validate_scheduled_span_dates 重複 |
| #51 | takusu-local-lib | 中 | なし | WorkersStorage API path 集約 |

#### Phase 4: 大規模リファクタ（前提多数）

| Issue | Crate | 重み | 前提 | 備考 |
|---|---|---|---|---|
| #31 | ワークスペース全体 | 大 | #32, #33, #34, #35, #39, #40 が merge 済み | クレートレイヤー整理 + 型集約。これらが先に merge されていると #31 は「型を 1 箇所に集める」だけに専念できる |
| #11 | takusu-local-lib | 大 | #12, #13, #49, #50, #51 が merge 済み | app.rs 3745 行の god module 分割。個別修正が終わった後で分割 |

#### Phase 5: #31 の後

| Issue | Crate | 重み | 前提 | 備考 |
|---|---|---|---|---|
| #54 (#1182) | takusu-local / contract / ts | 大 | #31 | `aide` 導入 + `JsonSchema` derive + `openapi-typescript` 生成 |

### 同時進行できないグループ

同一ファイルを触るため、並行すると merge conflict が確実に発生する。順次対応する。

| グループ | 触るファイル | 順序 |
|---|---|---|
| #31 / #32 / #33 / #34 / #39 / #40 | `storage/model.rs`, `client/lib.rs`, `worker/models.rs` | #31 が先。残りは #31 後に 1 箇所で直す |
| #11 / #12 / #13 / #49 / #50 | `takusu-local-lib/src/app.rs` | #12, #13, #49, #50 が先（並行不可）。#11 は最後 |
| #1 / #10 | `takusu-core/src/lib.rs` | 順不同だが並行不可 |
| #36 / #38 | `takusu-worker` handler | #36 が先（UUID prefix 削除で ID 解決が単純化する） |
| #44 / #53.1 | `takusu-util/src/jwt.rs` | #53.1 が先（thiserror 移行）。#44 はその後 |
| #24+#25 / #28 | `takusu-audio/src/stt.rs` | #24+#25 が先。#28 はその後 |
| #24 / #53.3 | `takusu-agent/src/audio.rs` | #53.3 が先（AudioError::Agent 削除）。#24 はその後 |

### 並行可能なセット

異なるクレートの PR は完全に並行できる。

| 並行セット | Issue 数 | 備考 |
|---|---|---|
| セット A | #1, #2, #3, #4, #5, #6, #7, #8, #9 | takusu-core 内、ファイル別（#1→#10 のみ順序あり） |
| セット B | #30, #41, #42, #43, #45, #46, #47, #52 | 完全独立 |
| セット C | #53.1, #53.2, #53.3, #22, #23, #18, #26, #27, #29, #48, #16, #17 | Phase 2（異なるクレートなら並行） |

### 依存関係のグラフ

```
Phase 1 (並行可能):
  #1 → #10 (同ファイル)
  #2 #3 #4 #5 #6 #7 #8 #9 (takusu-core 内、独立)
  #30 #41 #42 #43 #45 #46 #47 #52 (完全独立)

Phase 2 (並行可能、Phase 1 と独立):
  #53.1 → #44 (同ファイル)
  #53.2 #53.3 (独立)
  #22 #23 #18 (takusu-agent、独立)
  #26 #27 #29 (takusu-audio、独立)
  #48 #16 #17 (独立)

Phase 3 (クレート内で順序):
  #19+#20+#21 (1 PR)
  #53.3 → #24+#25 → #28 (audio.rs → stt.rs)
  #39 → #40 (client/lib.rs)
  #36 → #37, #36 → #38 (worker handler)
  #35 (worker query、独立)
  #32 #33 #34 (model.rs、互いに独立だが同ファイル)
  #12 #13 #49 #50 #51 (app.rs、互いに独立だが同ファイル)

Phase 4:
  #32+#33+#34+#35+#39+#40 merge 後 → #31
  #12+#13+#49+#50+#51 merge 後 → #11

Phase 5:
  #31 merge 後 → #54 (#1182)
```
