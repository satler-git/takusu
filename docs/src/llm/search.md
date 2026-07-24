# 検索クエリの構文

`list_tasks` は Direct である。
`memory_search` は Deferred である。

`list_tasks` と `memory_search` はクエリ文字列で絞り込める。
複数キーワードは AND、`OR`、`-` または `NOT`、`()` によるグループ化が使える。

## タスク検索の修飾子

- `status:`：`pending`、`scheduled`、`in_progress`、`completed`、`skipped`、`overdue`
- `title:`：タイトルに含まれる
- `desc:`：説明に含まれる
- `start:`：開始時刻
- `end:`：締切
- `scheduled-start:`：スケジュール開始
- `scheduled-end:`：スケジュール終了
- `from:`：`end:>=` のエイリアス
- `until:`：`start:<=` のエイリアス
- `habit:`：習慣に紐づくタスク
- `depends:`：指定タスクに依存する
- `dependents:`：指定タスクを依存に持つ
- `deps_count:`：依存数
- `is:`：`fixed`、`parallelizable`、`allows_parallel`、`overdue`
- `has:`：`description`、`completed_at`、`schedule`、`depends`

## 日付表現

- `YYYY-MM-DD`
- `today`、`tomorrow`、`yesterday`
- `Nd` または `-Nd`
- 演算子：`>=2026-07-25`、`<2026-07-30`

## 記憶検索の構文

`memory_search` の `q` は、複数キーワードを AND する。
`*` は任意の文字列にマッチする。
`kind` はカンマ区切りで OR になる。例：`proper_noun,fact`
