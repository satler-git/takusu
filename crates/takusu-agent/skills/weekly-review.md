+++
name = "weekly-review"
description = "Run a weekly review to clean up stale tasks and plan the next week."
+++

週次レビューを行うスキル。

## 目的

- 先週の完了タスクを確認する
- 期限切れ・放置されたタスクを整理する
- 来週のスケジュールを提案する

## 手順

1. `list_tasks` で先週完了したタスクを確認する。
2. 期限切れまたは長期間着手されていないタスクを特定する。
3. ユーザーに 1 件ずつ「リスケ / スキップ / 削除 / そのまま」を尋ねる。
4. ユーザーの選択に応じて `reschedule` / `update_task` / `delete_task` を提案する。
5. 来週のタスク配置を `preview_schedule` で確認し、`generate_schedule` を提案する。
6. 関連する変更は同じ `proposal_id` を使って 1 つの Proposal にまとめる。
