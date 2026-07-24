# タスクを管理する

## タスクを作成する

`task create` で新しいタスクを作成できます。

```sh
cargo run -p takusu-cli -- task create \
  --title "レポートを書く" \
  --end-at "2026-07-28T18:00:00" \
  --avg-time 2h \
  --sigma-time 30m
```

## タスクを更新する

`task update` でタスクの状態を更新できます。

```sh
cargo run -p takusu-cli -- task update <id> --status in_progress
```

## タスクを編集する

`task edit` で `$EDITOR` を開き、ファイルを保存すると PATCH リクエストが送信されます。

```sh
cargo run -p takusu-cli -- task edit <id>
```

## タスクを削除する

`task delete` でタスクを削除できます。

```sh
cargo run -p takusu-cli -- task delete <id>
```

## その他の操作

- `task status <id> <status>`：タスクの状態を直接変更します。
- `task replace <id>`：既存のタスクを完全に置き換えます。
- `task work <subcommand>`：作業セッションを管理します。`start`/`pause`/`complete`/`progress`/`progress-show`/`split` があります。
- `task deps-check`：冗長な依存辺を検出して整理します。
- `task import-ical <file>`：iCalendar ファイルから固定予定をインポートします。
- `task list` では `--from`/`--until`/`--status`/`--habit-id`/`--query` などで絞り込めます。

## タスクの状態

タスクは以下の状態を持ちます。

| 状態 | 説明 |
|------|------|
| `pending` | まだスケジュールされていない |
| `scheduled` | スケジュール済み |
| `in_progress` | 作業中 |
| `completed` | 完了 |
| `skipped` | スキップ |

## 検索

CLI では `--status` などのオプションでフィルタできます。
詳細な修飾子については、**[検索](../llm/search.md)**を参照してください。

## 注意点

`schedule generate` には `pending` と `scheduled` のタスクのみが含まれます。
`completed` や `skipped` のタスクはスケジュールに含まれません。
