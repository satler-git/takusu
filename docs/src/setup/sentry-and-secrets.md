# Sentry と Secret の扱い

## 注意

提供されているビルド成果物（AppImage や APK など）には、ビルドした人の Sentry DSN や API キーが埋め込まれている可能性があります。
自分の環境で運用する場合は、ソースからビルドしてください。

## Sentry

**Sentry**（アプリケーションのエラーを収集と監視するサービス）への接続情報は `SENTRY_DSN` 環境変数で設定します。

```sh
export SENTRY_DSN=https://xxx@yyy.ingest.sentry.io/zzz
cargo run -p takusu-local --release
```

モバイルでは `EXPO_PUBLIC_SENTRY_DSN` を使用します。

## API キー

API キーは `.env` や `config.toml`、環境変数で設定してください。
リポジトリにコミットしないよう注意してください。

```sh
# .envrc.example
eval "$(direnv-export-dotenv)"
```

## トークン

`takusu-local` のトークンは SHA-256 ハッシュで保存されます。
発行時にしか平文は表示されないので、控えを保管してください。

## 推奨

- `.env` ファイルを `.gitignore` に追加する
- `TAKUSU_JWT_SECRET` は 32 文字以上のランダム文字列を使う
- `TAKUSU_ROOT_TOKEN` は `cargo run -p takusu-cli -- gen-root-token` で生成した JWT を使う
