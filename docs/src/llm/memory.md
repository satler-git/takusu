# 記憶の検索と保存

`memory_search`、`similar_tasks`、`memory_save`、`memory_update`、`memory_delete` は Deferred ツールである。
使う前に `tool_search` で発見する。

takusu-agent はこれらを使って記憶を管理する。

## 記憶の種類

`kind` は以下のいずれかである。

- `proper_noun`：固有名詞
- `fact`：事実
- `task_note`：タスクに紐づくノート

`task_note` を使う場合は `subject_type` に `task`、`subject_id` にタスク ID を指定する必要がある。

## 検索

`memory_search` は `q` にキーワードを渡す。
複数キーワードは AND、`*` は任意の文字列にマッチする。
`kind` はカンマ区切りで OR になる。

`similar_tasks` は、完了済みタスクのタイトルから類似タスクを探す。
所要時間の推定に使う。

## 更新と削除

`memory_update` と `memory_delete` には `memory_ref` と `observed_revision` が必要である。
`observed_revision` は楽観的ロックに使われ、取得時の `revision` と一致していないと `Conflict` になる。
