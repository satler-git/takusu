# スケジュールを扱う

## スケジュールを生成する

```sh
cargo run -p takusu-cli -- schedule generate
```

`schedule generate` は、`pending` と `scheduled` のタスク、および習慣から展開されたタスクを対象にします。
**Solver** は、これらのタスクの開始時刻を最適に配置するスケジューリングコンポーネントです。

## 現在のスケジュールを確認する

```sh
cargo run -p takusu-cli -- schedule get
```

`rich` モードでは、テーブル形式で表示されます。

## 部分再スケジュール

特定の時間範囲内のタスクだけを再計算します。
`--mode` は必須です。
`range` を使う場合、`--from` と `--until` も必要です。
`tasks` を使う場合、`--task-ids` で対象タスクを指定します。

```sh
cargo run -p takusu-cli -- schedule reschedule \
  --mode range \
  --from "2026-07-28T09:00:00" \
  --until "2026-07-28T18:00:00"
```

範囲外のタスクは位置が固定されます。

## タスクを手動で移動する

```sh
cargo run -p takusu-cli -- schedule move <task_id> \
  --start-at "2026-07-28T14:00:00"
```

移動後の終了時刻が締切を超える場合はエラーになります。
`--force` を付けると、締切違反も強制的に適用できます。

## スケジュールをクリアする

```sh
cargo run -p takusu-cli -- schedule clear
```

`schedule clear` はスケジュールを削除します。
タスクの `status` は `scheduled` のままです。
再度 `schedule generate` するまで `pending` に戻しません。
