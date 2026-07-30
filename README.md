# takusu

ユーザーのスケジュールを自動構築するプランナーと、LLM 音声アシスタント。

## 特徴

- 締め切り・見積り・依存関係・並列性・諦めやすさを考慮した自動スケジューリング
- 焼きなまし法 (SA) + LNS + Tabu Search で最適化（並列リスタート・部分再スケジューリング対応）
- REST API サーバー (axum + SQLite) と Cloudflare Worker バックエンド
- CLI クライアント（エディタ編集・リッチテーブル表示）
- モバイルアプリ（Expo + React Native、UniFFI ネイティブモジュール）
- 音声アシスタント: 録音 + STT（sherpa-onnx）
- iCalendar インポート・Google Calendar 同期・習慣の RRULE 展開

## 構成

```
takusu/
├── crates/
│   ├── takusu-core/        # プランナー本体（データ型・スケジューリング算法）
│   ├── takusu-local/       # ローカルサーバー (axum + SQLite)
│   ├── takusu-local-lib/   # ビジネスロジック（サーバーと CLI の共通）
│   ├── takusu-contracts/     # プラガブル Storage trait + 共有型
│   ├── takusu-ical/        # iCalendar パーサー
│   ├── takusu-habit/       # RRULE 展開エンジン
│   ├── takusu-audio/       # 録音 + STT バックエンド + TTS トレイト
│   ├── takusu-audio-cli/   # 音声 CLI
│   ├── takusu-client/      # REST API クライアントライブラリ
│   ├── takusu-cli/         # CLI クライアント (clap)
│   ├── takusu-worker/      # Cloudflare Worker (Rust/WASM + D1)
│   ├── takusu-android/     # Android ネイティブ (UniFFI Kotlin バインディング)
│   ├── takusu-util/        # 共有ユーティリティ
│   └── google-cal/         # Google Calendar API クライアント (OAuth2)
├── mobile/                 # Expo / React Native アプリ
├── doc/
│   ├── plan/               # プランナー・機能設計 (markdown)
│   ├── mock/               # UI モック (HTML)
│   └── proposal.typ        # 設計ドキュメント (Typst・日本語)
└── scripts/                # ビルド・サーバー起動スクリプト
```

## セットアップ

```sh
nix develop   # または direnv allow
```

## コマンド

```sh
cargo check                              # 型チェック
cargo nextest run --workspace            # 全テスト
cargo nextest run -p takusu-core         # コアプランナーのテスト
cargo bench -p takusu-core               # ベンチマーク
cargo run --example daily                # サンプル実行
cargo run -p takusu-cli -- --help        # CLI クライアント
cargo run -p takusu-local                # ローカルサーバー起動
```

### 音声 CLI

```sh
cargo run -p takusu-audio-cli -- record   # マイク録音
cargo run -p takusu-audio-cli -- transcribe --backend sherpa --features sherpa audio.wav
```

### モバイルアプリ

```sh
# APK ビルド
nix develop --command bash -c "./scripts/build-android.sh"
nix develop --command bash -c "cd mobile && npx expo prebuild --platform android --no-install"
./scripts/post-prebuild-android.sh mobile/android
nix develop --command bash -c "cd mobile/android && ./gradlew :app:assembleRelease"

# 開発ビルド（実機デバッグ・Metro ホットリロード）
cd mobile
TAKUSU_BUILD_VARIANT=dev nix develop --command bash -c \
  "npx expo run:android --device --variant debug"
```

詳細は [`mobile/AGENTS.md`](mobile/AGENTS.md) を参照。

## 使い方

```sh
TAKUSU_ROOT_TOKEN=tsk_... cargo run -p takusu-local
```

## デプロイ

### ローカルサーバー (`takusu-local`)

```sh
cargo run -p takusu-local
# または
nix run .#takusu-local
```

主要な環境変数:

| 変数 | 役割 | 備考 |
|------|------|------|
| `TAKUSU_JWT_SECRET` | JWT 署名・検証用シークレット | SQLite  backend では必須 |
| `TAKUSU_STORAGE` | `sqlite` または `workers`/`cloudflare`/`d1` | デフォルト `sqlite` |
| `TAKUSU_DB` | SQLite ファイルパス | デフォルト `sqlite:./takusu.db` |
| `TAKUSU_BIND` | 待ち受けアドレス | デフォルト `127.0.0.1:3000` |
| `TAKUSU_WORKERS_URL` | Worker バックエンド URL | `TAKUSU_STORAGE=workers` 時に使用 |
| `TAKUSU_WORKERS_TOKEN` | Worker 認証トークン | 未設定時は `TAKUSU_ROOT_TOKEN` をフォールバック |
| `TAKUSU_ROOT_TOKEN` | ルートトークン（Worker 認証のフォールバック） | クライアントがルート JWT を提示すれば root 操作可能 |

### Cloudflare Worker (`takusu-worker`)

`crates/takusu-worker/wrangler.toml` の D1 `database_id` を実際のデータベース UUID に更新してから実行する。

```sh
cd crates/takusu-worker

# 必須シークレット
wrangler secret put TAKUSU_JWT_SECRET

# 任意: `wrangler.toml` の [vars] または --var フラグで設定
# TAKUSU_ALLOWED_ORIGIN="https://app.example.com"  # 空白区切りで許可 origin を制限
# TAKUSU_LOG=debug

# ビルド・マイグレーション・デプロイ
worker-build --release
wrangler d1 migrations apply takusu --remote
wrangler deploy
```

### リリースワークフロー (GitHub Actions)

`.github/workflows/release.yaml` は `v*` タグの push または手動実行で起動する。

1. **タグを切る**（推奨）

   ```sh
   ./scripts/release.sh              # v0.YYYYMMDD.n を自動生成
   ./scripts/release.sh 1.0.0        # 明示的バージョン
   ./scripts/release.sh 1.0.0 --no-push   # ローカル確認のみ
   ```

   このスクリプトは `Cargo.toml` / `Cargo.lock` / `mobile/app.json` / `mobile/package.json` のバージョンを更新し、`ts/takusu-client/openapi/openapi.json` と `ts/takusu-client/src/types.gen.ts` を再生成してから、`main` ブックマークを移動してタグを push する。

2. **GitHub Actions が実行するジョブ**

   - `deploy-worker`: Worker をビルドし、D1 マイグレーションを適用して Cloudflare にデプロイ
   - `build-cli`: `takusu-cli` を AppImage 化
   - `build-android-apk`: 署名付き APK をビルド
   - `prerelease`: 上記成果物を GitHub Releases にアップロード

3. **必要な GitHub Secrets**

   | Secret | 用途 |
   |--------|------|
   | `CLOUDFLARE_API_TOKEN` | Worker / D1 デプロイ（Workers & D1 書き込み権限） |
   | `CLOUDFLARE_ACCOUNT_ID` | Cloudflare アカウント ID |
   | `D1_DATABASE_ID` | D1 データベース UUID（`wrangler.toml` 内 `database_id` プレースホルダの上書きに使用） |
   | `TAKUSU_KEYSTORE_BASE64` | base64 エンコードされた Android リリース keystore |
   | `TAKUSU_STORE_PASSWORD` | keystore パスワード |
   | `TAKUSU_KEY_PASSWORD` | キーパスワード |
   | `TAKUSU_KEY_ALIAS` | キー alias |
   | `SENTRY_DSN` | Rust クライアント・サーバーの Sentry DSN |
   | `EXPO_PUBLIC_SENTRY_DSN` | モバイル Sentry DSN |
   | `SENTRY_AUTH_TOKEN` | モバイルソースマップアップロード用 |
   | `SENTRY_URL` / `SENTRY_ORG` / `SENTRY_PROJECT` | Sentry 設定（オプション） |

### 手動ビルド

- CLI AppImage:

  ```sh
  nix bundle --bundler github:ralismark/nix-appimage \
    .#takusu-cli -o takusu-cli-x86_64-linux.AppImage
  ```

- Android APK（リリース）:

  ```sh
  nix run .#build-android-apk
  ```

  開発ビルド / エミュレータビルドはそれぞれ `.#build-android-apk-dev` / `.#build-android-apk-emulator` を使用。

## 設計ドキュメント

[`doc/proposal.typ`](doc/proposal.typ) — プロジェクト全体の設計思想 (Typst・日本語)
[`ARCHITECTURE.md`](ARCHITECTURE.md) — クレート構成・データフロー・アルゴリズム詳細
[`AGENTS.md`](AGENTS.md) — エージェント向け必須ルール（詳細は `.devin/docs/`）

## References

- The agent loop and tool-calling abstractions are informed by the reference implementation in [pi](https://github.com/earendil-works/pi) (`packages/agent`), by Mario Zechner, used under the MIT License.
- Tool design and context management are informed by the [OpenAI function calling](https://developers.openai.com/api/docs/guides/function-calling), [tool search](https://developers.openai.com/api/docs/guides/tools-tool-search), and [programmatic tool calling](https://developers.openai.com/api/docs/guides/tools-programmatic-tool-calling) guides, and by the [Anthropic Claude tool use documentation](https://platform.claude.com/docs/en/agents-and-tools/tool-use/manage-tool-context) and [writing tools for agents](https://www.anthropic.com/engineering/writing-tools-for-agents) guide.
- The deferred-tool exposure pattern is informed by the [OpenAI Codex](https://github.com/openai/codex) reference implementation (`codex-rs/tools/src/tool_executor.rs`, `codex-rs/core/src/tools/spec_plan.rs`), used under the Apache License 2.0.

## License

MIT

Portions of the agent implementation are informed by [pi](https://github.com/earendil-works/pi) (Copyright (c) 2025 Mario Zechner), used under the MIT License.
