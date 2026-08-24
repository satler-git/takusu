+++
name = "intake"
description = "Initial interview to collect deadlines, recurring commitments, and calendar coverage."
+++

Run the first-time intake interview:

1. Ask the user for deadlines first: "締め切りが決まっているものを、思いつく順で話してください。"
2. Then ask for recurring commitments: "毎週や定期であるものは？ 授業、バイト、習慣など。"
3. Finally confirm calendar import: "カレンダーに入っている予定は同期しておきますか？"
4. At the start of each stage, call `set_intake_state` with the current `stage`, the `proposal_id` you will use for this batch, and any `collected_ids` from previously accepted items. Advance `stage` through `deadlines`, `recurring`, `calendar_import`, and finally `complete`.
5. For each item the user mentions, use `create_task` or `create_habit` with estimates filled from `similar_tasks` and context. Record `inferred_fields`.
6. Group all related create/update calls under one `proposal_id` so they appear as a single approval set.
7. When the user wants to pause or has no more to say, ask "今日はここまでにしますか？" If yes, call `set_intake_state` with `coverage_pending: true` (and the current `proposal_id`), then call `coverage_confirm` with the *same* `proposal_id` and `source: "intake_complete"`. This makes the coverage confirmation part of the same batch proposal; it is written only when the whole batch is approved, so `today-covered` cannot advance before the batch commits. Do not record coverage if any part of the batch was not committed.
8. Keep responses short. Ask at most one focused clarification at a time. The interview is resumable: the next session continues from the same state if the client saved it.
