# 習慣を管理する

## 習慣を作成する

`--recurrence` には **`RecurrenceRule`** の JSON を渡します。
**`RecurrenceRule`** は、習慣の繰り返し条件を表すデータです。
時刻は `--start-time` と `--end-time` で指定します。

```sh
cargo run -p takusu-cli -- habit create \
  --title "朝のランニング" \
  --recurrence '{"freq":"daily","interval":1,"by_day":[],"by_month":[],"by_month_day":[],"count":null,"exdates":[]}' \
  --start-time "06:00" \
  --end-time "06:30" \
  --avg-time 30m \
  --sigma-time 10m
```

## 習慣を編集、削除する

`habit edit` でエディタを開き、`habit update` で JSON を指定して更新できます。
`habit delete` で削除できます。

```sh
cargo run -p takusu-cli -- habit edit <id>
cargo run -p takusu-cli -- habit update <id> --recurrence '{"freq":"weekly","interval":1,"by_day":[{"weekday":"mon"},{"weekday":"wed"},{"weekday":"fri"}],"by_month":[],"by_month_day":[],"count":null,"exdates":[]}'
cargo run -p takusu-cli -- habit delete <id>
```

## その他の操作

- `habit show <id>`：習慣の詳細を表示します。
- `habit replace <id>`：習慣を完全に置き換えます。
- `habit scheduled-spans <subcommand>`：特定の期間のみ有効/無効にするスパンを管理します。
- `habit steps <subcommand>`：習慣に紐づく複数のステップを管理します。
- `habit steps-check <id>`：ステップ間の冗長な依存辺を検出します。

`habit create` / `habit update` には、固定予定化（`--fixed`）、並列実行（`--parallelizable` / `--allows-parallel`）、期間指定モード（`--window`）などのオプションがあります。

## 習慣からタスクへの展開

`schedule generate` を実行すると、「`RecurrenceRule`」に基づいてタスクが生成されます。
生成されたタスクは `habit_id` を持ち、元の習慣と紐づきます。

## RecurrenceRule JSON の例

| 目的 | JSON |
|------|------|
| 毎日 | `{"freq":"daily","interval":1,"by_day":[],"by_month":[],"by_month_day":[],"count":null,"exdates":[]}` |
| 毎週月、水、金 | `{"freq":"weekly","interval":1,"by_day":[{"weekday":"mon"},{"weekday":"wed"},{"weekday":"fri"}],"by_month":[],"by_month_day":[],"count":null,"exdates":[]}` |
| 毎月第 2 月曜 | `{"freq":"monthly","interval":1,"by_day":[{"n":2,"weekday":"mon"}],"by_month":[],"by_month_day":[],"count":null,"exdates":[]}` |
| 2 週間ごと月曜 | `{"freq":"weekly","interval":2,"by_day":[{"weekday":"mon"}],"by_month":[],"by_month_day":[],"count":null,"exdates":[]}` |
| 5 回だけ | `count` に `5` を設定 |

## フィールド

「`RecurrenceRule`」の各フィールドは以下の通りです。

- **`freq`**：頻度を指定します。`daily`、`weekly`、`monthly`、`yearly` のいずれかです。
- **`interval`**：間隔を指定します。1 以上の整数です。
- **`by_day`**：曜日を指定します。`{"weekday":"mon"}` または `{"n":2,"weekday":"mon"}` の形式です。
- **`by_month`**：対象月を指定します。1 から 12 の値です。
- **`by_month_day`**：対象日を指定します。1 から 31、`-1` は月末です。
- **`count`**：生成回数を指定します。`null` なら無制限です。
- **`exdates`**：除外日を指定します。`["2026-07-28"]` などの配列です。

## 生成後のタスクを調整する

習慣から生成されたタスクは個別に編集できます。
ただし、`status` を `in_progress`、`completed`、`skipped` に変更したタスクは、次回生成時に維持されます。
