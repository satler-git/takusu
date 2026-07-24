# 習慣

takusu における「習慣」は、繰り返し発生する予定の元になります。

`takusu-habit` では **`RecurrenceRule`**（JSON 形式の繰り返しルール）という構造体で繰り返しパターンを表現します。
時刻は `start_time` と `end_time` で分離して指定します。
「習慣」はスケジュール生成時に個別のタスクへ展開されます。

## RecurrenceRule

「RecurrenceRule」は次のフィールドを持つ JSON です。

| フィールド | 型 | 説明 |
|------------|-----|------|
| `freq` | string | `daily`, `weekly`, `monthly`, `yearly` |
| `interval` | integer | 間隔（1 以上） |
| `by_day` | array | 曜日指定。要素は `{"weekday":"mon"}` または `{"n":2,"weekday":"mon"}` |
| `by_month` | array | 対象月（1〜12） |
| `by_month_day` | array | 対象日（1〜31、`-1` は月末） |
| `count` | integer/null | 生成回数 |
| `exdates` | array | 除外日（`["2026-07-28"]` など） |

## 例

```json
{"freq":"daily","interval":1,"by_day":[],"by_month":[],"by_month_day":[],"count":null,"exdates":[]}
```

| 目的 | JSON |
|------|------|
| 毎日 | `{"freq":"daily","interval":1,...}` |
| 毎週月、水、金 | `{"freq":"weekly","interval":1,"by_day":[{"weekday":"mon"},{"weekday":"wed"},{"weekday":"fri"}],...}` |
| 毎月第 2 月曜 | `{"freq":"monthly","interval":1,"by_day":[{"n":2,"weekday":"mon"}],...}` |
| 2 週間ごと月曜 | `{"freq":"weekly","interval":2,"by_day":[{"weekday":"mon"}],...}` |
| 5 回だけ | `count` に `5` を設定 |

## 時刻の指定

「RecurrenceRule」はあくまで **日付単位** のパターンです。
時刻は習慣の `start_time` と `end_time`（`HH:MM`）で指定します。

```sh
cargo run -p takusu-cli -- habit create \
  --title "朝のランニング" \
  --recurrence '{"freq":"daily","interval":1,"by_day":[],"by_month":[],"by_month_day":[],"count":null,"exdates":[]}' \
  --start-time "06:00" \
  --end-time "06:30" \
  --avg-time 30m
```

## 習慣からタスクへ

`schedule generate` を実行すると、RecurrenceRule に基づいてタスクが生成されます。
生成されたタスクは `habit_id` を持ち、元の習慣と紐づきます。

## 生成後のタスクを調整する

習慣から生成されたタスクを個別に編集できます。
ただし、`status` を `in_progress`、`completed`、`skipped` に変更したタスクは、次回生成時に維持されます。

## RFC 5545 RRULE との関係

`takusu-habit` は内部的に RFC 5545 のサブセットをパースできます（`FREQ`、`INTERVAL`、`COUNT`、`UNTIL`、`BYDAY`、`BYMONTH`、`BYMONTHDAY`、`EXDATE`、`DTSTART`）。
ただし、習慣を作成する API や CLI では、JSON 形式の「RecurrenceRule」を直接受け付けます。
`BYHOUR` と `BYMINUTE` はサポートされていません。
時刻は `start_time` と `end_time` で指定してください。

## 詳細

- [習慣の管理](../guide/habits.md)
- [RRULE ツール](../llm/rrule.md)
