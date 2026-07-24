# takusu-agent の動作と承認

takusu-agent は takusu-local または takusu-worker に接続するエージェント用ハーネスである。
LLM はこのハーネスを通じてタスク、習慣、スケジュールを操作する。
ツール定義には名前、説明、引数の JSON Schema が含まれる。
個別の引数はツール定義を確認すればわかるため、ここでは呼び出しの前後の振る舞いを中心に説明する。

## ツールの可視性

takusu-agent のツールは、Direct と Deferred の 2 種類に分かれる。
Direct は毎回のツール一覧に含まれる。
Deferred はデフォルトで含まれず、必要なときに `tool_search` で検索して発見する。

Direct に含まれるのは以下のようなツールである。

- 一覧・取得：`list_tasks`、`get_task`、`list_habits`、`get_habit`、`get_schedule`
- プレビュー・変更提案：`preview_schedule`、`create_task`、`update_task`、`delete_task`、`create_habit`、`update_habit`、`delete_habit`
- ユーティリティ：`day_details`、`correct_asr`、`tool_search`

Deferred になるのは以下のようなツールである。

- 進捗：`task_start`、`task_pause`、`task_progress`、`task_complete`、`task_split`
- 記憶：`memory_search`、`memory_save`、`memory_update`、`memory_delete`、`similar_tasks`
- スキル：`skills_list`、`skills_read`、`skills_propose_add`、`skills_propose_edit`
- スケジュール生成：`generate_schedule`、`reschedule`、`move_task`
- 習慣 scheduled span：`habit_scheduled_spans`
- 設定：`get_settings`
- RRULE 展開：`expand_rrule`

`tool_search` は Direct である。
`query` にキーワードを渡し、`limit` で最大 20 件まで指定できる（デフォルト 5）。
Deferred ツールが必要なときは、まず `tool_search` を呼び、結果に含まれたツール名を使って呼び出す。

## 承認フロー

タスクの作成、更新、削除、習慣の作成、更新、削除、スケジュールの生成、再調整など、永続的な変更はすべてユーザー承認を経る。

ツールを呼び出すと、takusu-agent は `ToolOutput` として以下の情報を返す。

- `content`：LLM への応答文字列
- `why`：ユーザーに提示する変更理由
- `warnings`：注意喚起
- `proposed_changes`：承認 UI 用の変更提案リスト
- `inferred_fields`：曖昧な入力から推測したフィールド
- `discovered_tools`：`tool_search` で発見した Deferred ツール名
- `schedule_dirty`：スケジュールの再生成が必要になったことを示す

`proposed_changes` はアプリケーション UI に渡される。
承認されるまで実際の書き込みは行われない。

## タスク参照

タスクには `display_id` を使った参照がある。

- `#42`：display_id が 42 のタスク
- `h1#3`：display_id が 1 の習慣に属する display_id 3 のタスク
- `["#1", "#2", "h1#3"]`：複数参照を配列で指定できる

`task_ref` 引数に `#` を付けても付けなくても同じタスクを指す。

## 日時表現

時刻はサーバーで設定された timezone を基準に解釈される。
timezone や睡眠時間、workload 設定は `get_settings` で取得できる。
`get_settings` は Deferred ツールである。

- `YYYY-MM-DDTHH:MM:SS`：絶対時刻、オフセット省略時はサーバー TZ
- `YYYY-MM-DD`：日付、時刻は 0 時
- `today`、`tomorrow`、`yesterday`
- `7d`、`-3d`：相対日数
- `now`

## エラーと再試行

ツール呼び出しが失敗したとき、takusu-agent は `ToolError` を返す。

- `InvalidArgs`：引数が不正。フィールド名と理由が含まれる
- `NotFound`：参照先が存在しない
- `Conflict`：楽観的ロック競合。最新データを取り直して再試行する
- `Cancelled`：ユーザーがキャンセルした

`InvalidArgs` と `NotFound`、`Conflict`、`Cancelled` は再試行可能なエラーとして扱われる。
