# CLI コマンド一覧

## グローバルオプション

```sh
takusu [OPTIONS] <SUBCOMMAND>
```

| オプション | 説明 |
|------------|------|
| `--mode <rich|simple>` | 表示モード |
| `--tz <TZ>` | タイムゾーン（`Asia/Tokyo` 等） |
| `-h, --help` | ヘルプ |
| `-V, --version` | バージョン |

## トップレベルコマンド

| コマンド | 説明 |
|----------|------|
| `health` | サーバー稼働確認（トークン不要） |
| `gen-root-token` | JWT 形式のルートトークンを生成して出力 |
| `completion <SHELL>` | シェル補完スクリプトを生成 |
| `config <SUBCOMMAND>` | 設定ファイルの表示、初期化、更新 |
| `tui` | 対話式 TUI を起動 |
| `web` | Web UI サーバーを起動（`web` feature 使用時） |
| `mcp` | MCP サーバーを stdio で起動（`mcp` feature 使用時） |
| `agent <SUBCOMMAND>` | エージェントアシスタント |
| `license` | サードパーティライセンスを表示 |

`tui` は **TUI**（Text User Interface。端末上で対話的に操作するインターフェース）を起動します。
`web` は Web UI サーバーを起動します（`web` feature 使用時）。
`mcp` は **MCP**（Model Context Protocol。AI エージェントがツールを呼び出すためのプロトコル）サーバーを stdio で起動します。

## `task`

| コマンド | 説明 |
|----------|------|
| `task list [--status <status>]` | タスク一覧 |
| `task show <id>` | タスク詳細 |
| `task create [options]` | タスク作成。`--title`, `--end-at`, `--avg-time` が必要 |
| `task edit <id>` | エディタで編集 |
| `task update <id> [options]` | フィールド更新 |
| `task replace <id> [options]` | 完全に置換 |
| `task delete <id>` | 削除 |
| `task status <id> <status>` | 状態変更 |
| `task import-ical <file>` | iCal インポート。ファイルまたは `-` で標準入力 |
| `task deps-check` | 冗長な依存辺の検出と削除の提案 |
| `task work <SUBCOMMAND>` | 作業セッション関連コマンド |

`task create` の主なオプション:

- `--title <TITLE>`（必須）
- `--end-at <datetime>`（必須）
- `--avg-time <duration>`（必須、`30m`, `1h30m`, `2h` 等）
- `--sigma-time <duration>`（デフォルト `0`）
- `--start-at`, `--description`, `--depends`, `--parallelizable`, `--allows-parallel`, `--abandonability`, `--fixed`
- `--quantity-total`, `--quantity-done`, `--quantity-unit`, `--original-quantity-total`

`task list` では `--from`, `--until`, `--status`, `--no-overdue`, `--habit-id`, `--ical-uid`, `--limit`, `--query` などで絞り込めます。

### `task work`

| サブコマンド | 説明 |
|--------------|------|
| `task work start <id>` | 作業開始（`in_progress`） |
| `task work pause <id>` | 作業一時停止 |
| `task work complete <id>` | 作業完了（`completed`） |
| `task work progress <id> --quantity <n> [--note <text>]` | 進捗記録 |
| `task work progress-show <id>` | 進捗履歴表示 |
| `task work split <id> --retained-quantity <n> [options]` | タスク分割 |

## `schedule`

| コマンド | 説明 |
|----------|------|
| `schedule get` | 現在のスケジュールを取得 |
| `schedule generate` | スケジュール生成。`--task-ids`/`--sleep` を指定可能 |
| `schedule reschedule --mode <mode> [options]` | 部分再スケジュール |
| `schedule move <id> --start-at <datetime> [--force]` | タスク移動 |
| `schedule clear` | スケジュールクリア |

`schedule reschedule` の `--mode`:

- `range`: 範囲指定。`--from` と `--until` が必要
- `tasks`: 指定タスクのみ。`--task-ids` が必要

`--pinned` で固定するタスクを追加指定でき、`--sleep` で睡眠時間を指定できます（デフォルト `recommended`）。

## `habit`

| コマンド | 説明 |
|----------|------|
| `habit list` | 習慣一覧 |
| `habit show <id>` | 習慣詳細 |
| `habit create [options]` | 習慣作成。`--title`, `--recurrence`, `--start-time`, `--end-time`, `--avg-time` が必要 |
| `habit edit <id>` | エディタで編集 |
| `habit update <id> [options]` | 更新 |
| `habit replace <id> [options]` | 完全に置換 |
| `habit delete <id>` | 削除 |
| `habit scheduled-spans <SUBCOMMAND>` | 習慣の予定 span 管理 |
| `habit steps <SUBCOMMAND>` | 習慣のステップ管理 |
| `habit steps-check <id>` | ステップ依存の冗長辺検出 |

`--recurrence` には `takusu_habit::RecurrenceRule` の JSON を渡します。
例:

```json
{"freq":"daily","interval":1,"by_day":[],"by_month":[],"by_month_day":[],"count":null,"exdates":[]}
```

`habit create` / `habit update` / `habit replace` には `--window`（`day` または `period`）、`--fixed`、`--parallelizable`、`--allows-parallel`、`--active` などのオプションがあります。

### `habit scheduled-spans`

| サブコマンド | 説明 |
|--------------|------|
| `habit scheduled-spans add <id> --from <date> --to <date> [--reason <text>]` | span 追加 |
| `habit scheduled-spans list <id>` | 該当 habit の span 一覧 |
| `habit scheduled-spans list-all` | 全 habit の span 一覧 |
| `habit scheduled-spans remove <id> <span_id>` | span 削除 |

### `habit steps`

| サブコマンド | 説明 |
|--------------|------|
| `habit steps list <id>` | 該当 habit のステップ一覧 |
| `habit steps list-all` | 全 habit のステップ一覧 |
| `habit steps edit <id>` | `$EDITOR` でステップ編集（JSON 配列） |
| `habit steps set <id> <file>` | ファイルまたは `-` からステップを設定 |

## `token`

| コマンド | 説明 |
|----------|------|
| `token create [--label <label>]` | 新規トークン発行 |
| `token list` | トークン一覧 |
| `token revoke <id>` | トークン失効。`id` は整数 |

## `sync`

| コマンド | 説明 |
|----------|------|
| `sync settings` | 同期設定確認 |
| `sync setup --refresh-token <token>` | refresh_token 設定。`--calendar-id`/`--client-id`/`--client-secret`/`--enabled`/`--no-ask` も指定可能 |
| `sync login --client-id <id> --client-secret <secret>` | Google Calendar への OAuth2 ログイン。`--calendar-id`/`--port`/`--no-browser` を指定可能 |
| `sync trigger` | 即時同期 |
| `sync delete-all` | Google Calendar イベントをすべて削除 |
| `sync mappings` | Google Calendar イベントマッピング一覧 |

`sync login` は **OAuth2**（Google アカウントへのアクセス権限を安全に委任する認可フレームワーク）を使って Google Calendar にログインします。

## `memory`

| コマンド | 説明 |
|----------|------|
| `memory search <query> [options]` | 記憶検索。`--kind`/`--subject-type`/`--subject-id`/`--limit` を指定可能 |
| `memory show <id>` | 記憶表示 |
| `memory create <kind> <key> <content> [options]` | 記憶作成。`--subject-type`/`--upsert` を指定可能 |
| `memory update <id> --revision <n> --content <content>` | 記憶更新 |
| `memory delete <id> --revision <n>` | 記憶削除 |
| `memory similar <title> [--limit]` | 類似した完了タスクを検索 |

## `skill`

| コマンド | 説明 |
|----------|------|
| `skill list` | スキル一覧 |
| `skill show <slug>` | スキル詳細 |
| `skill create [options]` | スキル作成 |
| `skill update <slug> [options]` | スキル更新 |
| `skill delete <slug>` | スキル削除 |

## `config`

| サブコマンド | 説明 |
|--------------|------|
| `config show` | 設定ファイルのパスと内容を表示 |
| `config init` | デフォルト設定ファイルを作成 |
| `config set [options]` | 設定値を更新 |
| `config workers set --url <url> --token <token>` | Worker エンドポイントとトークンの設定 |
| `config workers health` | Worker ストレージ疎通確認 |

`config set` で設定できる主な項目:

- `--storage`, `--db`, `--worker-url`, `--workers-token`, `--root-token`, `--tz`, `--sleep-start`, `--sleep-end`
- `--comfortable <hours>`, `--maximum <hours>`, `--solver <sa|priority|auto>`, `--time-budget-ms <ms>`, `--seed <n>`, `--warm-start <bool>`

## `agent`

| サブコマンド | 説明 |
|--------------|------|
| `agent run [--text <text>] [--yes] [--allow <perm>] [--deny <perm>]` | エージェントアシスタントを実行 |
| `agent config show` | エージェント設定表示 |
| `agent config set <key> <value>` | エージェント設定更新 |
| `agent config permissions <SUBCOMMAND>` | エージェント権限管理 |

### `agent config permissions`

| サブコマンド | 説明 |
|--------------|------|
| `agent config permissions show` | 権限一覧 |
| `agent config permissions set <key> <value>` | 権限設定 |
| `agent config permissions unset <key>` | 権限削除 |
