+++
name = "capture"
description = "Register a one-off, recurring, free-time, or routine outcome from a single gap check-in answer."
+++

未分類 gap（予定に空白）への check-in 回答から、1 ターンでタスク・習慣・coverage を登録するスキル。

## いつ使うか

- ユーザーが「今なにしてる？」check-in に答えて活動を伝えたとき
- ユーザーが今やっていることを話したとき（例：「バイトの引き継ぎ資料つくってる」）
- 分類が明確でない場合（one-off / 毎週 / 自由時間 / ルーティン）

## 手順

1. `get_schedule` と `list_unsettled_intervals` を呼び、該当する未分類 gap を確認する。
2. 分類が明らかなら、直接対応するツールを呼ぶ。
   - 今回だけのタスク → `create_task`（`start_at` / `end_at` に gap の時刻を使う）
   - 毎週/定期的 → `create_habit`（`start_time` / `end_time` に gap の時刻の HH:MM を、`recurrence` は必要に応じて `write-rrule` を読んで作成）
   - 自由時間 → `coverage_confirm`（`source=target_period` としてその時間を自由時間にする）
   - ルーティン → `coverage_confirm`（`source=system`）または `create_habit`（明確に定期的な場合）
3. 分類が曖昧なら `gap_capture_check_in` を呼ぶ。`activity` にはユーザーの発話をそのまま入れる。
4. `gap_capture_check_in` の結果は「今回だけ / 毎週 / 自由時間 / ルーティン」の選択肢を含む CheckInCard になる。ユーザーが選んだら、次のターンで上記の対応ツールを 1 回だけ呼んで提案する。
5. 推定（`avg_minutes` / `sigma_minutes` / `quantity`）は `similar_tasks` や過去のコメントから補完し、`inferred_fields` に理由を残す。
6. 同じ batch の変更は同じ `proposal_id` でまとめ、`coverage_confirm` も含める。

## 例

- ユーザー：「バイトの引き継ぎ資料つくってる」
- 行動：
  - `gap_capture_check_in`（`activity=バイトの引き継ぎ資料つくってる`）
- ユーザー：「毎週」
- 行動：
  - `list_unsettled_intervals` で interval を確認
  - `similar_tasks` で見積もりを推定
  - `create_habit`（タイトル=バイトの引き継ぎ資料作成、recurrence=毎週、start_time/end_time に gap の時刻の HH:MM を使う）

## 制約

- 分類を決める追加の質問はしない。曖昧なら 4 つの選択肢を提示する。
- 自由時間・buffer・ルーティンとして既に説明できる時間は check-in しない。
