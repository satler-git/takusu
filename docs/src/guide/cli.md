# CLI

**`takusu-cli`** は、takusu のコマンドラインクライアントです。
**`takusu-local-lib`** を直接使用するため、ネットワークなしで動作します。

## 設定

CLI の設定は `$XDG_CONFIG_HOME/takusu/config.toml` に保存されます。

```toml
# storage = "sqlite"
# db = "sqlite:./takusu.db"
# worker_url = "http://127.0.0.1:8787"
# workers_token = "eyJ..."
# root_token = "eyJ..."
# jwt_secret = "<openssl rand -hex 32 で生成>"
# tz = "Asia/Tokyo"
# sleep_start = "22:00"
# sleep_end = "06:00"
```

表示モードは `--mode rich` または `--mode simple` で切り替えます。

## 表示モード

表示モードは **`rich`** と **`simple`** の 2 つです。

- **`rich`**：`comfy-table` を使ったカラフルなテーブル表示
- **`simple`**：プレーンテキスト表示

## サブコマンド一覧

### task

task サブコマンドは、タスクの一覧、表示、作成、編集、更新、削除を行います。

```sh
takusu task list
takusu task show <id>
takusu task create --title "買い物" --end-at 2026-07-28T18:00 --avg-time 30m
takusu task edit <id>
takusu task update <id> --status completed
takusu task delete <id>
```

### schedule

schedule サブコマンドは、スケジュールの取得、生成、再計算、タスクの手動移動、クリアを行います。

```sh
takusu schedule get
takusu schedule generate
takusu schedule reschedule --mode range --from <datetime> --until <datetime>
takusu schedule move <id> --start-at <datetime> [--force]
takusu schedule clear
```

### habit

habit サブコマンドは、習慣の一覧、作成、編集、更新、削除を行います。

```sh
takusu habit list
takusu habit create --title "朝ラン" \
  --recurrence '{"freq":"daily","interval":1,"by_day":[],"by_month":[],"by_month_day":[],"count":null,"exdates":[]}' \
  --start-time 06:00 --end-time 06:30 --avg-time 30m
takusu habit edit <id>
takusu habit update <id> --recurrence '{"freq":"weekly","interval":1,"by_day":[{"weekday":"mon"},{"weekday":"wed"},{"weekday":"fri"}],"by_month":[],"by_month_day":[],"count":null,"exdates":[]}'
takusu habit delete <id>
```

### token

token サブコマンドは、API トークンの作成、一覧、失効を行います。

```sh
takusu token create
takusu token list
takusu token revoke <id>
```

### sync

sync サブコマンドは、カレンダー同期の設定、認証、手動実行を行います。

```sh
takusu sync settings
takusu sync setup --refresh-token <TOKEN>
takusu sync login --client-id <ID> --client-secret <SECRET>
takusu sync trigger
```

### エージェント・TUI・その他

その他にも、以下のようなコマンドがあります。

- `takusu agent run`：LLM エージェントを対話で実行します。`--text` で初期指示、`--yes` で承認を省略できます。
- `takusu tui`：対話式 TUI を起動します。
- `takusu web`：Web UI サーバーを起動します（`web` feature 使用時）。
- `takusu skill`：スキルの作成・更新・削除・一覧表示を行います。
- `takusu memory`：記憶の作成・検索・更新・削除を行います。
- `takusu config`：設定ファイルの表示・初期化・更新を行います。
- `takusu health`：サーバーの稼働確認を行います。
- `takusu gen-root-token`：ルートトークンを生成します。
- `takusu completion`：シェル補完スクリプトを生成します。
- `takusu license`：サードパーティライセンスを表示します。
- `takusu mcp`：MCP サーバーを stdio で起動します（`mcp` feature 使用時）。

## エディタ編集

`task edit` は、一時ファイルにタスクを書き出して編集します。
`$EDITOR`（デフォルト `vi`）で保存後、CLI がパースして更新します。
`#` で始まる行はコメントとして無視されます。

## 所要時間の指定

`--avg-time` と `--sigma-time` は、`30m` や `1h30m`、`2h` などの長さ表現を受け付けます。
`-t` や `--time` といったオプションはありません。
`--sigma-time` を省略するか `0` を指定すると、自動的に `avg/5` が使われます。
