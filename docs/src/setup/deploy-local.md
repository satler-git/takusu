# ローカルサーバーを動かす

## 起動

以下のコマンド例で、`takusu-local` を起動します。
ルートトークンは `gen-root-token` で生成した JWT を使います。

```sh
export TAKUSU_JWT_SECRET=$(openssl rand -hex 32)
TAKUSU_ROOT_TOKEN=$(cargo run -q -p takusu-cli -- gen-root-token)
export TAKUSU_ROOT_TOKEN

cargo run -p takusu-local
```

## 必要な環境変数

`takusu-local` は **JWT**（JSON Web Token。認証トークンの署名と検証に使う形式）を使います。
起動には以下の環境変数を設定します。

| 変数 | 役割 | 備考 |
|------|------|------|
| `TAKUSU_JWT_SECRET` | JWT の署名と検証 | SQLite 使用時は必須 |
| `TAKUSU_ROOT_TOKEN` | ルート認証トークン | `gen-root-token` で生成した JWT |
| `TAKUSU_DB` | SQLite ファイルパス | デフォルト `sqlite:./takusu.db` |
| `TAKUSU_BIND` | 待ち受けアドレス | デフォルト `127.0.0.1:3000` |
| `TAKUSU_TOKEN_CACHE_TTL_SECS` | トークンキャッシュの有効期間 | デフォルト `60` 秒 |

`TAKUSU_JWT_SECRET` は 32 文字以上のランダム文字列を推奨します。
`TAKUSU_ROOT_TOKEN` は `gen-root-token` で生成する JWT です。
この JWT は `TAKUSU_JWT_SECRET` で署名されます。

## ストレージ

`TAKUSU_STORAGE` でストレージバックエンドを選択します。

- `sqlite`（デフォルト）
- `workers` / `cloudflare` / `d1`（Cloudflare Worker 経由）

## バックグラウンド運用

systemd サービスや `tmux` / `screen` で常時起動しておくのが一般的です。

## 更新

```sh
git pull
cargo build -p takusu-local --release
# サービスを再起動
```

マイグレーションはアプリケーション起動時に、組み込み SQL マイグレーションとして自動的に適用されます。
