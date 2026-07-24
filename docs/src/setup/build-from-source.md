# ソースからビルドする

## 前提条件

- **Nix**：パッケージマネージャー
- **Git**：バージョン管理システム

## Nix 開発環境

リポジトリをクローンしてから、**Nix** の開発シェルに入ります。

```sh
git clone https://github.com/takusu-dev/takusu
cd takusu
nix develop
```

`direnv` を使う場合は、許可コマンドを実行します。

```sh
direnv allow
```

## 型チェック

```sh
cargo check
```

## テスト

```sh
cargo nextest run --workspace
```

## ビルド

### ローカルサーバー

```sh
cargo build -p takusu-local --release
```

ビルド結果は `target/release/takusu-local` です。

### CLI

```sh
cargo build -p takusu-cli --release
```

ビルド結果は `target/release/takusu` です。

### モバイル

```sh
# 開発ビルド
TAKUSU_BUILD_VARIANT=dev nix develop --command bash -c \
  "cd mobile && npx expo run:android --device --variant debug"

# リリース APK
nix run .#build-android-apk
```

## Nix ビルド

```sh
nix build .#takusu-local
nix build .#takusu-cli
```

## 注意

公開されているビルド成果物には、ビルドした人の **Sentry DSN**（エラー監視サービス Sentry への接続 URL）や API キーが残っていることがあります。
自分の環境で使うときはソースからビルドしてください。

`.env` ファイルや `config.toml` は Git にコミットしないでください。
