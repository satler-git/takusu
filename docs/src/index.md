# takusu ドキュメント

**takusu** は、ユーザーのタスクと予定からスケジュールを自動構築するプランナーです。
LLM（大規模言語モデル）を UI として使う音声アシスタントも備えます。

このドキュメントは、**自分で 1 ユーザー 1 デプロイ** を前提としたエンドユーザー向けガイドです。
ビルド方法、デプロイ手順、操作方法、音声アシスタントの使い方を順を追って説明します。

## 機能

takusu が提供する機能は次の通りです。

- 締切、見積り、依存関係、並列性、諦めやすさを考慮した自動スケジューリングを行う。
- Solver モードを切り替える。
- REST API サーバー（`takusu-local`）と Cloudflare Worker（`takusu-worker`）を提供する。
- CLI クライアント（`takusu-cli`）を提供する。
- 対話式 TUI（`takusu tui`）と Web UI（`takusu web`）を提供する。
- Expo / React Native のモバイルアプリを提供する。
- LLM エージェント（`takusu agent`）による音声・テキスト対話、記憶、スキル、ツール呼び出しを行う。
- 音声アシスタント：録音からテキスト化（STT）し、テキストを音声で応答（TTS）する対話を行う。
- iCalendar インポートと Google Calendar 同期を行う。
- 習慣を JSON 形式の繰り返しルールで展開する。

## 構成

takusu は次のクレートとアプリで構成されています。

- `takusu-core`：スケジューリングエンジンです。
- `takusu-local`：ローカル REST API サーバー（axum + SQLite）です。
- `takusu-local-lib`：ビジネスロジックを担当します。
- `takusu-client`：HTTP クライアントライブラリです。
- `takusu-cli`：CLI クライアントです。
- `takusu-worker`：Cloudflare Worker（Rust/WASM + D1（Cloudflare のサーバーレス SQLite データベース））です。
- `takusu-audio`：録音、STT、TTS を扱うライブラリです。
- `takusu-audio-cli`：音声機能の内部テスト用 CLI です（エンドユーザー向けではありません）。
- `takusu-agent`：LLM エージェント本体です。
- `takusu-search`：全文検索エンジンです。
- `takusu-tui`：対話式 TUI です。
- `takusu-web`：Web UI サーバーです。
- `takusu-storage`：データモデルとストレージ抽象です。
- `mobile/`：Expo / React Native のモバイルアプリです（Rust の `takusu-android` をネイティブレイヤーに使います）。
- `google-cal`：Google Calendar API クライアントです。

## 次のステップ

- [クイックスタート](quick-start.md)：最短で動かす。
- [コンセプト](concepts/index.md)：タスク、習慣、スケジュールの考え方。
- [セットアップ](setup/index.md)：ビルドからデプロイまで。
