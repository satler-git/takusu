+++
name = "settlement"
description = "Record off-plan time and replan the rest of the day."
+++

予定外に使った時間を記録し、その後のスケジュールを再計画するスキル。

## いつ使うか

- 「9 時から 12 時までゲームしてた」
- 「予定がずれた、お昼寝してた」
- その他、予定通りに過ごせなかった時間があったとき

## 手順

1. ユーザーから時間帯と用途を確認する。
2. `list_unsettled_intervals` を呼び、既存の未精算 interval があれば `interval_id` を取得する。
3. `propose_settlement` を呼ぶ。
   - `start_at` / `end_at`: 未精算の時間帯
   - `classification`: 用途（`game`, `rest`, `chore`, `unclassified` など）
   - `interval_id`: 既存 interval があれば指定
4. `mode`, `from`, `until` は省略可。省略時は `mode=range`, `from=end_at`, `until=今日の終わり`。
5. 承認後、影響を受けたタスクがあれば `add_comment` でコンテキストを記録する。

## 例

- ユーザー：「10:00〜12:00 ゲームしてた」
- 行動：
  - `list_unsettled_intervals` を確認
  - `propose_settlement`（`start_at=2026-08-25T10:00`, `end_at=2026-08-25T12:00`, `classification=game`）
