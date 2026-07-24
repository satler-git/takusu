# 日付の詳細情報

`day_details` は Direct ツールである。

`day_details` は、指定した日付の曜日、日本の祝日情報、およびその日のスケジュールを返す。

## パラメータ

- `dates`：日付表現の配列。`YYYY-MM-DD`、`today`、`tomorrow`、`3d`、`-3d` など
- `include_schedule`：true の場合、各日のスケジュールも返す

## 返り値

各日付について以下の情報を返す。

- `date`：ISO 8601 日付
- `weekday`：曜日（`月` から `日`）
- `is_holiday`：祝日かどうか
- `holiday_name`：`is_holiday` が true の場合に含まれる祝日名
- `schedule`：`include_schedule=true` の場合のみ含まれるスケジュール配列

`schedule` 配列の要素は `task_id`、`title`、`start_at`、`end_at`、`status` を含む。
日をまたぐ予定は、その日と重なる部分だけが含まれる。
