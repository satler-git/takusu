# スケジュールの生成と調整

スケジュールは、タスクと習慣の発生を時間軸に配置した結果である。
`get_schedule` と `preview_schedule` は Direct である。
`generate_schedule`、`reschedule`、`move_task` は Deferred である。
設定を確認するときは Deferred の `get_settings` を使う。

## 現在のスケジュールを取得

`get_schedule` はアクティブなスケジュールを返す。
`from` と `to` で日付範囲を絞れる。
`no_overdue` を true にすると、期限切れセクションを省略できる。

返り値には `id`、`created_at`、`updated_at`、`entries` と、`no_overdue` が false の場合は `overdue` がある。
`entries` は各予定の `reference`、`title`、`start_at`、`end_at` を含む。
`overdue` は期限切れタスクの `reference`、`title`、`end_at` を含む。

## プレビューと生成

`preview_schedule` はスケジュールを生成するが、`active` スケジュールは置き換えない。
`generate_schedule` は承認後に `active` スケジュールを置き換える。

`preview_schedule` の返り値には `entries` のほか、`unscheduled_task_ids` と `displaced_task_ids` が含まれる。
前者はスケジュールに入らなかったタスク、後者は既存スケジュールから移動されたタスクを示す。

## 再スケジュールの mode

`preview_schedule` と `reschedule` の `mode` は `full`、`range`、`tasks` のいずれかである。
`preview_schedule` のデフォルトは `full` である。

- `full`：すべてのタスクと習慣から再生成する
- `range`：`from` と `until` で指定した範囲のみ再生成する
- `tasks`：`task_ids` で指定したタスクのみ再生成する

`pinned` に指定したタスク参照は、再スケジュール時に現在の開始時刻を維持する。

## sleep

`sleep` は睡眠時間を文字列で指定する。
`recommended`、`disabled`、`HH:MM-HH:MM` のいずれかを使う。
`recommended` はサーバー設定の睡眠時間を使う。
`disabled` は睡眠時間を無視する。

## 部分再スケジュール

`reschedule` は `mode` に応じた範囲またはタスクだけを再調整する。
承認後、該当部分が `active` スケジュールに反映される。
