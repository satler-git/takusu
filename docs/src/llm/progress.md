# 作業セッションと進捗

`task_start`、`task_pause`、`task_progress`、`task_complete`、`task_split` は Deferred ツールである。
使う前に `tool_search` で発見する。

タスクの作業は、開始、一時停止、進捗記録、完了、分割の一連で進む。

## 作業を開始する

`task_start` はタスクを `in_progress` にする提案を生成する。
`task_ref` を省略すると、`scheduled` または `pending` のタスクから選択を促す focused clarification が返る。

## 作業を一時停止する

`task_pause` は作業セッションを閉じ、実働時間を記録する。
`task_ref` を省略すると、`in_progress` のタスクから選択を促す。

## 進捗を記録する

`task_progress` は `quantity_done` を累積値で更新する。
`quantity_done` を減らすと訂正扱いになり、新しい速度観測にはならない。

`quantity_done` が `quantity_total` 以上になると、完了を示唆する警告が出る。
そのときは `task_complete` を呼んで、完了と実績時間を記録する。

作業時間から所要時間の推定値をプレビューすることがある。
そのプレビューは `after` の `avg_minutes` や `sigma_minutes` に反映される。

## 作業を完了する

`task_complete` はタスクを `completed` にし、実績時間を記録する。
`quantity_total` がある場合、`quantity_done` も `quantity_total` に合わせる。

## タスクを分割する

`task_split` はタスクを「元のタスクに残す量」と「新しい残りタスク」に分ける。
`retained_quantity` は 0 より大きく、`quantity_total` より小さく、`quantity_done` 以上である必要がある。

`set_dependency` はデフォルトで true となり、残りタスクが元のタスクに依存する。
`title` を省略すると、元のタスクのタイトルが使われる。
