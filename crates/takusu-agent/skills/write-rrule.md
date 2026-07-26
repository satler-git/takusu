+++
name = "write-rrule"
description = "Generate RFC 5545 RRULE strings for takusu recurring habits."
+++

# RRULE の書き方

`takusu` は RFC 5545 の RRULE を繰り返し習慣（habit）の定義に使う。エージェントがユーザーからの自然言語を RRULE に変換するときは、以下のガイドに従うこと。

## 対応している構文

- プロパティ: `DTSTART`、`RRULE`、`EXDATE`
- `RRULE` 内のパート: `FREQ`、`INTERVAL`、`COUNT`、`UNTIL`、`BYDAY`、`BYMONTH`、`BYMONTHDAY`
- 未対応のパート（`BYSETPOS`、`BYHOUR`、`BYMINUTE`、`BYSECOND`、`BYWEEKNO`、`BYYEARDAY`、`WKST` など）は使わないこと。

## 基本構成

```text
DTSTART;TZID=Asia/Tokyo:20260727T090000
RRULE:FREQ=DAILY;BYDAY=MO,TU,WE,TH,FR
EXDATE:20260729
```

- `DTSTART` は必須と同等。省略するとサーバー timezone の当日 0 時が使われるため、開始日時があいまいになる。
- UTC の場合は末尾に `Z` を付ける: `DTSTART:20260727T090000Z`
- 現地時間の場合は `TZID` パラメータを使う: `DTSTART;TZID=Asia/Tokyo:20260727T090000`
- `DTSTART` と `RRULE` は改行で区切る。`EXDATE` は省略可能。

## 各パートの意味

- `FREQ`: `DAILY`、`WEEKLY`、`MONTHLY`、`YEARLY` のいずれか。
- `INTERVAL`: `n` 回り目に 1 回発生。`FREQ=DAILY;INTERVAL=2` は 2 日に 1 回。デフォルトは 1。
- `BYDAY`: 曜日指定。`MO,TU,WE,TH,FR,SA,SU`。`2MO` は第 2 月曜、`-1FR` は最終金曜。
- `BYMONTH`: `1,3,5` のように 1〜12 の月を列挙。
- `BYMONTHDAY`: `1,15,-1` のように日付を列挙。`-1` は月末。
- `COUNT`: 繰り返し回数。`COUNT=0` は無効。
- `UNTIL`: 終了日。`YYYYMMDD` または `YYYYMMDDTHHMMSS`、UTC の場合は末尾に `Z`。
- `EXDATE`: 除外する日付。`YYYYMMDD` または `YYYYMMDDTHHMMSS`。

## よくあるパターン

### 平日の 9:00（Asia/Tokyo）

```text
DTSTART;TZID=Asia/Tokyo:20260727T090000
RRULE:FREQ=DAILY;BYDAY=MO,TU,WE,TH,FR
```

### 毎週月曜と金曜の 18:00 UTC

```text
DTSTART:20260727T180000Z
RRULE:FREQ=WEEKLY;BYDAY=MO,FR
```

### 2 週に 1 回の水曜

```text
DTSTART:20260729T120000Z
RRULE:FREQ=WEEKLY;INTERVAL=2;BYDAY=WE
```

### 毎月第 2・第 4 火曜の 19:00（Asia/Tokyo）

```text
DTSTART;TZID=Asia/Tokyo:20260714T190000
RRULE:FREQ=MONTHLY;BYDAY=2TU,4TU
```

### 毎月最終日の 23:00 UTC

```text
DTSTART:20260731T230000Z
RRULE:FREQ=MONTHLY;BYMONTHDAY=-1
```

### 毎年 1 月 1 日の 0:00（Asia/Tokyo）

```text
DTSTART;TZID=Asia/Tokyo:20260101T000000
RRULE:FREQ=YEARLY;BYMONTH=1;BYMONTHDAY=1
```

### 5 回だけ、うち 1 日を除外

```text
DTSTART:20260727T090000Z
RRULE:FREQ=DAILY;COUNT=5
EXDATE:20260729
```

## ユーザー意図から RRULE への変換手順

1. **開始日時を決める**。 timezone は `Asia/Tokyo` や `UTC` など、ユーザーの環境を使う。
2. **基本頻度を選ぶ**。
   - 「毎日」→ `FREQ=DAILY`
   - 「毎週 X 曜日」→ `FREQ=WEEKLY;BYDAY=...`
   - 「毎月 X 日 / 第 n X 曜日」→ `FREQ=MONTHLY;BYMONTHDAY=...` または `BYDAY=...`
   - 「毎年 X 月 X 日」→ `FREQ=YEARLY;BYMONTH=...;BYMONTHDAY=...`
3. **間隔を適用**。「2 日に 1 回」なら `INTERVAL=2`。
4. **終了条件を決める**。ユーザーが回数を指定したら `COUNT`、日付を指定したら `UNTIL`、無期限なら省略。
5. **除外日があれば `EXDATE` で追加**。

## よくある間違い

生成結果がユーザー意図と異なる場合は、特に `BYDAY`、`BYMONTHDAY`、`INTERVAL`、`DTSTART` の timezone を見直すこと。
