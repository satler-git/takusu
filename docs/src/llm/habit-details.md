# 習慣の steps と scheduled spans

習慣は、繰り返し発生する作業を表す。
`takusu-agent` は `create_habit`、`update_habit`、`habit_scheduled_spans` を通じて習慣を操作する。
`create_habit` と `update_habit` は Direct である。
`habit_scheduled_spans` は Deferred である。
ここではスキーマだけでは伝わらない習慣固有のルールをまとめる。

## recurrence フィールドは JSON 文字列

`create_habit` と `update_habit` の `recurrence` は、**RecurrenceRule** の JSON を文字列にシリアライズしたものを渡す。

RecurrenceRule の構造は次の通り。

- `freq`：`daily`、`weekly`、`monthly`、`yearly` のいずれか
- `interval`：間隔。デフォルトは 1
- `by_day`：曜日指定の配列。要素は `{"weekday": "mon"}` または `{"n": 2, "weekday": "tue"}` の形式
- `by_month`：1 から 12 の月を列挙する配列
- `by_month_day`：日付を列挙する配列。`-1` は月末
- `count`：繰り返し回数。`null` の場合は無期限
- `exdates`：除外する日付（`YYYY-MM-DD`）の配列

`weekday` は `mon`、`tue`、`wed`、`thu`、`fri`、`sat`、`sun` の 3 文字表記を使う。
`by_month_day` に負の値を入れると、月末から数えた日付を表す。

```json
{
  "freq": "daily",
  "interval": 1,
  "by_day": [],
  "by_month": [],
  "by_month_day": [],
  "count": null,
  "exdates": []
}
```

`create_habit` を呼ぶときは、この JSON をエスケープして `recurrence` 文字列に入れる。

```json
{
  "title": "朝のランニング",
  "recurrence": "{\"freq\":\"daily\",\"interval\":1,\"by_day\":[],\"by_month\":[],\"by_month_day\":[],\"count\":null,\"exdates\":[]}",
  "start_time": "06:00",
  "end_time": "06:30",
  "avg_minutes": 30
}
```

`start_time` と `end_time` は `HH:MM` 形式で指定する。
これは生成されるタスクの時刻であり、`recurrence` の `DTSTART` 時刻とは別に扱われる。
`recurrence` には `BYHOUR` や `BYMINUTE` を含めない。

## window_mode

`window_mode` は生成されるタスクの時間枠を決める。

- `day`（デフォルト）：発生当日の `start_time` から `end_time` まで
- `period`：現在の発生時刻から次の発生時刻まで

`period` にすると、一週間や一ヶ月に一度の習慣でもその区間全体が作業可能期間となる。
`period` を使うとき、複数日にまたがる時間枠になりうる。

## steps

習慣に `steps` を含めると、各発生が複数の step に分かれる。
step があるとき、スケジューラは habit レベルの `start_time`、`end_time`、`avg_minutes`、`sigma_minutes`、`parallelizable`、`allows_parallel`、`abandonability` を無視し、step ごとの値を使う。
そのため `get_habit` の返り値からは、step がある habit ではこれらのフィールドが省略される。

step の `position` は **1-indexed** の表示位置である。
`update_habit` では、既存の step と同じ `position` の要素は更新され、存在しない `position` は新規作成される。
`depends_on` には、依存先 step の `position` を整数配列で指定する。

```json
{
  "steps": [
    {
      "position": 1,
      "title": "ウォームアップ",
      "start_time": "06:00",
      "end_time": "06:10",
      "avg_minutes": 10
    },
    {
      "position": 2,
      "title": "本番",
      "start_time": "06:10",
      "end_time": "06:30",
      "avg_minutes": 20,
      "depends_on": [1]
    }
  ]
}
```

## scheduled spans

`habit_scheduled_spans` は `action` によって list、create、delete を切り替える。

- `action=list`：既存の span を一覧する
- `action=create`：新しい span を追加する提案を生成する
- `action=delete`：既存の span を削除する提案を生成する

span の意味は `active` フラグによって変わる。

- `active=true` の habit：`start_date` から `end_date` までは実行を休む期間（pause）
- `active=false` の habit：`start_date` から `end_date` までは実行を有効にする期間（activation window）

`action=delete` には `action=list` で返される `id` を `span_id` として渡す。
`start_date` と `end_date` は `YYYY-MM-DD` 形式で、`end_date` は `start_date` 以降である必要がある。

## active フラグ

`active` は習慣が有効かどうかを表す。
`active=false` にすると、スケジュール生成で新しいタスクが作られなくなる。
既に生成されたタスクには影響しない。
