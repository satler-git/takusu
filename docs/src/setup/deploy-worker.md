# Cloudflare Worker にデプロイする

**takusu-worker** は Rust/WASM で書かれた **Cloudflare Worker**（Cloudflare のエッジサーバーで動くサーバーレスアプリケーション）です。
ストレージには **D1**（Cloudflare が提供する SQLite 互換のサーバーレスデータベース）を使います。

## 前提条件

- **wrangler CLI**：Cloudflare Worker を管理するコマンドラインツール
- Cloudflare アカウント
- D1 データベース

## D1 データベース作成

```sh
wrangler d1 create takusu
```

`crates/takusu-worker/wrangler.toml` の `database_id` を、作成したデータベースの UUID に更新します。

## 必須シークレット

```sh
cd crates/takusu-worker
wrangler secret put TAKUSU_JWT_SECRET
```

## 任意設定

`wrangler.toml` の `[vars]` または `--var` フラグで設定します。

```toml
TAKUSU_ALLOWED_ORIGIN = "https://app.example.com"
TAKUSU_LOG = "debug"
```

`TAKUSU_ALLOWED_ORIGIN` は空白区切りで許可する origin を制限します。

## ビルド、マイグレーション、デプロイ

```sh
cd crates/takusu-worker
worker-build --release
wrangler d1 migrations apply takusu --remote
wrangler deploy
```

Nix 開発シェルには `worker-build` が含まれているため、通常は `cargo install` は不要です。

## ローカルサーバーから Worker を使う

```sh
TAKUSU_STORAGE=workers \
TAKUSU_WORKERS_URL=https://takusu-worker.xxx.workers.dev \
TAKUSU_WORKERS_TOKEN=<worker-token> \
cargo run -p takusu-local
```

## GitHub Actions による自動デプロイ

`.github/workflows/release.yaml` の `deploy-worker` ジョブを参照してください。
