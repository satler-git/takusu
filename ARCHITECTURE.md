# takusu アーキテクチャ

この文書は、takusu のコードベース全体を読む人間に向けて、アーキテクチャの全体像、
各クレートの役割・関係、コアアルゴリズムの詳細、設計判断の理由を説明する。

## 1. システム全体像

takusu は「自動スケジュール構築プランナー」と「音声アシスタント」の2つの側面を持つ。
Rust のワークスペースとして13のクレートで構成され、スケジューリングコア・REST APIサーバー・
CLIクライアント・音声処理・Google Calendar連携がそれぞれ独立したクレートになっている。

```
                    ┌──────────────────┐
                    │   takusu-cli     │  CLIクライアント
                    │   takusu-client  │  HTTP client library
                    └────────┬─────────┘
                             │ HTTP
                    ┌────────▼─────────┐
                    │  takusu-local    │  REST APIサーバー (axum + SQLite/Workers)
                    └────────┬─────────┘
                            │
          ┌─────────────────┼──────────────────┐
          ▼                 ▼                   ▼
   ┌──────────┐    ┌──────────────┐    ┌──────────────┐
   │takusu-core│    │ google-cal   │    │ takusu-ical  │
   │ (SA+LNS   │    │ (Calendar    │    │ (iCal parser)│
   │  +Tabu)   │    │  API client) │    └──────────────┘
   └──────────┘    └──────────────┘
   ┌──────────┐    ┌──────────────┐
   │takusu-   │    │ takusu-audio │
   │habit     │    │ (STT/Record/ │
   │(習慣生成) │    │  TTS trait)  │
   └──────────┘    └──────────────┘
   ┌──────────┐    ┌──────────────┐
   │takusu-   │    │ takusu-      │
   │storage   │    │ worker       │
   │(Storage  │    │ (Cloudflare  │
   │ trait)   │    │  Worker/CDYLIB)│
   └──────────┘    └──────────────┘
```

### クレート依存関係（簡略化）

```
takusu-local
  ├── takusu-core     (スケジューリングエンジン)
  ├── takusu-ical     (iCal パース)
  ├── google-cal      (Google Calendar API)
  ├── takusu-util     (日付解析、トークン生成)
  ├── takusu-contracts  (Storage trait)
  └── takusu-local-lib (ビジネスロジック)

takusu-cli
  └── takusu-client   (HTTP client; reqwest)

takusu-audio
  ├── cpal            (マイク録音)
  └── reqwest         (モデルダウンロード)

takusu-local
  ├── takusu-core
  ├── takusu-ical
  ├── google-cal
  ├── takusu-util
  └── takusu-contracts  (Storage trait)

takusu-worker
  └── takusu-contracts  (Storage trait, WASM/D1用)
```

## 2. コアアルゴリズム: takusu-core

### 2.1 時間モデル

1 スロット = 5 分。`Point(i64)` でエポックからのスロット数を表す。
すべてのタスクはこの離散時間軸上に配置される。

```
秒単位のタイムスタンプ → Point::from_timestamp(ts, 5) → Point(スロット数)
Point(スロット数)      → point_to_iso()                 → "2026-06-22T10:00:00Z"
```

- `Point::now()`: 現在時刻をスロット単位に切り上げ（端数の4分は切り捨て）
- `day_start`: タイムゾーンに応じて計算される「その日の0時」のスロット位置
- 睡眠: `SleepConfig { day_start, start (22:00), end (06:00), enabled }`

### 2.2 タスクモデル

```rust
pub struct Task {
    pub id: usize,
    pub start: Option<Point>,        // 開始可能時刻
    pub end: Option<Point>,          // 締切
    pub cost_estimate: NormalDist,   // 所要見積り (正規分布)
    pub depends: Vec<usize>,         // 依存タスクID
    pub parallelizable: bool,        // 自分が並列実行可能か？
    pub allows_parallel: bool,       // 他タスクの並列実行を許可するか？
    pub abandonability: f64,         // 0.0-1.0 諦めやすさ
}
```

### 2.3 アルゴリズム: SA + LNS + Tabu Search

#### フェーズ1: 初期解生成
1. トポロジカルソート（依存関係のあるタスクを先に）
2. フリーネス（余裕のなさ）順にソート: 余裕がないタスクほど優先
3. `try_place()` でタスクを1つずつ配置:
   - 依存タスクの完了後
   - スリープ時間を避け
   - 並列可能なタスクは重ねて配置
   - 配置できない場合はスケジュール末尾にペナルティ付きで配置

#### フェーズ2: 焼きなまし (SA)
```
T = T0 (総平均所要時間 × 0.1)
α = 0.93
iter_per_temp = タスク数 × 20 (最小100)

while T > T_min (T0 × 1e-4):
    for _ in 0..iter_per_temp:
        neighbor = generate_neighbor(current)  // 5種類の近傍
        δ = evaluate(neighbor) - evaluate(current)
        if δ < 0 || rand() < exp(-δ / T):
            current = neighbor
            if evaluate(current) > evaluate(best):
                best = current
    T *= α
```

#### 近傍生成 (5種類、確率分配)

| 種類 | 確率 | 動作 |
|------|------|------|
| shift | 25% | タスクをランダムな位置に移動 |
| swap | 25% | 2つのタスクを入れ替え |
| duration | 20% | タスクの所要時間を±1スロット |
| reorder | 15% | 隣接タスクの順序入れ替え |
| LNS | 15% | Large Neighborhood Search: ピボットタスクを中心に時間窓内のタスクを破壊し、フリーネス順に再構築 |

#### Tabu List
- 直前のタスクIDを記録（サイズ: タスク数 / 5 + 1）
- タブーでも、現行最良を上回る場合は採用（aspiration criterion）

#### 評価関数 (8成分)

```
total = inclusion_bonus + deadline_score + start_score + buffer_score
        + depend_score + duration_score + sleep_score + parallel_violation
```

すべてペナルティ重みは正。ペナルティは負の寄与として加算。
全成分が `i64` で計算可能（浮動小数点不使用）。

| 成分 | 計算式 | 重み | 備考 |
|------|--------|------|------|
| deadline_score | `slack × W_EARLY` / `slack × W_LATE × (1-abandonability)` | 1.0 / 20.0 | 早期ボーナス上限50 |
| start_score | `(sched_start − start) × W_START` | 5.0 | 早すぎる場合のみ |
| depend_score | `−violation_slots × W_DEPEND_BASE × (1−T/T₀)` | 100.0 | 制約アニーリング |
| buffer_score | `σ × remaining_slots × W_BUFFER` | 2.0 | 高σタスクに余裕を |
| duration_score | `−deficit² × W_SHORT` / `deficit × W_OVER` | 3.0 / 0.5 | 短すぎは2次 |
| sleep_score | `−sleep_used × W_NORMAL` / `−deficit² × W_SEVERE` | 4.0 / 15.0 | 3時間未満は重度 |
| parallel_violation | `−overlap_slots × W_PARALLEL_VIOL` | 500.0 | 不正な重なり（準硬制約） |
| inclusion_bonus | `+W_INCLUSION × scheduled_tasks` | 10.0 | タスク維持への報酬 |

#### 評価キャッシュ戦略
- `eval_current` / `eval_best` は温度ステップごとに1回計算
- `eval_neighbor` のみ各反復で計算
- 差分評価（neighbor生成前後の変化分のみ計算）は複雑さに対して効果が薄いため非採用
- 代わりに近傍全体を再評価（evaluate() は全成分を再計算）
- これにより3-4倍の高速化（評価関数の計算が支配的でないため）

#### Partial Rescheduling
- `plan_partial(pinned)`: 指定タスクを固定し、残りのみ最適化
- `plan_in_range(range, current, extra_pinned)`: 現在スケジュール中のタスクのうち、
  指定範囲内にあるものだけを再スケジュール。範囲外のタスクは位置を保持

```
pinned_tasks = out_of_range_tasks ∪ extra_pinned
free_tasks = in_range_tasks - extra_pinned
```

`solve_partial()` では近傍生成時に pinned タグをスキップ。

### 2.4 abandonability の意味

- `abandonability` が高い（0.8-1.0）: 締切を逃しても許容。
  `deadline_score` のペナルティが `(1 - abandonability)` 倍される。
- `abandonability` が低い（0.0-0.2）: 締切厳守。フルペナルティ。
- **タスクは絶対にドロップされない**: 不可能ならスケジュール末尾にペナルティ付きで配置される。

### 2.5 並列実行のモデル化

- `parallelizable == true`: このタスクは他のタスクと同時に実行できる（スマホ操作など）
- `allows_parallel == true`: このタスク実行中に他のタスクを同時実行できる（待ち時間など）
- 両方 `true` の場合のみ重なりが許可される
- 不正な重なりは `parallel_violation` としてペナルティ（重なったスロット数 × 500）

## 3. サーバーアーキテクチャ

### 3.1 takusu-local (プラグイン可能ストレージ)

`takusu-local-lib` が中核のビジネスロジックを提供し、`Storage` トレイト経由で
ストレージバックエンドを切り替えられる。

```rust
#[async_trait]
pub trait Storage: Send + Sync {
    // tokens
    async fn create_token(&self, ...) -> Result<...>;
    async fn get_token_by_hash(&self, ...) -> Result<...>;
    async fn list_tokens(&self, ...) -> Result<...>;
    async fn revoke_token(&self, ...) -> Result<...>;
    // tasks, habits, schedules, settings, google_cal...
}
```

- `TAKUSU_STORAGE=sqlite` → 直接 SQLite (`storage_sqlite.rs`)
- `TAKUSU_STORAGE=workers` → Cloudflare Worker 経由 HTTP
- `takusu-worker` は WASM (cdylib) として Worker 上で動作、D1 (CloudflareのSQLite互換DB) を使う

## 4. データフロー

### 4.1 スケジュール生成（完全版）

```
CLI: takusu schedule generate
                  │
                  ▼ POST /api/schedule/generate { sleep: "..." }
         ┌────── takusu-local ──────┐
         │                          │
         │ 1. parse_sleep()         │
         │    (tz・睡眠時間設定)      │
         │                          │
         │ 2. SELECT tasks          │
         │    WHERE status IN       │
         │    ('pending','scheduled')│
         │                          │
         │ 3. Planner::new(          │
         │       PlannerConfig::new( │
         │         now, sleep))      │
         │    for each task:        │
         │      planner.add(task)   │
         │                          │
         │ 4. planner.plan()        │
         │    → 4 parallel SA chains│
         │    → best Plan selected  │
         │                          │
         │ 5. UPSERT schedules      │
         │    (id='active')         │
         │                          │
         │ 6. UPDATE tasks SET      │
         │    status='scheduled'    │
         │    WHERE id IN (...)     │
         │                          │
         │ 7. do_sync() (Google Cal)│
         │                          │
         └────────┬─────────────────┘
                  ▼
          Response { entries: [...], ... }
```

### 4.2 部分再スケジュール（範囲指定）

```
POST /api/schedule/reschedule { mode: "range", from: "...", until: "..." }

1. 現在のスケジュールを取得
2. 範囲内のタスクを特定（start が from..until にあるもの）
3. 範囲外のタスク = pinned
4. planner.plan_in_range(range, current_schedule, extra_pinned=[])
5. 結果をDBに保存 → sync
```

### 4.3 エントリ移動 (Move)

```
PATCH /api/schedule/entries/:task_id { new_start: "...", force: false }

1. タスクを新しい位置に移動
2. 検証:
   - 締切超過 → warning
   - 依存関係違反 → warning
   - 睡眠時間と重なり → warning
3. force=false かつ warning あり → 409 Conflict + warnings
4. force=true → 警告を無視して更新
5. 保存 → sync
```

## 5. クライアント

### 5.1 takusu-client (HTTPクライアントライブラリ)

```rust
let client = Client::new("http://localhost:3000", "tsk_xxx");
let tasks = client.list_tasks(None).await?;
let schedule = client.generate_schedule(&req).await?;
```

- 全APIエンドポイントに対応
- `ClientError` は HTTPエラーと APIエラーを区別
- シリアライゼーションラウンドトリップテストあり

### 5.2 takusu-cli (CLIクライアント)

- clap derive でサブコマンドを定義
- 表示モード: `--mode rich` (comfy-table + color) / `--mode simple` (plain text)
- エディタ編集: `task edit <id>` で一時ファイルにタスクを書き出し、`$EDITOR` で編集後パースして PATCH
- コンフィグ: `$XDG_CONFIG_HOME/takusu/config.toml`

## 6. 音声処理

### 6.1 録音 (takusu-audio/src/record.rs)

- `cpal` でマイク入力
- フォーマット: F32 または I16、モノラル mix
- 16kHz リサンプリング
- RMS 0.1 に正規化
- Enter キーで停止

### 6.2 STT (takusu-audio/src/sherpa.rs)

```
[takusu-audio-cli / takusu-agent]
   │
   ├── model dir or cache ──▶ Sherpa-ONNX OfflineRecognizer
   │
   └── f32 16kHz mono PCM ──▶ SherpaOnnxAsr::transcribe ──▶ text
```

- バックエンド: Sherpa-ONNX (`OfflineRecognizer`)
- モデル: SenseVoice (default), FunASR Nano
- 言語: SenseVoice 言語指定可 (`auto`, `zh`, `en`, `ja`, `ko`)
- 初回実行時は `sherpa-sense-voice-int8` を `ModelCache` から自動ダウンロード
- `funasr-nano` は `--sherpa-model-dir` でローカルモデルが必要

### 6.3 TTS (takusu-audio/src/tts.rs)

- `TextToSpeech` トレイトと `TtsRequest`/`TtsOptions`/`TtsConfig`/`TtsError` 共有型
- 新しい TTS バックエンドは `TextToSpeech` を実装して追加する

## 7. Google Calendar 連携

### 7.1 OAuth2 フロー

```
takusu sync login --client-id <ID> --client-secret <SECRET>
   → 127.0.0.1 のローカルコールバックサーバーを起動
   → ブラウザを開いて Google 認証
   → サーバーが認可コードを受け取り、CLI が Google とトークン交換
   → refresh_token を DB に保存
```

- モバイルアプリでも `react-native-nitro-google-signin` を使ってネイティブ
  Google サインインから server auth code を取得し、バックエンドで
  refresh_token に交換できる (issue #1057)
- バックエンドの `/api/sync/oauth/callback` は `redirect_uri` なしでも
  server auth code を交換できる
- CLI で取得した refresh_token を共有バックエンド (local SQLite /
  Workers D1) から読み取って同期に使用することもできる
- モバイル設定画面に refresh_token 手動入力フィールドあり (フォールバック)
- `takusu sync setup --refresh-token <TOKEN>` で直接トークンを設定することも可能

### 7.2 差分同期 (google-cal/src/lib.rs)

```
do_sync():
1. DB の schedule エントリ一覧を取得
2. DB の google_cal_events マッピング一覧を取得
3. 削除・更新・作成を `BatchOp` に変換
4. Google Calendar Batch API (`/batch/calendar/v3`) へ 1 リクエストにまとめて送信
   - 1 リクエストあたり最大 1000 件で chunk 分割
5. 各レスポンスの `Content-ID` から対応する `BatchOp` を特定
6. 更新失敗時は `delete_event()` → `create_event()` にフォールバック
7. マッピングを upsert/delete
```

- リトライなし（同期は schedule 操作のたびに実行されるため、失敗分は次回同期で回収）
- `delete_event()` 410 Gone は成功扱い
- `delete_all()` も同様に Batch API を使用

## 8. iCal インポート (takusu-ical)

純粋 Rust の iCalendar パーサー。HTTP依存なし。

```
parse_ical(input: &str, tz: &jiff::tz::TimeZone) -> Result<Vec<IcalTask>, IcalError>

入力: BEGIN:VCALENDAR ... VEVENT ... END:VCALENDAR
出力: Vec<IcalTask { title, start_at, end_at, description, uid }>
```

- LINE FOLDING 対応 (RFC 5545 §3.1)
- DTSTART/DTEND: UTC (`Z` サフィックス)、明示的 UTC オフセット、浮動時間、日付のみ (終日)、`TZID` パラメータに対応。結果は UTC に正規化される。
- `DURATION` も `DTEND` の代替として使用可能。日付のみで `DTEND`/`DURATION` がない場合は翌日終日として扱う。
- UID 重複スキップ（インポート時）

## 9. 習慣生成 (takusu-habit)

`HabitConfig` トレイト + `RecurrenceRule` ビルダー。

```rust
let rule = RecurrenceRule::new(Frequency::Weekly)
    .interval(2)       // 2週間ごと
    .by_day(&[Weekday::Mon, Weekday::Wed])
    .count(10);        // 10回で終了

let gen = rule.create_generator(start, until)?;
```

対応: daily, weekly, monthly (by nth weekday 含む), yearly, exdates.

## 10. Cloudflare Worker 対応

`takusu-worker` は `worker-build` で WASM にコンパイルされ、Cloudflare Workers 上で動作。
D1 (SQLite互換) をストレージに使用。

`takusu-contracts` クレートの `Storage` トレイトを実装することで、
Worker とローカルサーバーで同一のハンドラコードを使用可能。

```
takusu-worker (WASM / cdylib)
  ├── router.rs    → HTTPリクエストのルーティング
  ├── handlers/    → 各エンドポイントのハンドラ（takusu-local と共有コード）
  └── d1.rs       → D1 へのクエリ実行
```

## 11. テスト戦略

| レベル | 対象 | 方法 | 数 |
|--------|------|------|----|
| 単体 | takusu-core | `#[cfg(test)] mod tests` | ~50 |
| 単体 | takusu-ical | `#[cfg(test)] mod tests` | 20 |
| 単体 | takusu-client | シリアライズラウンドトリップ | - |
| 統合 | takusu-local | axum oneshot + インメモリSQLite | 24 |
| ベンチマーク | takusu-core | Criterion (25/50/100 tasks) | 4 |

統合テストは実HTTPサーバー不要。`axum::Router` に直接リクエストを送る `oneshot` 方式。

## 12. 重要な設計判断

### 12.1 なぜ z3 ではなく焼きなましか？

開発環境に z3 が含まれているが、コアアルゴリズムには SAT ソルバーを使用していない。
理由:
- タスク数が増えると SAT の爆発が制御不能
- 評価関数が多目的（8成分）で、SAT では重み付き最適化が難しい
- SA は「まあまあ良い解」を速く出すのに適している
- 制約アニーリングにより、高温では制約違反を許容して探索空間を広く取れる

### 12.2 なぜタスクをドロップしないのか？

- ユーザーは「タスクが消えた」ことに気づきにくく、信頼を損なう
- 不可能なタスクは常にスケジュール末尾に置かれ、ペナルティで表現される
- `abandonability` で「諦めの度合い」を調整可能

### 12.3 なぜ storage 抽象化層があるのか？

- `Storage` トレイトで抽象化することで、ローカルSQLiteとCloudflare Workersの両方で
  同一のビジネスロジック (`takusu-local-lib`) を再利用できる。
- `takusu-local` + `takusu-worker`: Cloudflare Workers への移行パス。
- `takusu-cli` は `takusu-local-lib` を直接使用し、ネットワーク通信なしで動作する。

### 12.4 なぜ評価キャッシュ戦略が差分評価でないのか？

「近傍生成前後の変化分だけを再計算する」差分評価は、
各成分が複雑に相互作用する評価関数では保守が難しく、バグの元になる。
代わりに「温度ステップごとに eval_current と eval_best を1回だけ計算し、
近傍評価時は常に全再計算」という戦略を取る。
評価関数が支配的でないこのドメインでは、これで十分な速度が出る。

### 12.5 なぜスロットを5分にしたのか？

- 人間のタスク管理に十分な粒度
- 解空間が大きくなりすぎない（15分だと粗すぎ、1分だとSAが収束しない）
- 制約: この値はコード中にマジックナンバーとして散在しており、変更時は全クレートの修正が必要

## 13. コードナビゲーション

### 最初に読むべきファイル

| 順序 | ファイル | 理由 |
|------|----------|------|
| 1 | `crates/takusu-core/src/lib.rs` | コアの型定義（Point, Task, Planner） |
| 2 | `crates/takusu-core/src/evaluate.rs` | 評価関数（何が良いスケジュールか） |
| 3 | `crates/takusu-core/src/anneal.rs` | SA+LNS+Tabu の実装 |
| 4 | `crates/takusu-local-lib/src/app.rs` | サーバーでの使い方 |
| 5 | `crates/takusu-contracts/src/model.rs` | APIのデータモデル |
| 6 | `crates/takusu-client/src/lib.rs` | クライアントから見たAPI |

### 主要な型の定義場所

| 型 | 場所 |
|----|------|
| `Point` | `takusu-core/src/lib.rs:7` |
| `Task` | `takusu-core/src/lib.rs:254` |
| `Planner` | `takusu-core/src/lib.rs:333` |
| `Plan` | `takusu-core/src/lib.rs:458` |
| `SleepConfig` | `takusu-core/src/lib.rs:113` |
| `RescheduleRange` | `takusu-core/src/lib.rs:483` |
| `NormalDist` | `takusu-core/src/lib.rs:41` |
| `TaskRow`, `CreateTask`, `UpdateTask` | `takusu-contracts/src/model.rs` |
| `ScheduleRow`, `ScheduleEntry` | `takusu-contracts/src/model.rs` |
| `AppError` | `takusu-local-lib/src/error.rs` |
| `Storage` trait | `takusu-contracts/src/storage.rs` |
| `Client` | `takusu-client/src/lib.rs` |
| `SherpaOnnxAsr` | `takusu-audio/src/sherpa.rs` |
| `TtsClient` | `takusu-audio/src/tts.rs` |
| `Habit`, `RecurrenceRule` | `takusu-habit/src/lib.rs` |
| `IcalTask` | `takusu-ical/src/lib.rs` |
| `GoogleCalClient` | `google-cal/src/lib.rs` |

### テストの場所

| テスト | 場所 |
|--------|------|
| コアアルゴリズム | `takusu-core/src/*.rs` (cfg test モジュール) |
| サーバー統合テスト | `takusu-local/tests/integration.rs` (1024行) |
| iCal パーサー | `takusu-ical/src/lib.rs` (cfg test モジュール) |
| クライアント直列化 | `takusu-client/tests/roundtrip.rs` |
| ローカルサーバー統合 | `takusu-local/tests/integration.rs` |

### 既知の脆いコード

.devin/docs/code-style.md の "Hacks / Brittle Code" セクションを参照。
特に注意すべきもの:
- `point_to_iso` のハードコードされた 5分スロット (全クレートに分散)
- ~~`LIKE ? || '%'` による前方一致ID解決 (フルテーブルスキャン)~~ 解決済み (#1251)
- `COALESCE` によるフィールドクリア不能問題 (Worker側・SQLite側の両方)
- `freeness()` の直感に反する命名 (高いほど余裕がある = 優先度が低い)
