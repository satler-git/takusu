# takusu コード品質改善候補まとめ（第 2 版）

このドキュメントは、現時点のコードベースを独立して監査し、次の 4 つの基準に該当する問題を整理したものである。
各項目は「問題の要約」「該当箇所」「推奨される改善」「修正の重み（小/中/大）」を含む。

**監査基準**

1. Rust の作法に従っていなくて hack となっている
2. 型が適切に使用されておらず型安全性を損ない、手動でいちいちバリデーションによって保証している
3. Trait で綺麗に抽象化できるのに、されていない
4. 今後の拡張性を損なう技術的負債

**対象外・留意事項**

- `String` や `serde_json::Value` を enum/newtype/専用 struct に置き換えるべき箇所は、`doc/type-safety-issues.md` と第 1 版 (`doc/code-quality-issues.md`) で既出のため本ドキュメントでは扱わない。
- 単純なエラーメッセージ `String` や、自由入力の `title` / `description` / `body` フィールドは対象外とする。
- 大規模な crate 分離は推奨しない。型と trait の変更に絞る。

---

## 1. アーキテクチャ全体を貫く問題

本章では、単一クレートの問題ではなく、複数クレートにまたがる構造的負債を扱う。
これらは修正の影響範囲が広く、放置すると保守コストが線形に増大するため、最優先で対応すべき項目である。

### 1.1 `takusu-worker` が `Storage` trait を実装せず、ビジネスロジックが全面重複している

- **問題の要約**: `takusu-storage::Storage` trait は `SqliteStorage` と `WorkersStorage`（HTTP クライアント）の 2 実装を持ち、`TakusuApp` はこの trait に依存してビジネスロジックを集約している。一方 `takusu-worker` はこの trait を一切実装せず、各ハンドラが D1 の生 SQL と `JsValue` バインディングで永続化とビジネスロジックを両方再実装している。その結果、タスク CRUD、進捗管理、ハビット CRUD、メモリ、スキル、設定、スケジュール、トークン、同期マッピングの全エンドポイントで同一の業務ロジックが 2 箇所に並行して存在する。片方を修正する際にもう片方を忘れると、ローカルとクラウドで挙動が乖離する。
- **該当箇所**:
  - `crates/takusu-worker/src/handlers/tasks.rs:39-583`（タスク CRUD, 656 行）↔ `crates/takusu-local-lib/src/app/task.rs:32-365` + `crates/takusu-local-lib/src/storage_sqlite.rs:554-790`
  - `crates/takusu-worker/src/handlers/progress.rs:147-730`（進捗・ワークセッション・分割, 730 行）↔ `crates/takusu-local-lib/src/storage_sqlite.rs:1893-2470`
  - `crates/takusu-worker/src/handlers/habits.rs:26-562`（ハビット CRUD, 567 行）↔ `crates/takusu-local-lib/src/app/habit.rs` + `crates/takusu-local-lib/src/storage_sqlite.rs:818-990`
  - `crates/takusu-worker/src/handlers/memory.rs:154-607`（メモリ, 607 行）↔ `crates/takusu-local-lib/src/app/mod.rs:202-295`
  - `crates/takusu-worker/src/handlers/skills.rs:58-199` ↔ `crates/takusu-local-lib/src/app/mod.rs:122-198`
  - `crates/takusu-worker/src/handlers/settings.rs:13-72` ↔ `crates/takusu-local-lib/src/app/mod.rs:102-118`
  - `crates/takusu-worker/src/handlers/schedule.rs:12-65` ↔ `crates/takusu-local-lib/src/storage_sqlite.rs` の `get_schedule` / `save_schedule` / `clear_schedule`
  - `crates/takusu-worker/src/handlers/tokens.rs:34-117` ↔ `crates/takusu-local-lib/src/app/mod.rs:297-311`
  - `crates/takusu-worker/src/handlers/sync.rs:29-141` ↔ `crates/takusu-local-lib/src/storage_sqlite.rs` の gcal mappings 実装
- **推奨される改善**: `takusu-worker` に `Storage` trait の D1 実装（`D1Storage`）を導入し、ハンドラは `TakusuApp` に委譲する。D1 の `JsValue` バインディングの違いは trait 実装内部に隠蔽する。`WorkersStorage`（HTTP クライアント）と `D1Storage`（直接 D1）の 2 実装を並存させ、`takusu-local` と同じ「薄いラッパー」構造にする。
- **修正の重み**: 大

### 1.2 バリデーションロジックが `takusu-worker` と `takusu-local-lib` で重複定義されている

- **問題の要約**: `takusu-local-lib/src/validate.rs` は `Validate` trait を定義し、`CreateTask` / `UpdateTask` / `CreateHabit` / `CreateSkill` / `CreateMemory` / `[HabitStepInput]` 等に実装している。一方 `takusu-worker/src/validate.rs` は同じ内容のフリー関数を再実装している。両者のコメントに「Mirrors the worker-side / mirrors `takusu-local-lib`」と明記されており、意図的な重複だが、片方を修正する際にもう片方を忘れると不整合が生じる。マジックナンバー（`60 * 24 * 365`、`64`、`100`、`500`、`64 * 1024`）も重複している。
- **該当箇所**:
  - `crates/takusu-worker/src/validate.rs:113-144`（`validate_minutes`）↔ `crates/takusu-local-lib/src/validate.rs:33-64`
  - `crates/takusu-worker/src/validate.rs:151-155`（`validate_title`）↔ `crates/takusu-local-lib/src/validate.rs:71-75`
  - `crates/takusu-worker/src/validate.rs:159-186`（`validate_quantity`）↔ `crates/takusu-local-lib/src/validate.rs` の `CreateTask::validate` 内
  - `crates/takusu-worker/src/validate.rs:190-194`（`validate_recurrence`）↔ `crates/takusu-local-lib/src/validate.rs:81-85`（`parse_recurrence`）
  - `crates/takusu-worker/src/validate.rs:214-218`（`validate_timezone`）↔ `crates/takusu-local-lib/src/validate.rs:90-94`
  - `crates/takusu-worker/src/validate.rs:223-229`（`validate_settings`）↔ `crates/takusu-local-lib/src/validate.rs:303-311`
  - `crates/takusu-worker/src/validate.rs:237-263`（`validate_task_datetimes`）↔ `crates/takusu-local-lib/src/validate.rs:112-135`
  - `crates/takusu-worker/src/validate.rs:268-301`（`validate_steps`）↔ `crates/takusu-local-lib/src/validate.rs:316-350`
  - `crates/takusu-worker/src/validate.rs:305-329`（`detect_cycle`）↔ `crates/takusu-local-lib/src/graph.rs`（`detect_cycle`）
  - `crates/takusu-worker/src/handlers/memory.rs:101-152`（`validate_create` / `validate_update`）↔ `crates/takusu-local-lib/src/validate.rs:194-244`
  - `crates/takusu-worker/src/handlers/skills.rs:16-56`（`validate_slug` / `validate_create`）↔ `crates/takusu-local-lib/src/validate.rs:149-192`
- **推奨される改善**: `Validate` trait と各バリデータを `takusu-storage`（または新設の `takusu-validate` クレート）に切り出し、worker と local-lib の両方から利用する。worker が WASM バンドルサイズを気にする部分（`takusu-habit` 依存の `RecurrenceRule`）は feature flag で分離する。
- **修正の重み**: 中

### 1.3 ID 解決ロジックが重複し、文字列 prefix マッチで型安全性を損なっている

- **問題の要約**: display_id（`#42`、`h1#3`、`h2`）や完全 UUID を受け付けてルックアップするロジックが、`SqliteStorage`、`WorkersStorage`、`takusu-worker` の 3 箇所にほぼ同一の実装で存在する。受け付け形式のパース（`#` prefix、`h<N>#<M>` 形式、数値、完全 UUID 判定）が文字列 prefix マッチで行われており、型で保証されていない。パース部分を共通化すれば、各バックエンドは「解決された参照から UUID へのルックアップ」だけを実装すれば済む。なお UUID prefix 解決は #1251 で削除済みであり、現在は完全 UUID 一致のみを受け付ける。
- **該当箇所**:
  - `crates/takusu-local-lib/src/storage_sqlite.rs:2960-3006`（`resolve_task_id`）
  - `crates/takusu-local-lib/src/storage_sqlite.rs:3011-3033`（`resolve_habit_id`）
  - `crates/takusu-local-lib/src/storage_sqlite.rs:3039-3048`（`resolve_depends`）
  - `crates/takusu-local-lib/src/storage_workers.rs:1040-1100`（`resolve_task_id`）
  - `crates/takusu-worker/src/handlers/id_resolver.rs:28-81`（`resolve_task_id`）
  - `crates/takusu-worker/src/handlers/id_resolver.rs:89-119`（`resolve_habit_id`）
  - `crates/takusu-worker/src/handlers/tasks.rs:587-599`（`resolve_depends`）
  - `crates/takusu-local-lib/src/config.rs:41-44`（`storage_kind` の文字列マッチ）
- **推奨される改善**: ID 参照のパース規則を `takusu-storage`（または `takusu-types`）に `TaskRef` / `HabitRef` のような enum として切り出す。各バックエンドは「解決された参照から UUID」のルックアップだけを実装する。`storage_kind` も enum にする。
- **修正の重み**: 中

### 1.4 スケジュール要求型が 3 層に重複定義されている

- **問題の要約**: スケジュールの preview / generate / reschedule / move_entry のリクエスト型が、クライアント、ローカルハンドラ、アプリ層の 3 箇所で重複定義されている。クライアント層とローカルハンドラ層は同名の構造体だがフィールド型が異なり（`MoveEntry.start_at` が `Timestamp` vs `String`）、アプリ層は `Input` サフィックス付きの別名型。変換コードが各層に必要になり、新フィールド追加時に 3 箇所の修正が必要になる。
- **該当箇所**:
  - `crates/takusu-client/src/lib.rs:1028-1091`（`SchedulePreviewRequest` / `GenerateSchedule` / `Reschedule` / `MoveEntry`、`MoveEntry.start_at: Timestamp`）
  - `crates/takusu-local/src/handlers/schedule.rs:15-62`（同名型、`MoveEntry.start_at: String`）
  - `crates/takusu-local-lib/src/app/schedule.rs:100-123`（`GenerateScheduleInput` / `RescheduleInput` / `SchedulePreviewInput`）
  - `crates/takusu-client/src/lib.rs:1093-1100`（`MoveEntryResponse`、`start_at: Timestamp`）↔ `crates/takusu-local-lib/src/app/schedule.rs:136-141`（`MoveEntryOutput`、`start_at: String`）
- **推奨される改善**: クライアント・サーバ間で共有すべき型を `takusu-storage::model` に集約し、クライアントは再エクスポートのみ行う。`from` / `until` / `start_at` は `Option<Timestamp>` に統一し、ハンドラ層で `String` から `Timestamp` のパースを一度だけ行う。
- **修正の重み**: 中

---

## 2. God Object 化

本章では、単一の構造体やファイルが過剰な責務を抱え込み、拡張時に影響範囲が肥大化している箇所を扱う。

### 2.1 `AgentSession` が God Object 化している（`takusu-agent`）

- **問題の要約**: `lib.rs` は 4241 行に達する。`AgentSession` 構造体が config、LLM、履歴、compaction、承認、権限、schedule dirty、skills、discovered tools の約 17 フィールドを保持し、impl ブロックがターン orchestration、ストリーミング、承認実行、compaction 起動、履歴 trimming、システムプロンプト構築、habit step 解析、依存サイクル検出を全て抱える。さらに `TtsQueue` と markdown から speech への文境界ロジック（約 420 行）とテスト（約 2000 行）も同一ファイルに存在する。新しい機能追加時に影響範囲が巨大で可読性も低い。
- **該当箇所**:
  - `crates/takusu-agent/src/lib.rs:236-658`（`TtsQueue` / `markdown_to_speech`）
  - `crates/takusu-agent/src/lib.rs:662-688`（`AgentSession` 構造体）
  - `crates/takusu-agent/src/lib.rs:690-2252`（impl ブロック）
  - `crates/takusu-agent/src/lib.rs:2254-4241`（テスト）
- **推奨される改善**: `TtsQueue` / `markdown_to_speech` を `tts_queue.rs`（または `takusu-audio`）へ分離する。履歴 trimming / compaction を `history.rs`、承認フローを `approval.rs`、habit step 処理を `habit_steps.rs` へ抽出する。テストは `tests/` 配下へ移動する。
- **修正の重み**: 大

### 2.2 `TakusuApp` が 60 以上の public メソッドを持つ God Object（`takusu-local-lib`）

- **問題の要約**: `TakusuApp` は既にサブモジュール（`task.rs`、`habit.rs`、`schedule.rs`、`gcal.rs`、`dependency.rs`）に分割されているが、それでも 60 以上の public メソッドを持つ。新しい機能追加時の影響範囲が大きく、どのメソッドがどのドメインに属するかが構造体レベルで見えにくい。`build_planner` メソッドは約 120 行で依存関係解決、habit group 割り当て、planner 構築を全て行っている。
- **該当箇所**:
  - `crates/takusu-local-lib/src/app/mod.rs`（20 個の public メソッド）
  - `crates/takusu-local-lib/src/app/task.rs`（15 個）
  - `crates/takusu-local-lib/src/app/habit.rs`（16 個）
  - `crates/takusu-local-lib/src/app/schedule.rs:558-678`（`build_planner`、約 120 行）
- **推奨される改善**: ドメインごとに trait を導入する（例: `TaskService`、`HabitService`、`ScheduleService`）。`TakusuApp` をこれらの trait を集約する facade にする。各 service を独立した struct として実装し、`Arc<dyn Storage>` を注入する。`build_planner` は依存関係解決、habit group 割り当て、planner 構築の 3 メソッドに分割する。
- **修正の重み**: 大

### 2.3 `MutationArgs` が全種別の全フィールドを `Option<T>` で抱える god struct（`takusu-agent`）

- **問題の要約**: `MutationArgs` は task / habit / schedule の約 30 フィールドを全て `Option<T>` で持つ。`steps: Option<Vec<Value>>` は habit step を untyped `Value` で保持し、後段で手動分解される。全フィールドが `Option` のため「未指定」と「null」が区別できず、`validate_no_foreign_fields` による手動弁別が必要になっている。型が種別ごとの許可フィールドを表現していないため、実行時バリデーションで型安全性を補っている。さらに `MutationKind` に対して `tool_name`、`description`、`target_type`、`operation`、`change_summary`、`allowed_fields`、`schema`、`validate_args`、正規化、before 取得の 10 箇所以上の match が存在し、新種別追加時に全 match 箇所の編集が必要になる。フィールドリストの不一致が静的に検出されない。
- **該当箇所**:
  - `crates/takusu-agent/src/tools/takusu/mutation.rs:82-92`（`MutationKind` enum）
  - `crates/takusu-agent/src/tools/takusu/mutation.rs:94-332`（`tool_name` / `description` / `target_type` / `operation` / `change_summary` / `allowed_fields` / `schema` の多段 match）
  - `crates/takusu-agent/src/tools/takusu/mutation.rs:363-503`（`MutationArgs` god struct）
  - `crates/takusu-agent/src/tools/takusu/mutation.rs:505-545`（`set_fields` の手動ミラーリング）
  - `crates/takusu-agent/src/tools/takusu/mutation.rs:557-642`（`validate_no_foreign_fields` の別 match）
  - `crates/takusu-agent/src/tools/takusu/mutation.rs:665-715`（`validate_args`）
  - `crates/takusu-agent/src/tools/takusu/mutation.rs:717-900`（`call_typed` 内の正規化と before 取得）
- **推奨される改善**: `MutationKind` ごとに独立した `TypedTool` impl（`CreateTaskTool` / `UpdateTaskTool` / ...）に分割する。共通メタデータ（`target_type` / `operation` / `description`）は trait の associated const または各 impl で定義する。種別ごとの引数 struct（`CreateTaskArgs` / `UpdateTaskArgs` / ...）に分割し、フィールド許可リストを型レベルで表現して `set_fields` / `validate_no_foreign_fields` を廃止する。
- **修正の重み**: 大

---

## 3. Trait 抽象化の不足

本章では、Trait で綺麗に抽象化できるのにされていない箇所を扱う。
既存の良い実装例（`PlacementStrategy`、`SolverStrategy`、`SpeechToText` / `TextToSpeech`）は本章末尾にまとめる。

### 3.1 LLM プロバイダ切替が trait で抽象化されておらず `OpenAIClient` 一択（`takusu-agent`）

- **問題の要約**: `LlmClient` trait は存在するが、実装が `OpenAIClient` のみ。`LlmProviderKind` enum（`Openai` / `Openrouter` / `Custom`）はディスパッチに使われず、`OpenAIClient::new` が直接呼ばれる。OpenRouter / Custom は「OpenAI 互換エンドポイント」として `OpenAIClient` に押し込まれている。Anthropic ネイティブや Gemini のような非 OpenAI 互換プロバイダを追加するには、`OpenAIClient` を分岐させるか新規構築サイトを増やす必要がある。SSE のメッセージ形式の差異も `OpenAIClient` 内で吸収している。
- **該当箇所**:
  - `crates/takusu-agent/src/llm.rs:18-24`（`LlmProviderKind` enum）
  - `crates/takusu-agent/src/llm.rs:358-376`（`LlmClient` trait）
  - `crates/takusu-agent/src/llm.rs:378-605`（`OpenAIClient` のみの実装）
  - `crates/takusu-agent/src/transport.rs:482-486`（`OpenAIClient::new` の直接呼び出し）
- **推奨される改善**: `LlmClientFactory` trait（または `LlmProviderKind` に対する `fn build(config: &LlmConfig) -> Result<Arc<dyn LlmClient>>`）を導入し、プロバイダごとの impl で SSE / メッセージ形式の差異を吸収する。`LlmProviderKind` を実際のディスパッチに使う。
- **修正の重み**: 中

### 3.3 `NeighborOp` trait に LNS 操作が含まれていない（`takusu-core`）

- **問題の要約**: `ShiftOp`、`SwapOp` 等は `NeighborOp` trait を実装しているが、`neighbor_lns_into` 等の複雑な操作は trait 化されていない。`generate_neighbor_into` で match 分岐しており、LNS 操作だけが trait の外に存在する。新しい近傍操作を追加する際に、trait に追加するか match に追加するかが非一貫である。
- **該当箇所**:
  - `crates/takusu-core/src/anneal.rs:1816-1862`（`NeighborOp` trait 定義）
  - `crates/takusu-core/src/anneal.rs:1724-1748`（`generate_neighbor_into` での match 分岐）
  - `crates/takusu-core/src/anneal.rs:2181-2214`（`neighbor_lns_into`、trait 外）
- **推奨される改善**: LNS 操作も trait に含めるか、別の trait 階層を設計する。または `NeighborOp` を enum dispatch に変更して一貫性を持たせる。
- **修正の重み**: 中

### 3.5 `execute_proposed_change` の target 取得と実行器選択が二重分岐（`takusu-agent`）

- **問題の要約**: `execute_proposed_change` は `(change.target.kind, change.operation)` で target 取得を match し、その後 `change_executor::dispatch` で再度同じ `(kind, operation)` を match して実行器を選ぶ。target 取得と実行器選択が別 match に分かれており、新しい `TargetKind` 追加時に両方の編集が必要になる。`change_executor` は既に `ChangeExecutor` / `ChangeHandler` trait で抽象化されているので、target 取得も trait に持たせれば単一拠点化できる。
- **該当箇所**:
  - `crates/takusu-agent/src/lib.rs:1824-1865`（target 取得の match）
  - `crates/takusu-agent/src/lib.rs:1877-1882`（dispatch 呼び出し）
  - `crates/takusu-agent/src/change_executor.rs:882-912`（`dispatch` の match）
- **推奨される改善**: `ChangeExecutor` に `async fn fetch_target(&self, ctx) -> Result<(String, Option<Timestamp>, Option<HabitDetail>)>` を追加し、`dispatch` と同じテーブルで target 取得も解決する。`execute_proposed_change` の match を削除する。
- **修正の重み**: 中

### 3.6 `client_error` が 3 箇所に重複し、文字列部分一致でエラー分類（`takusu-agent`）

- **問題の要約**: `client_error` が `tools/takusu/common.rs`、`tools/memory.rs`、`tools/skills.rs` の 3 箇所に存在する。takusu / skills 版は `body.contains("not found") || body.contains("Not found")` という文字列部分一致で `NotFound` か `InvalidArgs` かを判定する。memory 版はステータスコード 404 で判定する。同じ `ClientError::Api` に対して 3 つの異なる分類ロジックが存在し、サーバ側のエラーメッセージ文言変更で分類が壊れる。
- **該当箇所**:
  - `crates/takusu-agent/src/tools/takusu/common.rs:33-47`
  - `crates/takusu-agent/src/tools/memory.rs:15-28`
  - `crates/takusu-agent/src/tools/skills.rs:125-139`
- **推奨される改善**: `takusu_client::ClientError` に `to_tool_error()` を実装するか、単一の共有 `client_error` 関数に集約する。分類はステータスコード（400 / 404 / 409）ベースに統一し、部分一致は廃止する。
- **修正の重み**: 小

### 3.7 `trim_messages` と `replace_history` がほぼ同一のトークン trimming ループを重複（`takusu-agent`）

- **問題の要約**: `trim_messages` と `replace_history` は、target 計算から `adjusted_target` の f64 比率式、末尾からのメッセージ削除、`tool` ロールの連続チェックまで、ほぼ同一のロジックを重複実装している。片方を修正する際にもう片方を忘れると、trim と replace で挙動が乖離する。
- **該当箇所**:
  - `crates/takusu-agent/src/lib.rs:2086-2172`（`trim_messages`）
  - `crates/takusu-agent/src/lib.rs:2174-2251`（`replace_history`）
- **推奨される改善**: 共通の `trim_to_target(messages, target_tokens, ratio) -> Vec<Message>` 関数に抽出し、両者から呼び出す。
- **修正の重み**: 小

---

## 4. Rust の作法からの逸脱

本章では、`unwrap()` の濫用、エラーの握りつぶし、グローバル可変状態、blocking 呼び出しなど、Rust のイディオムから外れた hack 的なコードを扱う。

### 4.1 `unwrap_or_default()` でエラーを握りつぶしている（`takusu-local-lib`）

- **問題の要約**: HTTP レスポンスのエラーボディ読み取りや JSON シリアライズ失敗時に `unwrap_or_default()` を使用しており、エラーが空文字列で隠蔽される。これによりデバッグが困難になる。エラーボディが空文字列になるため、エラーの原因がネットワーク障害なのかサーバ側のエラーなのか判別できない。
- **該当箇所**:
  - `crates/takusu-local-lib/src/storage_workers.rs:291, 302, 340, 631`（`resp.text().await.unwrap_or_default()`）
  - `crates/takusu-local-lib/src/storage_sqlite.rs:1574`（`serde_json::to_string(body).unwrap_or_default()`）
- **推奨される改善**: エラーを `StorageError::Internal` に変換して伝播する。`resp.text().await.map_err(|e| StorageError::Internal(format!("read body: {e}")))?` のように明示的なエラー処理に置換する。
- **修正の重み**: 中

### 4.2 `.ok()` でエラーを握りつぶしている（`takusu-local-lib`）

- **問題の要約**: ディレクトリ作成や値の取得失敗時に `.ok()` を使用しており、失敗が無視される。ディレクトリ作成失敗は権限問題やパス問題を示す可能性があるが、ログにも出力されない。
- **該当箇所**:
  - `crates/takusu-local-lib/src/storage_sqlite.rs:159`（`std::fs::create_dir_all(parent).ok()`）
  - `crates/takusu-local-lib/src/storage_sqlite.rs:619, 720, 2453`（`normalize_text(...).ok()`）
  - `crates/takusu-local-lib/src/token_cache.rs:27`
- **推奨される改善**: ディレクトリ作成失敗はログに出力するかエラーを返す。`normalize_text` の失敗が意図的な NULL 許容ならコメントで明示する。
- **修正の重み**: 小

### 4.3 本番パスの `Mutex` / `RwLock` ガード `.unwrap()` が散在（`takusu-agent`）

- **問題の要約**: `AgentSession` の全 `Mutex` / `RwLock` フィールドに対し、本番 async パスで `.lock().unwrap()` / `.read().unwrap()` / `.write().unwrap()` を呼んでいる。あるスレッドがロック保持中に panic すると `PoisonError` が伝播し、以降の `.unwrap()` でエージェントプロセス全体がクラッシュする。`pending_approval()` だけ `.lock().ok()?` で不整合。`ToolRegistry` のキャッシュ系も同様。
- **該当箇所**:
  - `crates/takusu-agent/src/lib.rs:755-764, 777-795, 815-852, 874-875, 905-940, 976, 1081-1103, 1119-1143, 1164-1261, 1321-1335, 1471-1500, 1521-1545, 1914, 2052-2081, 2114-2250`
  - `crates/takusu-agent/src/tool.rs:812-813, 893-965`
- **推奨される改善**: ロック取得を隠蔽するプライベートヘルパで `PoisonError` を `AgentError` に変換するか、`parking_lot::Mutex`（poison 無し）へ移行する。少なくとも本番パスの `.unwrap()` は `.lock().map_err(...)?` 等に置換する。
- **修正の重み**: 中

### 4.4 ガードカウンタによる無限ループ防止（`takusu-core`）

- **問題の要約**: ループの終了条件がガードカウンタ（`guard > 10_000` / `guard > 1000`）に依存しており、これは hack 的アプローチ。本来はアルゴリズム的に終了を保証すべき。ガードカウンタに到達した場合、結果が不完全な可能性があるが、その旨が呼び出し側に伝わらない。
- **該当箇所**:
  - `crates/takusu-core/src/placement.rs:262-269`（`try_place` の guard カウンタ）
  - `crates/takusu-core/src/decoder.rs:1093-1098`（`feasible_slots` の guard カウンタ）
- **推奨される改善**: アルゴリズム的に終了を保証する（例: 計算済みの upper bound に到達したら終了）。どうしてもガードが必要な場合は、より明確な名前とドキュメントを付け、ガード到達時に警告ログを出力する。
- **修正の重み**: 中

### 4.5 `thread_local` static のグローバル可変状態（`takusu-core`）

- **問題の要約**: `placement.rs` で `thread_local` static の `RefCell<Vec<>>` を使用しており、これはパフォーマンス最適化のための hack。グローバル可変状態であり、テスト時の挙動が予測しにくい。並列実行時にスレッド間で状態が共有されないことは保証されるが、同じスレッド内で前回の呼び出しの残骸が残る可能性がある。
- **該当箇所**:
  - `crates/takusu-core/src/placement.rs:53-62`（`thread_local! static INSERTION_PLAN` 等）
- **推奨される改善**: パフォーマンスが許容するなら、通常の scratch buffer を引数で渡す設計に変更する。どうしても `thread_local` が必要な場合は、明確なドキュメントとクリアメソッドを提供する。
- **修正の重み**: 中

### 4.6 `record.rs` で blocking な `thread::spawn` を async context 内で使用（`takusu-audio`）

- **問題の要約**: `record()` 関数内で `std::thread::spawn` を使用して stdin からの入力待ちを行っている。これは blocking 操作であり、async context 内での使用には不適切。CPAL の audio stream callback 内で `try_lock` を使用しているが、失敗時に握りつぶしている。
- **該当箇所**:
  - `crates/takusu-audio/src/record.rs:113-118`（`thread::spawn` での stdin 待ち）
  - `crates/takusu-audio/src/record.rs:79-81`（`try_lock` 失敗時の握りつぶし）
  - `crates/takusu-audio/src/record.rs:137`（`unwrap()` の使用）
- **推奨される改善**: stdin 待ちを async 化するか、`tokio::task::spawn_blocking` を使用する。`try_lock` 失敗時のログ出力を追加する。`unwrap()` を適切なエラー処理に置き換える。
- **修正の重み**: 中

### 4.7 `clone()` の過剰使用（`takusu-local-lib`、`takusu-agent`）

- **問題の要約**: 多数の場所で `.clone()` が使用されており、パフォーマンスに影響する可能性がある。`Arc<String>` の clone、ループ内での HashMap のキー / 値の clone、LLM メッセージ変換での文字列 clone 等が散在する。多くは所有権移動のために必要だが、参照渡しや `Cow` で回避可能な箇所も含まれる。
- **該当箇所**:
  - `crates/takusu-local-lib/src/app/dependency.rs:23, 44, 98, 112-121, 139, 153-162`（HashMap のキー / 値の clone）
  - `crates/takusu-local-lib/src/app/task.rs:34, 84, 90, 94, 151, 157, 160, 167, 169, 230, 266, 321, 459-460`（body の clone）
  - `crates/takusu-local-lib/src/app/gcal.rs:152-155, 178, 182, 192, 201, 203, 224, 338-341, 347`（credentials の clone）
  - `crates/takusu-agent/src/lib.rs:938, 1045, 1051`（Arc 値の clone）
  - `crates/takusu-agent/src/llm.rs:168, 172, 294, 472-473`（メッセージ変換での文字列 clone）
  - `crates/takusu-agent/src/tools/takusu/mutation.rs:769, 771, 807, 823, 828, 846, 868-870, 1065, 1071, 1079, 1081`（JSON map と文字列の clone）
- **推奨される改善**: `Arc<String>` を `Arc<str>` に変更する。ループ内での clone は参照を使用するようにリファクタリングする。一部の clone は `Cow` や参照渡しで回避可能。
- **修正の重み**: 中

---

## 5. 型安全性の欠如（タプル・番兵値・フラグ）

本章では、`String` / `Value` 以外の型安全性問題、すなわちタプルによる意味表現、番兵値、`f64` の範囲制約なし、フラグ変数による状態表現を扱う。

### 5.1 `Point(i64)` と `Slots(i64)` の区別が曖昧（`takusu-core`）

- **問題の要約**: `Point` と `Slots` はどちらも `i64` の newtype だが、コード中で直接 `i64` として演算されており、意味の混同が起きている。`Point(start.0 + dur)` のように `Point` と `i64`（duration）を足しており、`.0` アクセスで newtype の境界が形骸化している。`Point + Slots -> Point` の演算子が定義されていれば、`.0` アクセスを減らせる。
- **該当箇所**:
  - `crates/takusu-core/src/anneal.rs:1923`（`Point(new_start_0 + dur)`）
  - `crates/takusu-core/src/anneal.rs:1954`（`Point(b_p.start.0 + a_dur)`）
  - `crates/takusu-core/src/placement.rs:270`（`Point(cursor.0 + dur)`）
  - 他多数の箇所で同様のパターン
- **推奨される改善**: `Point + Slots -> Point` の演算子を定義する。または明示的な変換メソッドを用意し、`.0` アクセスを減らす。
- **修正の重み**: 中

### 5.2 タプルによる意味表現（`takusu-core`）

- **問題の要約**: `TabuList` の scratch buffer で `(start, end)` をタプルで表現しており、意味が不明確。`TimeWindow` 型が既に存在するのにタプルを使っている。
- **該当箇所**:
  - `crates/takusu-core/src/anneal.rs:1298`（`tabu_scratch: Vec<Option<(i64, i64)>>`）
  - `crates/takusu-core/src/anneal.rs:1669-1696`（`mark_tabu_scheds` での使用）
- **推奨される改善**: `Vec<Option<TimeWindow>>` に変更する。または専用の `TabuEntry` 型を定義する。
- **修正の重み**: 小

### 5.3 番兵値としての `i64::MAX` 使用（`takusu-core`）

- **問題の要約**: 未配置の後続タスクがある場合、`latest_start` として `i64::MAX` を使用しており、これは番兵値パターン。`Option<Point>` で十分表現可能なのに、番兵値で「無い」状態を表現している。番兵値は呼び出し側で特別扱いが必要で、忘れると `i64::MAX` がそのまま演算に使われてしまう。
- **該当箇所**:
  - `crates/takusu-core/src/decoder.rs:244-259`（`latest_end_for` 関数）
  - `crates/takusu-core/src/decoder.rs:774, 783, 823, 829, 838`（同様のパターン）
- **推奨される改善**: `Option<Point>` を返すように変更し、呼び出し側で `unwrap_or()` を使用する。
- **修正の重み**: 小

### 5.4 `Vec<Option<TimeWindow>>` で「無い」状態を表現（`takusu-core`）

- **問題の要約**: task_id から `TimeWindow` へのマッピングに `Vec<Option<>>` を使用しており、`None` チェックが散在している。`index.get(*dep_id).and_then(|x| *x)` のパターンが多数の箇所で繰り返されている。task_id が密であることを前提とするなら現状の設計は許容されるが、index アクセスをカプセル化するヘルパーがないため、呼び出し側で `Option` の処理が重複する。
- **該当箇所**:
  - `crates/takusu-core/src/evaluate.rs:194`（`index: &mut Vec<Option<TimeWindow>>`）
  - `crates/takusu-core/src/evaluate.rs:378-410`（`build_index_into`）
  - `crates/takusu-core/src/decoder.rs:957`（`index: Vec<Option<TimeWindow>>`）
- **推奨される改善**: index アクセスをカプセル化するヘルパー関数を追加する。または `IndexMap` 等のより明確なデータ構造を使用する。
- **修正の重み**: 小

### 5.6 `Permissions` が `BTreeMap<String, bool>` で文字列キー＋ワイルドカード文字列結合（`takusu-agent`）

- **問題の要約**: `Permissions` は `allow: BTreeMap<String, bool>`。`resolve` は `format!("{target}:{operation}")`、`format!("{target}:*")`、`format!("*:{operation}")`、`"*:*"` の文字列を都度生成して検索する。呼び出し側は `change.target.kind.as_str()` / `change.operation.as_str()` で enum を文字列に変換してから渡す。権限キーの typo が静かに不一致になり、`format!` によるアロケーションが毎回発生する。`TargetKind` / `ChangeOperation` enum が存在するのに文字列境界で受け渡ししている。
- **該当箇所**:
  - `crates/takusu-agent/src/permissions.rs:6-48`（`Permissions` 構造体と `resolve` メソッド）
  - `crates/takusu-agent/src/lib.rs:878-895`（呼び出し側での enum から文字列への変換）
- **推奨される改善**: `Permissions` のキーを `(TargetKind, ChangeOperation)` または `PermissionKey` 型にし、ワイルドカードは `Option<TargetKind>` / `Option<ChangeOperation>` で表現する。`resolve` は enum 受け取りにする。
- **修正の重み**: 中

### 5.7 `normalize_status` が文字列 match でステータス同義語を処理し `_ =>` で握りつぶし（`takusu-agent`）

- **問題の要約**: `normalize_status` は lowercased 文字列に対する手書き match で "done" / "complete" / "completed"→"completed" などの同義語を処理し、`_ => lower` で未知の値をそのまま返す。未知の値が黙って通過し、後段の `parse::<TaskStatusFilter>` で初めて弾かれる。`TaskStatus` enum が既に存在するのに文字列で分岐している。
- **該当箇所**:
  - `crates/takusu-agent/src/tools/takusu/common.rs:280-292`（`normalize_status`）
  - `crates/takusu-agent/src/tools/takusu/mutation.rs:741-745`（使用箇所）
  - `crates/takusu-agent/src/tools/takusu/read_tools.rs:182-195`（使用箇所）
- **推奨される改善**: `TaskStatus` / `TaskStatusFilter` の `FromStr` に同義語を集約し、`normalize_status` は `s.parse::<TaskStatusFilter>()` に委譲する。未知値は即座にエラーにする。
- **修正の重み**: 小

---

## 6. マジックナンバーとハードコード

本章では、設計意図が不明確なマジックナンバーと、実行時に変更できないハードコードされた設定値を扱う。
これらは可読性を下げ、パラメータ調整を困難にする技術的負債である。

### 6.5 Google Calendar API エンドポイントがハードコード（`google-cal`）

- **問題の要約**: Google Calendar API のエンドポイント URL が `const` としてハードコードされている。テスト用の `with_urls` メソッドがあるが、これは `cfg(test)` のみ。本番コードでエンドポイントを変更できない。
- **該当箇所**:
  - `crates/google-cal/src/lib.rs:11-14`（URL のハードコード）
  - `crates/google-cal/src/lib.rs:202-209`（`events_url()` での URL 構築）
  - `crates/google-cal/src/lib.rs:211-218`（`event_url()` での URL 構築）
- **推奨される改善**: URL を config 構造体に移動し、コンストラクタで渡せるようにする。デフォルト値を提供しつつ、カスタマイズ可能にする。
- **修正の重み**: 中

---

## 7. 手動実装のパーサと数値計算

本章では、ライブラリが存在するにもかかわらず手動実装されているパーサや数値計算を扱う。
これらは精度問題やバグの温床であり、ライブラリへの移行が望ましい箇所である。

### 7.1 `parse_duration_seconds` が `f64` で計算している（`takusu-ical`）

- **問題の要約**: RFC 5545 duration パーサーを手動実装しており、`f64` で計算している。浮動小数点数の精度問題が発生する可能性がある。`jiff` ライブラリを使用しているが、duration パースには利用していない。
- **該当箇所**:
  - `crates/takusu-ical/src/lib.rs:267-328`（`parse_duration_seconds` 関数）
  - `crates/takusu-ical/src/lib.rs:311-318`（`f64` での計算と `i64` へのキャスト）
- **推奨される改善**: `jiff` の duration パーサーがあれば使用する。なければ、整数演算のみで実装し直す（`W = 7 * 86400`、`D = 86400`、`H = 3600`、`M = 60` 等を定数として使用）。`f64` での計算を避ける。
- **修正の重み**: 中

### 7.2 手動実装の日付パースと範囲チェック（`takusu-ical`）

- **問題の要約**: `parse_ymd_components` と `parse_hms_components` で文字列スライスと手動パースを行っている。`parse_hms_components` で範囲チェックを手動で行っている（0-23、0-59）。`jiff::civil::Date` を使用しているが、パース処理は自前実装。
- **該当箇所**:
  - `crates/takusu-ical/src/lib.rs:148-181`（`parse_ymd_components`、`parse_hms_components`）
  - `crates/takusu-ical/src/lib.rs:177-179`（手動範囲チェック）
- **推奨される改善**: `jiff` のパーサーを使用するか、`chrono` 等のライブラリのパーサーを使用する。手動パースを避け、ライブラリのバリデーションに委ねる。
- **修正の重み**: 中

### 7.3 `date_to_day_number` の手動実装とマジックナンバー（`takusu-habit`）

- **問題の要約**: ユリウス日変換を手動実装しており、`4800`、`12`、`3`、`365`、`32045` 等のマジックナンバーを使用している。`jiff` を使用しているが、この計算には利用していない。
- **該当箇所**:
  - `crates/takusu-habit/src/time.rs:21-28`（`date_to_day_number` 関数）
- **推奨される改善**: `jiff` に同等の機能があれば使用する。なければ、定数に名前を付け、コメントで出典を明記する。テストで既知の日付（2000-01-01 = 2451545）を検証しているのは良い。
- **修正の重み**: 小

### 7.5 `i64` と `Duration` の混用（`takusu-habit`）

- **問題の要約**: `takusu-habit/time.rs` で `Point`（`i64` の slot、`crates/takusu-core/src/lib.rs:80` で `pub struct Point(pub i64)` として既に newtype 化済み）と `jiff::Timestamp` の変換を手動で行っている。`SLOT_MINUTES` 定数を使用しているが、`.0` アクセスで直接 `i64` として演算しているため、newtype の境界が形骸化し型安全性が低い。
- **該当箇所**:
  - `crates/takusu-habit/src/time.rs:9-19`（`point_to_date`、`date_time_to_point`）
  - `crates/takusu-habit/src/time.rs:10-11`（手動での秒数計算、`checked_mul` を使用しているのは良い）
- **推奨される改善**: `Point` は既に newtype だが `.0` アクセスで直接 `i64` として演算されているため、変換メソッドを型安全にする。または `jiff::Timestamp` を直接使用し、slot への変換を一箇所に集約する。
- **修正の重み**: 中
