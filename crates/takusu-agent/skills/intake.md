+++
name = "intake"
description = "Initial interview to collect deadlines, recurring commitments, and coverage."
+++

初回の intake インタビューを行うスキル。

## いつ使うか

- `intake_state` が `not_started` のとき
- `coverage` が `bootstrap` のとき

## ゴール

ユーザーの締め切りと定期予定を 1 つの Proposal としてまとめ、承認を得る。

## 手順

1. 「締め切りが決まっているものを、思いつく順で話してください」と尋ねる。
2. 「毎週や定期であるものは？ 授業、バイト、習慣など」と尋ねる。
3. ユーザーの話を聞きながら、各項目を `create_task` または `create_habit` として提案する。
   - 数量が含まれる場合は `tool_search` で `similar_tasks` を見つけて呼び、`avg_minutes` / `sigma_minutes` / `quantity_total` / `quantity_unit` を推定する。
   - 推定理由は `create_task` / `create_habit` の `inferred_fields` に記載する。
4. すべての `create_task` / `create_habit` は同じ `proposal_id` を使う。
5. 各ステージの開始時に `set_intake_state` を呼び、以下を記録する。
   - `stage`: `deadlines` → `recurring` → `complete`
   - `proposal_id`: このバッチで使う値
   - `collected_ids`: これまで提案したタスク・習慣の id
6. ユーザーが「今日はここまで」と言ったら、まとめて承認を求める。OK なら `set_intake_state` を `coverage_pending: true` で呼び、同じ `proposal_id` で `coverage_confirm`（`start_at` / `end_at` / `timezone` を本日の範囲で補完して、`source: "intake_complete"`）を呼ぶ。
7. 補足が必要な場合は、一度に 1 つの焦点を絞った質問だけをする。複数の質問を連ねない。

## 例

- ユーザー：「来週月曜にレポート締め切り。毎週水曜はゼミ。」
- 行動：
  - レポート → `create_task`（`end_at` は来週月曜 23:59、数量があれば `similar_tasks` で推定）
  - ゼミ → `create_habit`（`rrule` に `BYDAY=WE`、習慣名は「ゼミ」）
  - 両方に同じ `proposal_id` を指定
