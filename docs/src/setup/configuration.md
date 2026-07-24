# 設定ファイルと環境変数

## 環境変数

`takusu-local` で使う環境変数を以下に示します。

| 変数 | 必須 | デフォルト | 説明 |
|------|------|------------|------|
| `TAKUSU_JWT_SECRET` | SQLite 使用時 | - | JWT の署名と検証、ルートトークンの生成に使うシークレット |
| `TAKUSU_STORAGE` | - | `sqlite` | `sqlite` / `workers` / `cloudflare` / `d1` |
| `TAKUSU_DB` | - | `sqlite:./takusu.db` | SQLite ファイルパス |
| `TAKUSU_BIND` | - | `127.0.0.1:3000` | 待ち受けアドレス |
| `TAKUSU_WORKERS_URL` | Worker 使用時 | - | Worker エンドポイント URL |
| `TAKUSU_WORKERS_TOKEN` | Worker 使用時 | `TAKUSU_ROOT_TOKEN` | Worker 認証トークン |
| `TAKUSU_ROOT_TOKEN` | - | - | ルートトークン。`gen-root-token` で生成した JWT |
| `CARTESIA_API_KEY` | TTS 使用時 | - | Cartesia Sonic TTS API キー |
| `SENTRY_DSN` | - | - | Sentry DSN |

`CARTESIA_API_KEY` は **TTS**（Text-to-Speech。テキストを音声に変換する機能）の Cartesia Sonic API キーです。
`SENTRY_DSN` は **Sentry**（エラー監視サービス）への接続 URL です。

## `takusu-local` 設定

### Nix 起動

`takusu-local` はコマンドライン引数を解析しません。
待ち受けアドレスは `TAKUSU_BIND` 環境変数で指定します。

```sh
TAKUSU_BIND=127.0.0.1:3000 nix run .#takusu-local
```

### CLI 設定ファイル

CLI の設定ファイルは `$XDG_CONFIG_HOME/takusu/config.toml` です。

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

`jwt_secret` は `openssl rand -hex 32` または `takusu-util::jwt::generate_secret()` で生成できます。

## Solver 設定

**Solver**（スケジュールを生成する探索アルゴリズム）は、**Planner**（スケジュール生成の設定を保持するコンポーネント）の設定または環境変数で切り替えられます。

| 値 | 説明 |
|----|------|
| `sa` | Simulated Annealing + LNS + Tabu Search |
| `priority` | Priority decoder + ALNS |
| `auto` | `priority` を試し、失敗時に `sa` へフォールバック |

## 睡眠時間設定

`settings` API または CLI、モバイルの設定画面で、就寝時刻と起床時刻を設定できます。
Solver は睡眠時間を避けてスケジュールを作成します。
