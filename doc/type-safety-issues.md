# takusu 型安全性改善候補まとめ

このドキュメントは、`String` や `serde_json::Value` を使いすぎていて、より型安全な Rust 抽象（enum、newtype、専用 struct）に置き換えるべき箇所を整理したものです。
各項目は「問題の要約」「現在の型」「推奨型」「修正の重み（小/中/大）」「該当ファイル・行番号」を含みます。

## 対象外・留意事項

- `ScoreWeight` や `Slots` の newtype 化は既存設計（`Point` など）でカバーされているため対象外とします。
- 単純なエラーメッセージ `String` や、自由入力の `title`/`description`/`body` フィールドは対象外です。
- 大規模な crate 分割は推奨しません。型の変更だけを行います。

---

## 1. TTS/STT プロバイダー設定の `String` 乱用（`takusu-audio` / `takusu-agent`）

### 1.1 TTS/STT backend / provider / model / voice / language が `String`

- **問題の要約**: 設定や実行時分岐で、固定値であるはずの backend 名・provider 名・モデル名・言語コードを文字列比較で扱っている。
- **現在の型**: `String` / `Option<String>`
- **推奨型**:
  - `TtsBackend`（既存） / 新規 `SttBackend`
  - `SherpaOnnxModel`（既存）
  - 新規 `ExecutionProvider`（`cpu` / `cuda` / `coreml` 等）
  - `VoiceId`、`ModelId` newtype
  - `LanguageCode` newtype（BCP-47 コード検証付き）
- **修正の重み**: 小〜中
- **該当箇所**:
  - `crates/takusu-agent/src/audio_config.rs:14-28` (`SttConfig`)
  - `crates/takusu-agent/src/audio_config.rs:72-83` (`TtsConfig`)
  - `crates/takusu-audio/src/tts.rs:44-54` (`TtsProviderConfig`)
  - `crates/takusu-audio/src/tts.rs:64-72` (`TtsOptions` / `TtsRequest`)
  - `crates/takusu-audio/src/cartesia.rs:84-91` (`CartesiaSonicConfig`)
  - `crates/takusu-audio/src/sherpa.rs:27-38` (`SherpaOnnxAsrConfig`)
  - `crates/takusu-agent/src/transport.rs:291` (`UpdateAgentTtsSettings.backend`)
  - `crates/takusu-agent/src/audio.rs:233-413`（文字列から backend/model へのマッピング）

### 1.2 Cartesia 出力フォーマットの `container` / `encoding` が `String`

- **問題の要約**: Cartesia API の container/encoding は `raw`/`wav`/`mp3` や `pcm_s16le`/`pcm_f32le` など決まった値しか存在しないが、`String` になっている。
- **現在の型**: `String`
- **推奨型**: `CartesiaContainer` / `CartesiaEncoding` enum
- **修正の重み**: 小
- **該当箇所**:
  - `crates/takusu-audio/src/cartesia.rs:20-22` (`CartesiaOutputFormat`)
  - `crates/takusu-audio/src/tts.rs:65` (`TtsOptions.response_format`)

### 1.3 API key / URL / `model_id` / `voice_id` の型なし文字列

- **問題の要約**: API key や URL、モデル/声 ID が生 `String` で扱われており、混在・誤用・流出リスクがある。
- **現在の型**: `String` / `Option<String>`
- **推奨型**:
  - `reqwest::Url` または newtype `EndpointUrl`
  - `secrecy::SecretString` または `ApiKey` newtype
  - `VoiceId` / `ModelId` newtype
- **修正の重み**: 小
- **該当箇所**:
  - `crates/takusu-audio/src/tts.rs:59-60` (`TtsConfig`)
  - `crates/takusu-audio/src/cartesia.rs:85-86` (`CartesiaSonicConfig`)
  - `crates/takusu-agent/src/audio_config.rs:76-81` (`TtsConfig.api_key_env` / `api_key`)
  - `crates/takusu-agent/src/llm.rs:41, 47` (`LlmConfig.base_url` / `api_key_env` / `api_key`)

---

## 2. `serde_json::Value` の直接使用（`takusu-agent`）

### 2.1 `Tool` trait とレジストリ全体

- **問題の要約**: ツールのパラメータスキーマ、ツール呼び出し引数、定義一覧がすべて `serde_json::Value` になっており、コンパイル時検証がない。
- **現在の型**: `serde_json::Value`
- **推奨型**:
  - `Tool` に associated type `Params: DeserializeOwned + schemars::JsonSchema` を追加
  - スキーマは `schemars` により生成
  - 呼び出しは `Params` として deserialize
- **修正の重み**: 大
- **該当箇所**:
  - `crates/takusu-agent/src/tool.rs:199-216` (`Tool` trait)
  - `crates/takusu-agent/src/tool.rs:358-360` (`ToolRegistry.definitions` / `schemas`)
  - `crates/takusu-agent/src/tool.rs:122-190` (`ProposedChange` / `InferredField` / `ChangeReceipt`)

### 2.2 LLM リクエスト/レスポンスの手作り JSON

- **問題の要約**: OpenAI 互換 API 用の `Message` / `ToolCall` / `ChatCompletionRequest` が `json!` や `Value` で組み立てられている。
- **現在の型**: `Value` / `Vec<Value>` / `String`（JSON 文字列）
- **推奨型**: 通常の `#[derive(Serialize, Deserialize)]` struct/enum（`Message`、`ToolDefinition`、`ToolCallArguments` 等）
- **修正の重み**: 中〜大
- **該当箇所**:
  - `crates/takusu-agent/src/llm.rs:139-155` (`ToolCall` / `ToolCall::to_openai`)
  - `crates/takusu-agent/src/llm.rs:211-251` (`Message` / `Message::to_openai`)
  - `crates/takusu-agent/src/llm.rs:547-610` (`ChatCompletionRequest` / `ChatCompletionResponse` / `ToolCallResponse`)

### 2.3 ミューテーション実行と引数バリデーション

- **問題の要約**: `execute_proposed_change` や `parse_habit_step` などで `serde_json::Map<String, Value>` を手動で分解している。
- **現在の型**: `Value` / `Map<String, Value>`
- **推奨型**: 各操作ごとに `CreateTaskArgs` / `UpdateHabitArgs` 等の専用 struct を定義して deserialize
- **修正の重み**: 中〜大
- **該当箇所**:
  - `crates/takusu-agent/src/lib.rs:1900-2235` (`execute_proposed_change`)
  - `crates/takusu-agent/src/lib.rs:1522-1630` (`parse_habit_step`)
  - `crates/takusu-agent/src/tools/takusu.rs:2187-2284` (`normalize_mutation_field` / `validate_mutation_args`)

### 2.4 個別ツールでの `Value` 使用

- **問題の要約**: `skills` / `progress` / `memory` / `day_details` / `rrule` / `tool_search` などのツールで、引数・スキーマ・返り値に `Value` を直接使っている。
- **現在の型**: `Value` / `Map<String, Value>` / `Option<Value>`
- **推奨型**: 各ツールに専用 `Args` struct と `Output` struct を定義
- **修正の重み**: 中
- **該当箇所**:
  - `crates/takusu-agent/src/tools/skills.rs:112, 255, 321, 432` 等
  - `crates/takusu-agent/src/tools/progress.rs:170-182` (`progress_output`)
  - `crates/takusu-agent/src/tools/memory.rs:82-91` (`make_proposal`)
  - `crates/takusu-agent/src/tools/day_details.rs:43-65`
  - `crates/takusu-agent/src/tools/rrule.rs:46-64`
  - `crates/takusu-agent/src/tools/tool_search.rs:41-61`
  - `crates/takusu-agent/src/tools/takusu.rs:561, 668, 713, 736, 772`（`format_task_json` 等、多数）

### 2.5 単純な JSON 返答

- **問題の要約**: `/health` などが `json!({"ok": true})` の `Value` を返している。
- **現在の型**: `serde_json::Value`
- **推奨型**: `#[derive(Serialize)] struct HealthResponse { ok: bool }`
- **修正の重み**: 小
- **該当箇所**:
  - `crates/takusu-agent/src/transport.rs:397-403`

---

## 3. enum 相当の `String` フィールド

### 3.1 `status`（Task）

- **問題の要約**: タスクの状態を `"pending"` / `"scheduled"` / `"in_progress"` / `"completed"` / `"skipped"` / `"overdue"` などの文字列で扱っている。
- **現在の型**: `String` / `Option<String>`
- **推奨型**: `TaskStatus` enum
- **修正の重み**: 小
- **該当箇所**:
  - `crates/takusu-contracts/src/model.rs:30, 160, 185`
  - `crates/takusu-client/src/lib.rs:1244, 1331, 1352`
  - `crates/takusu-worker/src/models.rs:26, 113, 135`
  - `crates/takusu-local/src/handlers/task.rs:23` (`TaskQueryParams`)
  - `crates/takusu-cli/src/main.rs:508-509`
  - `crates/takusu-agent/src/tools/takusu.rs:455-464` (`normalize_status`)

### 3.2 `window_mode`（Habit）

- **問題の要約**: Habit のウィンドウモードが `"day"` / `"period"` の文字列になっている。
- **現在の型**: `String` / `Option<String>`
- **推奨型**: `WindowMode` enum
- **修正の重み**: 小
- **該当箇所**:
  - `crates/takusu-contracts/src/model.rs:220, 246, 301, 409`
  - `crates/takusu-client/src/lib.rs:1384, 1409, 1439`
  - `crates/takusu-worker/src/models.rs:154, 179, 209`
  - `crates/takusu-local-lib/src/app.rs:106-112` (`validate_window_mode`)
  - `crates/takusu-cli/src/main.rs:611, 648, 688`

### 3.3 `solver`（Settings）

- **問題の要約**: `"sa"` / `"priority"` / `"auto"` の solver 名が文字列になっている。`takusu-core` には既に `Solver` enum がある。
- **現在の型**: `String` / `Option<String>`
- **推奨型**: `takusu_core::Solver`（serde アダプタで DB/JSON 互換）
- **修正の重み**: 小
- **該当箇所**:
  - `crates/takusu-contracts/src/model.rs:550, 701`
  - `crates/takusu-client/src/lib.rs:1917, 1945`
  - `crates/takusu-worker/src/models.rs:397-401`
  - `crates/takusu-local-lib/src/app.rs:560-571` (`parse_solver`)

### 3.4 `recurrence`（Habit）

- **問題の要約**: 繰り返しルールが JSON テキスト文字列で保存・受け渡しされている。`takusu_habit` には `RecurrenceRule` がある。
- **現在の型**: `String`
- **推奨型**: `takusu_habit::RecurrenceRule`（serde JSON 文字列アダプタで `sqlx` 対応）
- **修正の重み**: 中
- **該当箇所**:
  - `crates/takusu-contracts/src/model.rs:202, 230, 280, 393`
  - `crates/takusu-client/src/lib.rs:1369, 1394, 1419`
  - `crates/takusu-worker/src/models.rs:139, 164, 189`
  - `crates/takusu-local-lib/src/app.rs:722-723, 1693-1694`（`serde_json::from_str`）

### 3.5 `scope`（Token）

- **問題の要約**: JWT scope が `"root"` / `"read-write"` の文字列になっている。
- **現在の型**: `String`
- **推奨型**: `TokenScope` enum
- **修正の重み**: 小
- **該当箇所**:
  - `crates/takusu-contracts/src/model.rs:486, 498`
  - `crates/takusu-client/src/lib.rs:1675, 1687`
  - `crates/takusu-worker/src/models.rs:334, 346`
  - `crates/takusu-util/src/jwt.rs:43-62` (`Claims.scope`)

### 3.6 `kind` / `subject_type`（Memory）

- **問題の要約**: Memory の `kind`（`proper_noun` / `fact` / `task_note` 等）と `subject_type`（`task` / `habit` / `skill` / `schedule` 等）が文字列になっている。
- **現在の型**: `String` / `Option<String>`
- **推奨型**: `MemoryKind` / `SubjectType` enum
- **修正の重み**: 小
- **該当箇所**:
  - `crates/takusu-contracts/src/model.rs:599, 632, 636, 654, 656`
  - `crates/takusu-client/src/lib.rs:1772, 1788, 1792, 1812`
  - `crates/takusu-worker/src/models.rs:454, 484`
  - `crates/takusu-local-lib/src/app.rs:162-183`（バリデーション）
  - `crates/takusu-agent/src/tools/memory.rs:264, 275`
  - `crates/takusu-local-lib/src/storage_sqlite.rs:1665-1737`

### 3.7 `operation` / `target_type`（Agent ツール変更レシート）

- **問題の要約**: ツール実行の `ProposedChange` / `ChangeReceipt` で操作種別と対象種別が文字列になっており、`execute_proposed_change` で文字列 prefix マッチで分岐している。
- **現在の型**: `String`
- **推奨型**:
  - `ChangeOperation` enum（`create` / `update` / `delete` / `generate` / `move` 等）
  - `TargetType` enum または `Target { kind: TargetType, display_id: ... }`
- **修正の重み**: 中
- **該当箇所**:
  - `crates/takusu-agent/src/tool.rs:124, 167-168`
  - `crates/takusu-agent/src/lib.rs:1917-1975`（`target_label` prefix 判定）
  - `crates/takusu-agent/src/tools/takusu.rs:1641-2173`（`operation` / `target_label` 生成）
  - `crates/takusu-agent/src/tools/progress.rs:198`（`operation` / `target_label`）

### 3.8 `Reschedule.mode` / `GenerateSchedule.sleep`

- **問題の要約**: スケジュール再生成の `mode`（`"full"` / `"tasks"` / `"range"`）と `sleep`（`"recommended"` / `"disabled"` / `"HH:MM-HH:MM"`）が文字列になっている。
- **現在の型**: `String`
- **推奨型**: `RescheduleMode` enum、`SleepInput` enum または `SleepWindow` struct
- **修正の重み**: 小〜中
- **該当箇所**:
  - `crates/takusu-client/src/lib.rs:1602, 1632, 1641, 1652` (`GenerateSchedule.sleep` / `Reschedule.mode` / `Reschedule.sleep`)
  - `crates/takusu-local-lib/src/app.rs:496-518` (`parse_sleep`)
  - `crates/takusu-local-lib/src/app.rs:2278-2324` (`mode` 文字列マッチ)

### 3.9 `language`（Audio / STT / TTS）

- **問題の要約**: 言語指定が文字列になっており、有効な BCP-47 コードかどうかの検証が分散している。
- **現在の型**: `String` / `Option<String>`
- **推奨型**: `LanguageCode` newtype（BCP-47 検証付き）
- **修正の重み**: 小
- **該当箇所**:
  - `crates/takusu-agent/src/audio_config.rs:16, 80`
  - `crates/takusu-audio/src/tts.rs:51` (`TtsProviderConfig.language`)
  - `crates/takusu-audio/src/cartesia.rs:90` (`CartesiaSonicConfig.language`)
  - `crates/takusu-audio/src/sherpa.rs:35` (`SherpaOnnxAsrConfig.language`)

---

## 4. newtype 候補となるプリミティブ

### 4.1 `abandonability`

- **問題の要約**: タスク/習スク/習慣の「諦めやすさ」が `[0, 1]` のはずの `f64` で扱われており、範囲外値や NaN の可能性がある。
- **現在の型**: `f64`
- **推奨型**: `Abandonability` newtype（コンストラクタで clamp）
- **修正の重み**: 小
- **該当箇所**:
  - `crates/takusu-core/src/lib.rs:298` (`Task.abandonability`)
  - `crates/takusu-habit/src/lib.rs:83` (`Habit.abandonability`)
  - `crates/takusu-contracts/src/model.rs:29, 211, 348` (`TaskRow` / `HabitRow` / `HabitStepRow`)
  - `crates/takusu-client/src/lib.rs:1243, 1378, 1481`
  - `crates/takusu-worker/src/models.rs:25, 148, 251`
  - `crates/takusu-cli/src/main.rs:399-400, 482, 599-600, 676-677`

### 4.2 `quantity`（数量）

- **問題の要約**: タスクの数量が `i64` または `Option<String>`（単位）で扱われており、負値や単位混在のリスクがある。
- **現在の型**: `i64` / `Option<i64>` / `Option<String>`
- **推奨型**: `Quantity`（非負 i64）と `QuantityUnit` newtype
- **修正の重み**: 小
- **該当箇所**:
  - `crates/takusu-contracts/src/model.rs:43-49, 741, 769`
  - `crates/takusu-client/src/lib.rs:1254-1258, 1340-1346`
  - `crates/takusu-worker/src/models.rs:36-46, 81-87, 123-129`
  - `crates/takusu-cli/src/main.rs:542, 555`
  - `crates/takusu-agent/src/tools/progress.rs:415-563`

### 4.3 `time_budget_ms`

- **問題の要約**: 求解時間上限がミリ秒 `i64` で保存され、`local-lib` で `Duration` に変換している。
- **現在の型**: `Option<i64>`
- **推奨型**: `Option<std::time::Duration>` または `Option<jiff::SignedDuration>`（serde アダプタ付き）
- **修正の重み**: 小
- **該当箇所**:
  - `crates/takusu-contracts/src/model.rs:553, 704`
  - `crates/takusu-client/src/lib.rs:1920, 1948`
  - `crates/takusu-worker/src/models.rs:401`
  - `crates/takusu-local-lib/src/app.rs:577-581`

### 4.4 分 ↔ スロットの変換

- **問題の要約**: 1 スロット = 5 分という変換が `avg_minutes / 5` などいたる所に散らばっており、マジックナンバーが重複している（`.devin/docs/code-style.md`「`point_to_iso` hardcoded 5-minute slots」も参照）。
- **現在の型**: `i64` / `u64`（分やスロットの区別なし）
- **推奨型**:
  - `Minutes` newtype（`.to_slots()` / `.from_slots()` 提供）
  - `NormalDist::from_minutes(avg: Minutes, sigma: Minutes)` 等
  - `SLOT_MINUTES` 定数を単一箇所に集約
- **修正の重み**: 中
- **該当箇所**:
  - `crates/takusu-contracts/src/model.rs`（`avg_minutes` / `sigma_minutes` / `active_minutes` 等）
  - `crates/takusu-client/src/lib.rs`（同上）
  - `crates/takusu-worker/src/models.rs`（同上）
  - `crates/takusu-local-lib/src/app.rs:353, 385, 527-529, 577, 727-734`
  - `crates/takusu-util/src/lib.rs:261-324` (`parse_duration`)
  - `crates/takusu-cli/src/main.rs:392-398, 436-438, 475-481, 592-598, 669-675`
  - `crates/takusu-core/src/lib.rs:127-138` (`NormalDist` in slots)
  - `crates/takusu-habit/src/time.rs:6-10` (`SLOT_MINUTES`)

### 4.5 `comfortable_minutes` / `maximum_minutes` / `sleep_minutes_*`

- **問題の要約**: 作業負荷・睡眠時間などの「分」が素の `i64` になっている。
- **現在の型**: `Option<i64>` / `i64`
- **推奨型**: `Minutes` newtype
- **修正の重み**: 小
- **該当箇所**:
  - `crates/takusu-contracts/src/model.rs:545-547, 733`
  - `crates/takusu-client/src/lib.rs:1911-1920, 1939-1948`
  - `crates/takusu-local-lib/src/app.rs:523-550` (`parse_workload`)
  - `crates/takusu-local-lib/src/app.rs:788-792` (`SchedulePreviewOutput`)

### 4.6 `request_timeout_seconds`

- **問題の要約**: LLM リクエストタイムアウトが秒数 `u64` になっている。
- **現在の型**: `u64`
- **推奨型**: `std::time::Duration`
- **修正の重み**: 小
- **該当箇所**:
  - `crates/takusu-agent/src/llm.rs:57, 313-318`

---

## 5. 日付・時刻の `String` 化

### 5.1 ISO 8601 タイムスタンプ

- **問題の要約**: タスク/習慣/スケジュール/メモリ/トークン等の `start_at` / `end_at` / `completed_at` / `created_at` / `updated_at` / `expires_at` 等が ISO 8601 文字列で扱われている。
- **現在の型**: `String` / `Option<String>`
- **推奨型**: `jiff::Timestamp`（RFC3339 serde アダプタ付き）
- **修正の重み**: 中〜大
- **該当箇所**（代表例。`takusu-client` / `takusu-worker` でも同一構造がミラーされている）:
  - `crates/takusu-contracts/src/model.rs:20-21, 52-53, 63-64, 222-223, 319-320, 353, 461-462, 489-491, 560-561, 610-612, 671-673, 722-725, 732`
  - `crates/takusu-client/src/lib.rs:1234-1235, 1267-1268, 1367-1370, 1392-1396, 1451-1455, 1485, 1615-1618, 1622-1625, 1677-1690`
  - `crates/takusu-worker/src/models.rs:16-50, 155, 224-225, 255, 313-314, 337-340, 458-459`
  - `crates/takusu-local-lib/src/app.rs:799-800` (`MoveEntryOutput`)
  - `crates/takusu-agent/src/llm.rs:34` (`LlmProviderConfig.models_fetched_at`)
  - `crates/takusu-agent/src/tool.rs:134` (`ProposedChange.observed_updated_at`)

### 5.2 `HH:MM` 時刻

- **問題の要約**: Habit の `start_time` / `end_time` や Settings の `sleep_start` / `sleep_end` が `HH:MM` 文字列で扱われている。`takusu_habit::TimeOfDay` が既に存在する。
- **現在の型**: `String`
- **推奨型**: `takusu_habit::TimeOfDay`（Serialize/Deserialize derive 追加）
- **修正の重み**: 中
- **該当箇所**:
  - `crates/takusu-contracts/src/model.rs:203-204, 231-232, 282-283, 340-341, 394-395, 542-543`
  - `crates/takusu-client/src/lib.rs:1369-1370, 1394-1396, 1420-1422, 1473-1474`
  - `crates/takusu-worker/src/models.rs:140-141, 165-166, 190-191, 243-244`
  - `crates/takusu-habit/src/time.rs:12-38`（既存 `TimeOfDay`）
  - `crates/takusu-local-lib/src/app.rs:103-112` (`validate_hhmm`)

### 5.3 `YYYY-MM-DD` 日付

- **問題の要約**: Habit scheduled span の `start_date` / `end_date` が文字列になっている。
- **現在の型**: `String`
- **推奨型**: `jiff::civil::Date`
- **修正の重み**: 小
- **該当箇所**:
  - `crates/takusu-contracts/src/model.rs:316-317, 324-325`
  - `crates/takusu-client/src/lib.rs:1451-1452, 1459-1460`
  - `crates/takusu-worker/src/models.rs:221-222, 229-230`

### 5.4 timezone

- **問題の要約**: タイムゾーン名が生文字列になっており、毎回 `parse_settings_timezone` で検証している。
- **現在の型**: `String`
- **推奨型**: `jiff::tz::TimeZone`（serde アダプタ付き）
- **修正の重み**: 小
- **該当箇所**:
  - `crates/takusu-contracts/src/model.rs:541`
  - `crates/takusu-client/src/lib.rs:1908`
  - `crates/takusu-worker/src/models.rs:389`
  - `crates/takusu-local-lib/src/app.rs:257-268` (`parse_settings_timezone`)

### 5.5 JSON 文字列化された `schedule` / `depends` / `depends_on`

- **問題の要約**: `ScheduleRow.schedule`、`TaskRow.depends`、`HabitStepRow.depends_on` などが JSON 配列を文字列として保持している。
- **現在の型**: `String` / `Option<String>`
- **推奨型**: `Vec<ScheduleEntry>` / `Vec<String>` にして `sqlx::types::Json` または serde アダプタで保存
- **修正の重み**: 小
- **該当箇所**:
  - `crates/takusu-contracts/src/model.rs:24, 352, 462`
  - `crates/takusu-client/src/lib.rs:1238, 1484, 1617`
  - `crates/takusu-worker/src/models.rs:20, 254, 313`
  - `crates/takusu-local-lib/src/storage_sqlite.rs:99, 571, 1214, 2549`

### 5.6 Token / JWT 時刻表現の不統一

- **問題の要約**: `TokenRow` の時刻は ISO 8601 文字列だが、`Claims.iat` / `exp` は Unix 秒 `i64` になっており、表現が統一されていない。
- **現在の型**: `String`（ISO） / `i64`（Unix seconds）
- **推奨型**: 統一して `jiff::Timestamp`（serde アダプタで ISO / Unix 両対応）
- **修正の重み**: 小
- **該当箇所**:
  - `crates/takusu-contracts/src/model.rs:489-491`
  - `crates/takusu-client/src/lib.rs:1677-1690`
  - `crates/takusu-util/src/jwt.rs:52-55`

---

## 6. タプルの newtype 化（`takusu-core`）

### 6.1 `Plan.schedules` / `Placement` / `previous_schedule`

- **問題の要約**: `Plan.schedules` が `Vec<(Point, Point, usize)>`、`Placement` が type alias、`Planner.previous_schedule` が `Vec<Option<(Point, Point)>>` となっており、要素の意味（開始 slot / 終了 slot / task index）が型では表現されていない。
- **現在の型**: `Vec<(Point, Point, usize)>` / `&[(Point, Point, usize)]` / `Vec<Option<(Point, Point)>>`
- **推奨型**:
  - `TaskPlacement { start: Point, end: Point, task_id: usize }`
  - `TimeWindow { start: Point, end: Point }`
- **修正の重み**: 中
- **該当箇所**:
  - `crates/takusu-core/src/placement.rs:7` (`Placement` alias)
  - `crates/takusu-core/src/lib.rs:318-320` (`Plan.schedules`)
  - `crates/takusu-core/src/lib.rs:423` (`Planner.previous_schedule`)
  - `crates/takusu-core/src/lib.rs:502` (`set_previous_schedule`)
  - `crates/takusu-core/src/lib.rs:557, 575` (`plan_partial` / `plan_in_range`)
  - `crates/takusu-core/src/solver.rs:46, 56` (`solve_partial` / `solve_partial_with_seed`)
  - `crates/takusu-core/src/anneal.rs`（多数、`Placement` や `&(Point, Point)` の利用箇所）
  - `crates/takusu-core/src/decoder.rs:33, 93, 595`

### 6.2 `habit_entries`（`Vec<(usize, i64)>`）

- **問題の要約**: `evaluate.rs` などで habit group index と anchor slot を表すタプル `(usize, i64)` が散らばっている。
- **現在の型**: `Vec<(usize, i64)>`
- **推奨型**: `HabitAnchor` または `HabitGroupAnchor` newtype
- **修正の重み**: 小
- **該当箇所**:
  - `crates/takusu-core/src/placement.rs:20` (`INSERTION_HABIT`)
  - `crates/takusu-core/src/evaluate.rs:141, 166, 620`

---

## 7. その他

### 7.1 `SimilarTaskRow.similarity`

- **問題の要約**: 類似度が `"dice:0.xxx"` のような文字列になっている。
- **現在の型**: `String`
- **推奨型**: `Similarity { metric: SimilarityMetric, score: f64 }` または `Similarity` newtype
- **修正の重み**: 小
- **該当箇所**:
  - `crates/takusu-contracts/src/model.rs:674`
  - `crates/takusu-local-lib/src/storage_sqlite.rs:1996`

### 7.2 `Reschedule.mode` / `GenerateSchedule.sleep`

- 3.8 項に同じ。

---

## 8. Trait ベースのリファクタリング設計

本章では、既存の型安全性問題を解決するために導入すべき Trait ベースの抽象化を設計します。各設計は「問題の要約」「提案する trait/struct」「適用先ファイル・行番号」「修正の重み」を含みます。


### 8.1 `Tool` trait の型安全化（`takusu-agent`）

#### 問題の要約
- 既存項目: 2.1 `Tool` trait とレジストリ全体
- 現在、`Tool::parameters_schema()` が `serde_json::Value` を返し、`Tool::call()` も `Value` を引数に取るため、コンパイル時の型検証がない。
- 各ツール実装で手動で `Value` をパース・検証しており、エラーが実行時まで遅延する。

#### 提案する trait / struct

> **制約（監査で判明）**
>
> - `ToolRegistry` は `tools: HashMap<String, Box<dyn Tool>>` でツールを保持している（`crates/takusu-agent/src/tool.rs:250`）。
>   したがって **`Tool` 本体に associated type を足すと object safety が壊れ、`Box<dyn Tool>` がコンパイルできない**。
> - `ToolOutput` は **既に struct として存在する**（`crates/takusu-agent/src/tool.rs:180-197`）。同名の trait は定義できない。
> - `schemars` はワークスペースの依存に **入っていない**（`Cargo.lock` に transitively 存在するのみ）。導入する場合は明示的に追加する。
>
> このため、object-safe な `Tool` を残したまま、型付き層を上に重ねる二層構造にする。

```rust
use schemars::JsonSchema;
use serde::{Serialize, de::DeserializeOwned};

/// 型付きツールの引数。
pub trait ToolArgs: DeserializeOwned + JsonSchema + Send + Sync {
    /// 引数のバリデーション（必要に応じてオーバーライド）。
    fn validate(&self) -> Result<(), InvalidArgsError> {
        Ok(())
    }
}

/// 型付きツール。associated type を持つので object-safe ではない。
/// 実装側はこちらだけを書く。
#[async_trait::async_trait]
pub trait TypedTool: Send + Sync {
    type Params: ToolArgs;

    fn name(&self) -> &'static str;
    fn description(&self) -> &'static str;

    fn exposure(&self) -> ToolExposure {
        ToolExposure::Direct
    }

    async fn call_typed(&self, args: Self::Params) -> Result<ToolOutput, ToolError>;

    async fn call_typed_with_id(
        &self,
        _id: &str,
        args: Self::Params,
    ) -> Result<ToolOutput, ToolError> {
        self.call_typed(args).await
    }
}

/// `TypedTool` を実装した型は、自動的に object-safe な `Tool` になる。
/// `ToolRegistry` は従来どおり `Box<dyn Tool>` を保持できる。
#[async_trait::async_trait]
impl<T: TypedTool> Tool for T {
    fn name(&self) -> &'static str {
        TypedTool::name(self)
    }

    fn description(&self) -> &'static str {
        TypedTool::description(self)
    }

    fn exposure(&self) -> ToolExposure {
        TypedTool::exposure(self)
    }

    /// JSON Schema は schemars で自動生成する。
    fn parameters_schema(&self) -> Value {
        serde_json::to_value(schemars::schema_for!(T::Params)).unwrap_or_else(|_| json!({}))
    }

    async fn call(&self, args: Value) -> Result<ToolOutput, ToolError> {
        let params: T::Params = serde_json::from_value(args)
            .map_err(|e| ToolError::InvalidArgs(InvalidArgsError::no_field(e.to_string())))?;
        params.validate().map_err(ToolError::InvalidArgs)?;
        self.call_typed(params).await
    }

    async fn call_with_id(&self, id: &str, args: Value) -> Result<ToolOutput, ToolError> {
        let params: T::Params = serde_json::from_value(args)
            .map_err(|e| ToolError::InvalidArgs(InvalidArgsError::no_field(e.to_string())))?;
        params.validate().map_err(ToolError::InvalidArgs)?;
        self.call_typed_with_id(id, params).await
    }
}
```

**注意点**

- `impl<T: TypedTool> Tool for T` は blanket impl なので、**既存の手書き `impl Tool for XxxTool` と衝突する**。
  移行は「1 ツールずつ `Tool` → `TypedTool` へ書き換える」ことができず、全ツールを一度に切り替える必要がある。
  段階移行したい場合は blanket impl をやめ、`TypedTool` を包むラッパ `struct Typed<T>(T)` を用意して
  `impl<T: TypedTool> Tool for Typed<T>` とし、`registry.register(Box::new(Typed(tool)))` で登録する。
- `ToolOutput`（既存 struct）はそのまま戻り値に使う。出力側の型付けは本項の対象外とし、必要なら別途
  `ToolOutput.content` を生成する `Serialize` な struct を各ツールに用意する（2.4 参照）。
- `schemars` 導入時は、`#[derive(JsonSchema)]` の説明文が既存の手書きスキーマ（`description` や
  `additionalProperties: false`）と一致するかを検証すること。LLM に渡すスキーマが変わると挙動が変わる。

#### 適用先ファイル・行番号
- `crates/takusu-agent/src/tool.rs:199-231` (`Tool` trait 定義)
- `crates/takusu-agent/src/tool.rs:249-253` (`ToolRegistry.tools` が `Box<dyn Tool>`)
- `crates/takusu-agent/src/tool.rs:180-197` (既存 `ToolOutput` struct)
- `crates/takusu-agent/src/tools/*.rs` (各ツール実装全体)

#### 前提作業
- `schemars` をワークスペース依存に追加する。

#### 修正の重み
- **大** (Tool trait 全体と全ツール実装に影響)


### 8.2 enum/newtype の DB/JSON/文字列相互変換を統一する trait

#### 問題の要約
- 既存項目: 3.1–3.7 (status, window_mode, solver, scope, kind, recurrence, operation/target_type)
- `takusu-contracts` / `takusu-client` / `takusu-worker` で同一の enum 相当フィールドが `String` で重複定義されている。
- DB 保存時・JSON シリアライズ時・API リクエスト時でそれぞれ文字列変換が分散している。

#### 提案する trait / struct

> **制約（監査で判明）**
>
> - `takusu-contracts` の sqlx は `default-features = false, features = ["derive", "macros"]`。
>   **ドライバ（`sqlite`）を有効にしていない**ため、`Type<Sqlite>` / `Encode<Sqlite>` / `Decode<Sqlite>` の
>   手書き実装はこの crate ではできない。使えるのは `#[derive(sqlx::FromRow)]` と、その
>   `#[sqlx(try_from = "String")]` 属性まで。
> - sqlx 0.9 に `#[sqlx(try_from = ..., with = ...)]` という併用構文は存在しない。`with` は使わない。
> - `HasValueRef` に依存した汎用アダプタは sqlx 0.9 の API と合わない。汎用 `enum_sqlx` モジュールは作らず、
>   **`TryFrom<String>` の実装 + `#[sqlx(try_from = "String")]` で済ませる**。
> - `EnumLabel::default()` は `Default::default()` と曖昧になるため `enum_default()` にする。
> - `all_variants() -> &'static [Self]` には `Self: Sized` が要る。

```rust
// 置き場所: takusu-util（takusu-contracts / takusu-client / takusu-worker がいずれも依存しており、
// wasm でもビルドできる唯一の共有 crate）

use serde::{Deserialize, Deserializer, Serializer};

/// enum を DB/JSON/文字列で一貫して扱うための trait。
pub trait EnumLabel:
    Sized
    + Clone
    + Copy
    + PartialEq
    + Eq
    + std::fmt::Display
    + std::str::FromStr
    + Send
    + Sync
    + 'static
{
    /// DB が NULL / 未知値だったときのフォールバック。
    /// `Default` と衝突しないよう別名にする。
    fn enum_default() -> Self;

    /// 全バリアント（エラーメッセージや API のバリデーションに使う）。
    fn all_variants() -> &'static [Self];
}

/// serde 用の文字列アダプタ。`#[serde(with = "enum_serde")]` で使う。
pub mod enum_serde {
    use super::EnumLabel;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S, T>(value: &T, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
        T: EnumLabel,
    {
        serializer.collect_str(value)
    }

    pub fn deserialize<'de, D, T>(deserializer: D) -> Result<T, D::Error>
    where
        D: Deserializer<'de>,
        T: EnumLabel,
        <T as std::str::FromStr>::Err: std::fmt::Display,
    {
        let s = String::deserialize(deserializer)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

/// 定型の `Display` / `FromStr` / `EnumLabel` / `TryFrom<String>` をまとめて生成するマクロ。
/// これで各 enum の記述量を 1/4 程度にできる。
#[macro_export]
macro_rules! enum_label {
    (
        $(#[$meta:meta])*
        $vis:vis enum $name:ident {
            $(#[default])? $first:ident = $first_s:literal,
            $($variant:ident = $s:literal),* $(,)?
        }
    ) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        $vis enum $name {
            $first,
            $($variant),*
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(match self {
                    Self::$first => $first_s,
                    $(Self::$variant => $s),*
                })
            }
        }

        impl std::str::FromStr for $name {
            type Err = $crate::UnknownLabel;
            fn from_str(s: &str) -> Result<Self, Self::Err> {
                match s {
                    $first_s => Ok(Self::$first),
                    $($s => Ok(Self::$variant),)*
                    other => Err($crate::UnknownLabel::new(stringify!($name), other)),
                }
            }
        }

        impl TryFrom<String> for $name {
            type Error = $crate::UnknownLabel;
            fn try_from(s: String) -> Result<Self, Self::Error> {
                s.parse()
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::$first
            }
        }

        impl $crate::EnumLabel for $name {
            fn enum_default() -> Self {
                Self::$first
            }
            fn all_variants() -> &'static [Self] {
                &[Self::$first, $(Self::$variant),*]
            }
        }
    };
}

/// 使用例: TaskStatus
enum_label! {
    pub enum TaskStatus {
        #[default] Pending = "pending",
        Scheduled = "scheduled",
        InProgress = "in_progress",
        Completed = "completed",
        Skipped = "skipped",
        Overdue = "overdue",
    }
}

/// Row 側での使い方。`with = "enum_sqlx"` のような併用構文は存在しないので使わない。
/// `TryFrom<String>` はマクロが生成済みなので `try_from` だけで足りる。
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct TaskRow {
    // ...
    #[serde(with = "takusu_util::enum_serde")]
    #[sqlx(try_from = "String")]
    pub status: TaskStatus,
    // ...
}
```

**`Solver` / `RecurrenceRule` の扱い（依存グラフの制約）**

- `takusu-core` は **serde に依存していない**（`serde` は dev-dependencies のみ）。
  そのため既存の `takusu_core::Solver` をそのまま Row / Request 型に使うことはできない。
  取れる手は次の 3 つ。
  1. `takusu-core` に `serde` を optional feature で足し、`Solver` に derive する（core を汚す）。
  2. `takusu-util` に `Solver` を `enum_label!` で定義し、`takusu-local-lib` の境界で
     `takusu_core::Solver` へ変換する（**推奨**。core の依存を増やさない）。
  3. 現状の `parse_solver` を残す（改善なし）。
- `takusu_habit::RecurrenceRule` は serde derive 済みだが、`takusu-contracts` / `takusu-client` /
  `takusu-worker` はいずれも `takusu-habit` に依存していない。
  依存を足すと `takusu-core` まで引きずり込むことになり、とくに wasm ターゲットの
  `takusu-worker` には重い。**3.4 は当面 `String` のままとし、`takusu-local-lib` の境界でのみ
  `RecurrenceRule` に変換する**のが現実的。

#### 適用先ファイル・行番号
- `crates/takusu-contracts/src/model.rs:30, 160, 185` (TaskRow.status)
- `crates/takusu-contracts/src/model.rs:220, 246, 301, 409` (HabitRow.window_mode)
- `crates/takusu-contracts/src/model.rs:550, 701` (SettingsRow.solver)
- `crates/takusu-contracts/src/model.rs:486, 498` (TokenRow.scope)
- `crates/takusu-contracts/src/model.rs:599, 632, 636, 654, 656` (MemoryRow.kind/subject_type)
- `crates/takusu-client/src/lib.rs` (対応するフィールド全体)
- `crates/takusu-worker/src/models.rs` (対応するフィールド全体)

#### 前提作業
- `takusu-util` に `EnumLabel` / `enum_serde` / `enum_label!` / `UnknownLabel` を追加する。

#### 修正の重み
- **中** (enum 定義と serde アダプタの追加、各 Row 型のフィールド型変更)


### 8.3 入力値検証を統一する `Validate` trait

#### 問題の要約
- 既存項目: 4.1–4.6 (abandonability, quantity, time_budget_ms, 分↔スロット変換)
- `takusu-local-lib/src/app.rs` に `validate_minutes`, `validate_title`, `validate_recurrence`, `validate_window_mode`, `validate_skill`, `validate_memory`, `validate_hhmm`, `validate_timezone` など多数の検証関数が散在している。
- 各検証ロジックが関数として分離されており、struct に紐付いていないため、再利用が困難。

#### 提案する trait / struct

```rust
use crate::error::AppError;

/// 入力値のバリデーション trait
pub trait Validate {
    fn validate(&self) -> Result<(), AppError>;
}

/// コンテキスト付きバリデーション（DB 既存値との比較など）
pub trait ValidateWithContext<C> {
    fn validate_with_context(&self, ctx: &C) -> Result<(), AppError>;
}

/// 分単位の値を検証する newtype
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Minutes(i64);

impl Minutes {
    pub const MAX: i64 = 60 * 24 * 365; // 約1年

    pub fn new(value: i64) -> Result<Self, AppError> {
        if value < 0 {
            return Err(AppError::BadRequest(format!(
                "minutes must be >= 0 (got {value})"
            )));
        }
        if value > Self::MAX {
            return Err(AppError::BadRequest(format!(
                "minutes must be at most {} (got {value})",
                Self::MAX
            )));
        }
        Ok(Self(value))
    }

    pub fn as_i64(self) -> i64 {
        self.0
    }
}

impl Validate for Minutes {
    fn validate(&self) -> Result<(), AppError> {
        Self::new(self.0).map(|_| ())
    }
}

impl serde::Serialize for Minutes {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        self.0.serialize(serializer)
    }
}

impl<'de> serde::Deserialize<'de> for Minutes {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = i64::deserialize(deserializer)?;
        Self::new(value).map_err(serde::de::Error::custom)
    }
}

/// 使用例: CreateTask に Validate を実装
impl Validate for CreateTask {
    fn validate(&self) -> Result<(), AppError> {
        validate_title(&self.title)?;
        let avg = Minutes::new(self.avg_minutes)?;
        if let Some(sigma) = self.sigma_minutes {
            Minutes::new(sigma)?;
        }
        validate_task_datetimes(
            self.start_at.as_deref(),
            &self.end_at,
            None,
            None,
        )?;
        Ok(())
    }
}

/// 既存の validate_* 関数を trait メソッドに統合
impl Validate for CreateHabit {
    fn validate(&self) -> Result<(), AppError> {
        validate_title(&self.title)?;
        validate_recurrence(&self.recurrence)?;
        validate_hhmm(&self.start_time)?;
        validate_hhmm(&self.end_time)?;
        if let Some(mode) = &self.window_mode {
            validate_window_mode(mode)?;
        }
        Ok(())
    }
}
```

#### 適用先ファイル・行番号
- `crates/takusu-local-lib/src/app.rs:51-82` (validate_minutes)
- `crates/takusu-local-lib/src/app.rs:89-93` (validate_title)
- `crates/takusu-local-lib/src/app.rs:97-101` (validate_recurrence)
- `crates/takusu-local-lib/src/app.rs:106-114` (validate_window_mode)
- `crates/takusu-local-lib/src/app.rs:117-158` (validate_skill)
- `crates/takusu-local-lib/src/app.rs:161-190` (validate_memory)
- `crates/takusu-local-lib/src/app.rs:193-195` (validate_hhmm)
- `crates/takusu-local-lib/src/app.rs:260-268` (validate_timezone)
- `crates/takusu-local-lib/src/storage_sqlite.rs:2802-2820` (validate_quantity)

#### 修正の重み
- **中** (trait 定義と各 struct への実装追加、既存関数の置き換え)


### 8.4 時間・slot 変換を newtype に集約する

#### 問題の要約
- 既存項目: 4.4 分 ↔ スロットの変換
- 1 スロット = 5 分という変換が `avg_minutes / 5` など各所に散らばり、マジックナンバーが重複している。
- `takusu-habit/src/time.rs:6` に `SLOT_MINUTES: i64 = 5` があるが、他 crate からは参照されておらず、
  `takusu-agent/src/tools/rrule.rs:179` では **同じ定数がローカルに再定義されている**。

> **制約（監査で判明）**
>
> - `takusu-core` 側は実は 5 をハードコードしていない。`Point::from_timestamp(ts, per: u16)` と
>   `Planner.per` で分/スロットを引数化している（`crates/takusu-core/src/lib.rs:78`）。
>   ハードコードされているのは **`takusu-habit` / `takusu-agent` / `takusu-local-lib` / CLI 側**。
>   したがって 4.4 の主眼は「core を直す」ではなく「core の外の重複定数を 1 箇所に寄せる」こと。
> - `Point` は `takusu-core` 定義。`AsMinutes` を `takusu-util` に置くと **orphan rule 違反**で
>   `impl AsMinutes for Point` が書けない。
> - `impl AsSlots for i64` は primitive への外部 trait 実装であり、trait を定義した crate 以外では
>   書けない。trait と impl を同じ crate に置く場合のみ可能だが、`i64` 全体に生やすのは
>   単位の取り違えを助長するので **やらない**。

```rust
// 置き場所: takusu-core（Point と同じ crate に置けば orphan rule を回避できる）。
// takusu-core は serde に依存していないので、newtype に serde derive は付けない。
// serde が要る層（storage / client / worker）では i64 のまま持ち、境界で変換する。

/// 分。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Minutes(pub i64);

/// スロット数。1 スロット = `SLOT_MINUTES` 分。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Slots(pub i64);

/// 唯一の定義。takusu-habit / takusu-agent のローカル定数はこれを参照するよう置き換える。
pub const SLOT_MINUTES: i64 = 5;

impl Minutes {
    /// 切り捨て。端数を切り上げたい場合は `to_slots_ceil` を使う。
    pub const fn to_slots(self) -> Slots {
        Slots(self.0 / SLOT_MINUTES)
    }

    pub const fn to_slots_ceil(self) -> Slots {
        Slots(self.0.div_euclid(SLOT_MINUTES) + i64::from(self.0.rem_euclid(SLOT_MINUTES) != 0))
    }
}

impl Slots {
    pub const fn to_minutes(self) -> Minutes {
        Minutes(self.0 * SLOT_MINUTES)
    }
}

impl Point {
    /// エポックからの経過分。
    pub const fn minutes_since_epoch(self) -> Minutes {
        Minutes(self.0 * SLOT_MINUTES)
    }
}

impl NormalDist {
    /// 分から構築する。負値は呼び出し側で弾く。
    pub fn from_minutes(avg: Minutes, sigma: Minutes) -> Self {
        Self {
            avg: avg.to_slots().0.max(0) as u64,
            sigma: sigma.to_slots().0.max(0) as u64,
        }
    }
}
```

**なぜ trait ではなく inherent method か**

`AsMinutes` / `AsSlots` のような trait は、実装対象が `Minutes` / `Slots` / `Point` の 3 つしかなく、
かつすべて同じ crate に置くことになるため、trait にする利点がない。use を強制するぶん不便になる。
**trait 化はやめ、inherent method で提供する**のが素直。

なお `Planner.per` が 5 以外を取りうる設計になっているため、`SLOT_MINUTES` を定数で固定してよいのは
`takusu-core` の外側（habit / agent / local-lib / CLI）に限る。core 内部は引き続き `per` を使う。

#### 適用先ファイル・行番号
- `crates/takusu-core/src/lib.rs:68-103` (`Point`)、`:125-131` (`NormalDist`) — newtype と定数の追加先
- `crates/takusu-habit/src/time.rs:6-10` — `SLOT_MINUTES` を core からの re-export に置き換え
- `crates/takusu-agent/src/tools/rrule.rs:179` — ローカル定数 `SLOT_MINUTES` の重複を削除
- `crates/takusu-local-lib/src/app.rs:353, 385, 527-529, 577, 727-734` (分→スロット変換)
- `crates/takusu-util/src/lib.rs:261-324` (`parse_duration`)
- `crates/takusu-cli/src/main.rs:392-398, 436-438, 475-481, 592-598, 669-675` (CLI での変換)

#### 修正の重み
- **中** (newtype と定数の追加、各所の変換ロジック置き換え)


### 8.5 Audio provider 抽象化

#### 問題の要約
- 既存項目: 1.1 TTS/STT backend / provider / model / voice / language が `String`
- `takusu-agent/src/audio_config.rs` で `backend` / `provider` / `model` が文字列で、`audio.rs` で文字列マッチで分岐している。
- `takusu-audio` には既に `TtsBackend` enum があるが、STT 側は未定義。

#### 提案する trait / struct

```rust
/// STT backend identifier（TtsBackend に合わせて追加）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SttBackend {
    Sherpa,
}

impl std::fmt::Display for SttBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Sherpa => write!(f, "sherpa"),
        }
    }
}

impl std::str::FromStr for SttBackend {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "sherpa" => Ok(Self::Sherpa),
            _ => Err(format!("unsupported STT backend: {s}")),
        }
    }
}

/// 実行プロバイダー（CPU/CUDA/CoreML 等）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ExecutionProvider {
    Cpu,
    Cuda,
    CoreMl,
}

impl std::fmt::Display for ExecutionProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Cpu => write!(f, "cpu"),
            Self::Cuda => write!(f, "cuda"),
            Self::CoreMl => write!(f, "coreml"),
        }
    }
}

impl std::str::FromStr for ExecutionProvider {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "cpu" => Ok(Self::Cpu),
            "cuda" => Ok(Self::Cuda),
            "coreml" => Ok(Self::CoreMl),
            _ => Err(format!("unsupported execution provider: {s}")),
        }
    }
}

/// SttConfig の型安全化
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(default)]
pub struct SttConfig {
    #[serde(default = "default_stt_backend")]
    pub backend: SttBackend,
    #[serde(default = "default_stt_language")]
    pub language: String,
    #[serde(default)]
    pub model_dir: String,
    // SherpaOnnxModel は takusu-audio/src/sherpa.rs:17-23 に既存の enum。新規定義は不要。
    #[serde(default = "default_stt_model")]
    pub model: SherpaOnnxModel,
    #[serde(default = "default_stt_use_itn")]
    pub use_itn: bool,
    #[serde(default = "default_stt_num_threads")]
    pub num_threads: i32,
    #[serde(default = "default_stt_provider")]
    pub provider: ExecutionProvider,
    #[serde(default = "default_stt_sample_rate")]
    pub sample_rate: i32,
}

/// TtsConfig の型安全化
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(default)]
pub struct TtsConfig {
    #[serde(default = "default_tts_backend")]
    pub backend: TtsBackend,
    #[serde(default = "default_tts_api_key_env")]
    pub api_key_env: String,
    #[serde(default)]
    pub api_key: String,
    #[serde(default = "default_tts_voice_id")]
    pub voice_id: VoiceId, // newtype 化
    #[serde(default = "default_tts_language")]
    pub language: LanguageCode, // newtype 化
    #[serde(default = "default_tts_sample_rate")]
    pub sample_rate: u32,
    pub speed: Option<f32>,
    #[serde(default)]
    pub mute: bool,
}
```

**`SpeechBackend` / `SpeechModel` trait は作らない（監査結果）**

当初案では `backend_name()` / `model_name()` を返すだけの trait を提案していたが、
これは文字列を返すだけで型安全性が上がらず、既存の文字列マッチを置き換える効果もない。
**enum 化だけで目的は達成される**ので trait は不要。

抽象化が要るのは backend 名ではなく **生成された実体**の側であり、そこは既存の
`build_stt` / `build_tts` が返す `Arc<dyn SpeechToText>` / `Arc<dyn TextToSpeech>` で足りている。
enum 化の効果は `build_stt` / `build_tts` の `match` が網羅的になり、
backend を追加したときにコンパイルエラーで検知できるようになる点にある。

#### 適用先ファイル・行番号
- `crates/takusu-audio/src/tts.rs:16-21` (既存 `TtsBackend`)
- `crates/takusu-audio/src/sherpa.rs:17-23` (既存 `SherpaOnnxModel`)
- `crates/takusu-audio/src/stt.rs` (`SttBackend` を新規追加)
- `crates/takusu-agent/src/audio_config.rs:12-29` (SttConfig)
- `crates/takusu-agent/src/audio_config.rs:70-86` (TtsConfig)
- `crates/takusu-agent/src/audio.rs:226-268` (build_stt 文字列マッチ)
- `crates/takusu-agent/src/audio.rs:272-297` (build_tts 文字列マッチ)

#### 注意
- `audio_config` は設定ファイル（TOML）から読み込まれる。enum 化すると **未知の値でパースが失敗する**。
  現在の「未知なら既定値にフォールバック」挙動を保つなら、`#[serde(deserialize_with = ...)]` で
  未知値を既定値に落とすか、`Option<SttBackend>` にして呼び出し側で既定値を入れること。

#### 修正の重み
- **小〜中** (enum 定義追加、config struct のフィールド型変更、文字列マッチの置き換え)


### 8.6 Agent change target 抽象化

#### 問題の要約
- 既存項目: 3.7 `operation` / `target_type`（Agent ツール変更レシート）
- `ProposedChange` / `ChangeReceipt` で `operation` / `target_type` / `target_label` が文字列になっており、`execute_proposed_change` で `target_label.starts_with("task")` 等の prefix マッチで分岐している。
- 操作種別と対象種別が型で表現されていないため、コンパイル時の網羅性チェックができない。

#### 提案する trait / struct

> **制約（監査で判明）**
>
> - 実際のフィールド構成は次のとおりで、当初案の前提とずれている。
>   - `ProposedChange`（`tool.rs:122-135`）: `operation: String`, **`target_label: String`**（`target_type` は無い）
>   - `ChangeReceipt`（`tool.rs:165-178`）: `operation: String`, **`target_type: String`**, **`target_id: String`**
>   - `ChangeReceipt` は `#[derive(Default)]` 済みなので、enum 化するなら
>     `ChangeOperation` / `TargetKind` にも `Default` が要る。
> - `target_type` は `lib.rs` で `target_label` を空白分割して生成している。enum 化すればこの導出は不要になる。
> - `ProposedChange` は **LLM に JSON として提示され、承認 UI にも渡る**。フィールド名や表現を変えると
>   クライアント（CLI / Android / web）と保存済みセッションの互換性が壊れる。
>   まずは **JSON 表現を変えずに Rust 側の型だけ変える**こと（`operation` は文字列のまま出す、
>   `target_label` も文字列のまま出す）を優先する。

```rust
// takusu-util の enum_label! を使えば Display / FromStr / Default / TryFrom は自動生成できる。
// ここでは生成される形を明示する。

/// 操作種別
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ChangeOperation {
    #[default]
    Create,
    Update,
    Delete,
    Generate,
    Reschedule,
}

/// 対象種別
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TargetKind {
    #[default]
    Task,
    Habit,
    Skill,
    Memory,
    Schedule,
}
// Display / FromStr は enum_label! が "create" / "task" などの既存文字列で生成する。

/// 型安全なターゲット指定。
/// JSON 上は従来どおり `"task T-123"` の 1 本の文字列として出す（後方互換）。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Target {
    pub kind: TargetKind,
    pub display_id: String,
}

impl Target {
    pub fn new(kind: TargetKind, display_id: impl Into<String>) -> Self {
        Self {
            kind,
            display_id: display_id.into(),
        }
    }
}

impl std::fmt::Display for Target {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} {}", self.kind, self.display_id)
    }
}

impl std::str::FromStr for Target {
    type Err = UnknownLabel;
    fn from_str(label: &str) -> Result<Self, Self::Err> {
        // display_id 側に空白が入りうるので splitn を使う。
        let (kind, rest) = label
            .split_once(char::is_whitespace)
            .ok_or_else(|| UnknownLabel::new("Target", label))?;
        Ok(Self::new(kind.parse()?, rest.trim().to_string()))
    }
}

// Display / FromStr があるので、serde は文字列として往復させる。
// これにより JSON 表現は現行の `target_label` と完全に同じままになる。

/// ProposedChange の型安全化。JSON のキー名と値は現行のまま。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProposedChange {
    #[serde(with = "takusu_util::enum_serde")]
    pub operation: ChangeOperation,
    /// JSON 上のキー名は `target_label` のまま維持する。
    #[serde(rename = "target_label", with = "display_fromstr")]
    pub target: Target,
    pub description: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub before: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub after: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub arguments: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub observed_updated_at: Option<String>,
}

/// ChangeReceipt の型安全化。
/// 現行は `target_type` / `target_id` の 2 フィールドなので、`Target` を flatten して同じ形で出す。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ChangeReceipt {
    #[serde(with = "takusu_util::enum_serde")]
    pub operation: ChangeOperation,
    /// `target_type` / `target_id` の 2 キーに展開される。
    #[serde(flatten)]
    pub target: ReceiptTarget,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub before: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub after: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_revision: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub inferred_fields: Option<Value>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ReceiptTarget {
    #[serde(with = "takusu_util::enum_serde")]
    pub target_type: TargetKind,
    pub target_id: String,
}

/// execute_proposed_change の型安全な分岐
impl AgentSession {
    async fn execute_proposed_change(
        &self,
        change: &ProposedChange,
        args: Value,
        operation_id: Option<&str>,
    ) -> Result<ChangeReceipt, AgentError> {
        let (target_id, current_updated_at, existing_habit) = match change.target.kind {
            TargetKind::Schedule => (String::new(), None, None),
            TargetKind::Task => {
                let task = self.client.get_task(&change.target.display_id).await
                    .map_err(|e| AgentError::Tool(ToolError::Other(Box::new(e))))?;
                (task.id, Some(task.updated_at), None)
            }
            TargetKind::Habit => {
                let habit = self.client.get_habit(&change.target.display_id).await
                    .map_err(|e| AgentError::Tool(ToolError::Other(Box::new(e))))?;
                (habit.habit.id.clone(), Some(habit.habit.updated_at.clone()), Some(habit))
            }
            TargetKind::Skill => {
                let skill = self.client.get_skill(&change.target.display_id).await
                    .map_err(|e| AgentError::Tool(ToolError::Other(Box::new(e))))?;
                (skill.slug, Some(skill.updated_at), None)
            }
            TargetKind::Memory => {
                let memory = self.client.get_memory(&change.target.display_id).await
                    .map_err(|e| AgentError::Tool(client_error(e)))?;
                (memory.id, Some(memory.updated_at), None)
            }
        };

        // 既存の文字列 prefix マッチを enum マッチに置き換え
        match (change.target.kind, change.operation) {
            (TargetKind::Task, ChangeOperation::Create) => {
                // ...
            }
            (TargetKind::Task, ChangeOperation::Update) => {
                // ...
            }
            // ... 他の組み合わせ
            _ => Err(AgentError::Tool(ToolError::InvalidArgs(
                InvalidArgsError::no_field(format!(
                    "unsupported operation {:?} on target {:?}",
                    change.operation, change.target.kind
                ))
            ))),
        }
    }
}
```

#### 適用先ファイル・行番号
- `crates/takusu-agent/src/tool.rs:122-178` (`ProposedChange` / `ChangeReceipt`)
- `crates/takusu-agent/src/lib.rs:1917-1975` (`execute_proposed_change` 内の target_label prefix 判定)
- `crates/takusu-agent/src/tools/takusu.rs:1641-2173` (`operation` / `target_label` 生成)
- `crates/takusu-agent/src/tools/progress.rs:198` (`operation` / `target_label`)

#### 修正の重み
- **中** (enum 定義、struct のフィールド型変更、分岐ロジックの置き換え)


---

## 9. 監査結果

章 1〜8 の記載を実コードと照合した結果をまとめる。

### 9.1 章 1〜7 の正確性

章 1〜7 の該当箇所（ファイル・行番号・型名）は **全項目が実コードと一致していた**。
存在しない型名・フィールド名の記載（ハルシネーション）や、行番号の重大なズレはない。

ただし次の 3 点は前提の補正が要る。

| 項目 | 補正内容 |
|------|----------|
| 3.3 `solver` | `takusu_core::Solver` は存在するが、**`takusu-core` は serde に依存していない**（serde は dev-dependencies のみ）。そのまま Row / Request 型には使えない。 |
| 3.4 `recurrence` | `takusu_habit::RecurrenceRule` は serde derive 済みだが、`takusu-contracts` / `takusu-client` / `takusu-worker` はいずれも `takusu-habit` に依存していない。依存を足すと `takusu-core` まで引き込む。 |
| 4.4 分↔スロット | `takusu-core` は 5 をハードコードしていない（`Point::from_timestamp(ts, per)` / `Planner.per` で引数化済み）。重複しているのは core の **外側**。`takusu-agent/src/tools/rrule.rs:179` にローカル定数の重複がある。 |

### 9.2 依存グラフ（型の置き場所の判断材料）

```
takusu-search ── takusu-util ─┬─ takusu-contracts ─┐
                              ├─ takusu-client ──┤
                              ├─ takusu-worker   │  (wasm / Cloudflare Workers, sqlx なし)
                              └─ google-cal      │
                                                 ├─ takusu-local-lib ─┬─ takusu-local
takusu-core ── takusu-habit ─────────────────────┘                    ├─ takusu-cli
                                                                      ├─ takusu-web
takusu-audio ── takusu-android                                        └─ takusu-tui
takusu-client ── takusu-agent
```

判断の指針。

- **共有 enum / newtype の置き場所は `takusu-util` 一択**。`takusu-contracts` / `takusu-client` /
  `takusu-worker` が共通して依存する唯一の crate で、wasm ビルドにも対応している。
- **`takusu-core` に serde を持ち込まない**。core は純粋なプランナとして依存を絞っている。
  `Solver` を共有したい場合は `takusu-util` に別定義を置き、`takusu-local-lib` の境界で変換する。
- **`takusu-worker` は Cloudflare Workers 向け（`crate-type = ["cdylib"]`、D1）** で `sqlx` を持たない。
  storage 側の型変更が worker に波及する設計にしないこと。

### 9.3 章 8 の設計上の問題（修正済み）

初版の擬似コードには次の致命的な誤りがあった。本ドキュメントでは修正済み。

| 項目 | 問題 | 対応 |
|------|------|------|
| 8.1 | `Tool` に associated type を足すと `Box<dyn Tool>`（`tool.rs:250`）が object safety 違反でコンパイル不能 | object-safe な `Tool` を残し、`TypedTool` を上に重ねる二層構造に変更 |
| 8.1 | `ToolOutput` を trait として定義していたが、**同名の struct が既に存在**（`tool.rs:180-197`） | 既存 struct をそのまま使う設計に変更 |
| 8.1 | `schemars` がワークスペース依存に無い | 前提作業として明記 |
| 8.2 | `#[sqlx(try_from = ..., with = ...)]` という併用構文は存在しない | `TryFrom<String>` + `#[sqlx(try_from = "String")]` に変更 |
| 8.2 | `HasValueRef` ベースの汎用 sqlx アダプタは sqlx 0.9 の API と不一致。そもそも `takusu-contracts` はドライバ feature を有効にしておらず `Encode`/`Decode` を書けない | 汎用 sqlx アダプタを廃止 |
| 8.2 | `EnumLabel::default()` が `Default::default()` と曖昧 | `enum_default()` に改名。`Sized` bound も追加 |
| 8.4 | `impl AsMinutes for Point` は orphan rule 違反（`Point` は `takusu-core`） | 定義を `takusu-core` に移し、trait をやめて inherent method に変更 |
| 8.4 | `impl AsSlots for i64` は primitive への外部 impl で不可、かつ単位の取り違えを助長 | 削除 |
| 8.5 | `SherpaOnnxModel` を「enum 化」と書いていたが **既に enum** | 既存利用と明記 |
| 8.5 | `SpeechBackend` / `SpeechModel` trait は文字列を返すだけで価値がない | 削除 |
| 8.6 | `ProposedChange` に `target_type` は無い（実際は `target_label` のみ）。`ChangeReceipt` は `target_type` + `target_id` | 実フィールドに合わせて設計を修正 |
| 8.6 | `ChangeReceipt` の `#[derive(Default)]` を保つには enum 側に `Default` が要る | `#[default]` を追加 |

### 9.4 残る主要リスク

1. **`ProposedChange` の JSON 表現は変えられない**。LLM への提示、承認 UI、保存済みセッションが依存している。
   Rust 側の型だけ変え、シリアライズ結果は現行と一致させること。回帰テストで JSON を固定する。
2. **`schemars` によるスキーマ生成は LLM の挙動を変えうる**。手書きスキーマの `description` や
   `additionalProperties: false` が失われると、ツール呼び出しの精度が落ちる可能性がある。
3. **`audio_config` の enum 化は設定パースを厳格化する**。現在は未知の文字列でも既定値にフォールバックしている
   可能性があり、そのまま enum にすると起動失敗になる。
4. **章 6（`takusu-core` のタプル → struct）はホットパスに触れる**。`takusu-core` には
   `benches/plan.rs` と `benches/realworld.rs` があるので、変更前後でベンチを取ること。

---

## 10. 修正計画

方針は次の 3 つ。

- **共有 enum / newtype は `takusu-util` に置く**。crate 依存グラフを変えない。
- **外部に見える JSON / DB 表現は変えない**。型だけ変える。表現を変える提案は本計画から除外する。
- **1 フェーズ = 1 PR** とし、各フェーズ単独でビルドとテストが通る状態を保つ。

### フェーズ 0: 基盤の追加（前提）

`takusu-util` に共通の型基盤を置く。この時点では既存コードは変更しない。

- `takusu-util` に `EnumLabel` trait、`enum_serde` モジュール、`enum_label!` マクロ、`UnknownLabel` エラー型を追加する。
- 単体テストで `Display` / `FromStr` / serde 往復 / 未知値のエラーを確認する。

**成果物**: `crates/takusu-util/src/enum_label.rs`（新規）
**検証**: `cargo nextest run -p takusu-util`
**リスク**: 低（純粋な追加）

### フェーズ 1: enum 相当の `String` を enum 化（3.1 / 3.2 / 3.5 / 3.6 / 8.2）

最も費用対効果が高い。`status` / `window_mode` / `scope` / `kind` / `subject_type` を対象にする。

1. `takusu-util` に `TaskStatus` / `WindowMode` / `TokenScope` / `MemoryKind` / `SubjectType` を `enum_label!` で定義する。
2. `takusu-contracts/src/model.rs` の該当フィールドを差し替え、`#[sqlx(try_from = "String")]` を付ける。
3. `takusu-client/src/lib.rs`、`takusu-worker/src/models.rs` の対応フィールドを差し替える。
4. `takusu-local-lib/src/app.rs` の `validate_window_mode` など、enum 化で不要になった検証を削除する。
5. `takusu-agent/src/tools/takusu.rs:455-464` の `normalize_status` を `TaskStatus::from_str` に置き換える。
6. `takusu-cli` の該当箇所を `clap::ValueEnum` に寄せる。

**除外**: 3.3 `solver` と 3.4 `recurrence` は依存グラフの制約があるためフェーズ 5 に回す。
**検証**: `cargo check --workspace`、`cargo nextest run`、`cargo clippy`。DB に既存の未知値が無いか確認する。
**リスク**: 中（DB の既存値が enum に無い文字列だとデコードで失敗する。マイグレーション前に値の分布を確認すること）

### フェーズ 2: 分↔スロットの定数統一（4.4 / 8.4）

1. `takusu-core` に `Minutes` / `Slots` newtype と `SLOT_MINUTES` 定数、`Point::minutes_since_epoch`、`NormalDist::from_minutes` を追加する。
2. `takusu-habit/src/time.rs:6` の `SLOT_MINUTES` を `takusu-core` からの re-export に変える。
3. `takusu-agent/src/tools/rrule.rs:179` のローカル定数を削除して `takusu_habit`（または core）を参照する。
4. `takusu-local-lib` / `takusu-cli` の `/ 5`、`* 5` を newtype 経由に置き換える。

**注意**: `takusu-core` 内部は `Planner.per` を使い続ける。定数で置き換えないこと。
**検証**: `cargo nextest run -p takusu-core -p takusu-habit`、`cargo bench -p takusu-core`（回帰が無いこと）
**リスク**: 低〜中（切り捨て / 切り上げの挙動を変えないよう、置き換え前の式と一致するか確認する）

### フェーズ 3: 値域を持つプリミティブの newtype 化（4.1 / 4.2 / 4.6）

1. `takusu-util` に `Abandonability`（`[0,1]` に clamp）、`Quantity`（非負）を追加する。
2. `takusu-contracts` / `takusu-client` / `takusu-worker` / `takusu-core` / `takusu-habit` の該当フィールドを差し替える。
3. `takusu-agent/src/llm.rs:57` の `request_timeout_seconds: u64` を `Duration` に変える。

**検証**: `cargo nextest run`。境界値（0、1、負値、NaN）のテストを追加する。
**リスク**: 低

### フェーズ 4: agent の変更レシートを enum 化（3.7 / 8.6）

1. `takusu-agent` に `ChangeOperation` / `TargetKind` / `Target` / `ReceiptTarget` を追加する。
2. `ProposedChange` / `ChangeReceipt` のフィールド型を差し替える。**JSON 表現は現行と同一に保つ**。
3. `execute_proposed_change`（`lib.rs:1917-1975`）の prefix マッチを `match (kind, operation)` に置き換える。
4. `tools/takusu.rs` / `tools/progress.rs` の生成側を型経由にする。

**前提**: フェーズ 0 の `enum_label!` が入っていること。
**検証**: 既存 JSON を固定する回帰テストを **先に追加**してから型を変える。
**リスク**: 中（承認フローの後方互換。JSON が 1 バイトでも変わると client に影響する）

### フェーズ 5: 依存グラフの調整を伴うもの（3.3 / 3.4）

1. `takusu-util` に `Solver` を `enum_label!` で定義し、`takusu-local-lib` の境界で `takusu_core::Solver` へ変換する。`parse_solver` を削除する。
2. `recurrence` は `takusu-contracts` / `takusu-client` / `takusu-worker` では `String` のまま維持し、`takusu-local-lib` の境界でのみ `RecurrenceRule` に変換する。境界を 1 箇所に集約する。

**検証**: `cargo check --workspace --target wasm32-unknown-unknown`（worker が壊れていないこと）
**リスク**: 中

### フェーズ 6: 日付・時刻の型化（5.1〜5.4 / 5.6）

範囲が最も広いので、対象を絞って段階実施する。

1. まず `sleep_start` / `sleep_end` / Habit の `start_time` / `end_time` を `TimeOfDay` にする。
   `takusu-habit::TimeOfDay` は現在 `Debug, Clone, Copy` しか derive していないので、
   `PartialEq` / `Eq` / `Serialize` / `Deserialize` を追加するか、`takusu-util` に同等の型を置く。
2. 次に `start_date` / `end_date` を `jiff::civil::Date` にする。
3. 最後に `start_at` / `end_at` / `created_at` などのタイムスタンプを `jiff::Timestamp` にする。

**注意**: `jiff` に `sqlx` feature は有効化されていない。sqlx との統合には feature 追加か、
`#[sqlx(try_from = "String")]` 経由の変換が要る。DB のカラムは TEXT のままにしておくこと。
**検証**: `cargo nextest run`、タイムゾーン跨ぎのテストを追加する。
**リスク**: 中〜大

### フェーズ 7: `takusu-agent` の `serde_json::Value` 削減（2.1〜2.5 / 8.1）

最もコストが高い。ここまでのフェーズが安定してから着手する。

1. `schemars` をワークスペース依存に追加する。
2. `TypedTool` trait とラッパを `tool.rs` に追加する。既存 `Tool` は残す。
3. **1 ツールずつ** `TypedTool` に移行する。移行のたびに、生成される JSON Schema が
   手書きスキーマと一致するかをテストで確認する。
4. `llm.rs` の `Message` / `ChatCompletionRequest` / `ToolCall` を struct 化する。
5. `execute_proposed_change` / `parse_habit_step` の `Map<String, Value>` 手動分解を専用 struct に置き換える。

**検証**: ツールごとにスキーマのスナップショットテストを置く。
**リスク**: 大（LLM の挙動が変わりうる。スキーマ差分は必ずレビューする）

### フェーズ 8: `takusu-core` のタプル解消（6.1 / 6.2）

1. `benches/plan.rs` / `benches/realworld.rs` でベースラインを取る。
2. `Placement` を `struct TaskPlacement { start, end, task_id }` に変える。
3. `previous_schedule` の `Option<(Point, Point)>` を `Option<TimeWindow>` に変える。
4. ベンチを再取得し、回帰が無いことを確認する。

**注意**: `anneal.rs` / `decoder.rs` / `evaluate.rs` はホットパス。`#[repr(C)]` やフィールド順で
レイアウトが変わりうるので、**ベンチの前後比較を必須**とする。回帰したら中止してよい。
**リスク**: 中（正しさよりも性能が問題になる領域）

### 実施順序と依存関係

```
フェーズ0（基盤）
  ├→ フェーズ1（enum 化）──→ フェーズ4（agent レシート）
  │                       └→ フェーズ5（solver / recurrence）
  ├→ フェーズ3（newtype）
  └→ フェーズ6（日付・時刻）

フェーズ2（slot 定数）      … 独立
フェーズ7（Value 削減）     … フェーズ0/1 の後
フェーズ8（core タプル）    … 独立、性能次第で中止可
```

**まず着手すべきはフェーズ 0 → 1 → 2**。ここまでで「文字列 typo によるバグ」の大半が消え、
影響範囲も限定的で、後方互換の問題も起きにくい。

フェーズ 7 と 8 は投資対効果を見てから判断する。とくにフェーズ 8 は性能回帰したら
実施しない判断があってよい。
