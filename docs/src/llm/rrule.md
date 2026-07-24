# 繰り返しルール

`expand_rrule` は Deferred ツールである。
使う前に `tool_search` で発見する。

takusu は 2 種類の繰り返し表現を使う。

`create_habit` や `update_habit` の `recurrence` は、RecurrenceRule の JSON 文字列である。
`expand_rrule` は、RFC 5545 の RRULE 文字列を展開して日時のリストを返す検証用ツールである。

## RecurrenceRule JSON

`create_habit` 用の JSON 形式は `habit-details.md` で説明する。

## RFC 5545 RRULE

`expand_rrule` で使えるのは以下のパートである。

- `DTSTART`（必須）
- `RRULE` 内：`FREQ`、`INTERVAL`、`COUNT`、`UNTIL`、`BYDAY`、`BYMONTH`、`BYMONTHDAY`
- `EXDATE`

以下は使えない。

- `BYHOUR`、`BYMINUTE`、`BYSECOND`、`BYSETPOS`、`BYWEEKNO`、`BYYEARDAY`、`WKST`

`DTSTART` には timezone を明示する。
UTC の場合は末尾に `Z`、現地時間の場合は `TZID` パラメータを使う。

```text
DTSTART;TZID=Asia/Tokyo:20260727T090000
RRULE:FREQ=DAILY;BYDAY=MO,TU,WE,TH,FR
EXDATE:20260729
```

## 検証

`expand_rrule` を呼んで、生成される日時が意図通りか確認する。
`count` は 1 から 1000 まで指定できる。
