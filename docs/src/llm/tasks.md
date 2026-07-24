# タスクの扱い

タスクは、締切と所要時間と依存関係を持つ作業単位である。
takusu-agent は `list_tasks`、`get_task`、`create_task`、`update_task`、`delete_task`、`move_task` を提供する。
`create_task`、`update_task`、`delete_task`、`list_tasks`、`get_task` は Direct である。
`move_task` は Deferred である。
ここでは、ツール定義に入りきらないタスク特有の概念を説明する。

## list_tasks の絞り込み

`list_tasks` は `status`、`from`、`until`、`no_overdue`、`habit_id`、`q`、`limit` で絞り込める。
`status` は正規化された値でフィルタリングされる。
`no_overdue` を true にすると `end_at` が過ぎたタスクを除外する。
`no_overdue` は `status=overdue` と同時に使わない。
詳しい `q` の構文は `search.md` を参照する。

## タスク参照の正規化

`task_ref` には `#` を付けても付けなくても同じ。
takusu-agent は内部で先頭の `#` を取り除く。
`h1#3` のように習慣スコープを持つ参照もそのまま使える。

## 状態

`status` には `pending`、`scheduled`、`in_progress`、`completed`、`skipped` がある。
`update_task` の `status` には同義語を渡すこともできる。

- `done`、`complete`、`completed` → `completed`
- `todo`、`to-do`、`to_do`、`pending` → `pending`
- `in-progress`、`in_progress`、`inprogress`、`doing`、`in progress` → `in_progress`
- `skip`、`skipped` → `skipped`
- `planned`、`scheduled` → `scheduled`

`completed` や `skipped` のタスクに対しては、作業開始、一時停止、進捗記録、完了、分割はできない。

## 依存、並列、固定

`depends` は他のタスク参照の配列である。
依存先が完了していないと、スケジュール上で先行する。

`parallelizable` が true のタスクは、依存さえ満たされていれば他のタスクと同時進行可能になる。
`allows_parallel` が true のタスクは、その時間帯に他のタスクを重ねてよいことを表す。

`fixed` が true のタスクは、開始時刻が固定され、スケジューラが動かさない。

## 数量

`quantity_total` と `quantity_done`、`quantity_unit` を使うと、タスクを数量的に追跡できる。
`quantity_done` は `task_progress` で累積値として更新する。
減らすと訂正扱いになり、新しい速度観測にはならない。

`quantity_done` が `quantity_total` 以上になると、`task_complete` を呼んで完了として記録すべきかの警告が出る。

## 所要時間の標準偏差

`sigma_minutes` は所要時間の標準偏差を分で表す。
省略すると 0 になる。
0 の場合、タスクの所要時間は確定的に扱われる。

## move_task

`move_task` はアクティブなスケジュールに含まれるタスクの開始時刻を移動する提案を生成する。
`fixed` はデフォルトで true となり、移動後も時刻が固定される。
`force` を true にすると、締切違反の警告を無視できる。

`move_task` は Deferred ツールである。
使う前に `tool_search` で `move_task` を発見する。
