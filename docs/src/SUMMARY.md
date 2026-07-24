# Summary

[takusu ドキュメント](index.md)
[クイックスタート](quick-start.md)

---

# コンセプト

- [はじめに](concepts/index.md)
- [タスク](concepts/task.md)
- [習慣](concepts/habit.md)
- [スケジュール](concepts/schedule.md)
- [Solver](concepts/solver.md)
- [カレンダー連携](concepts/calendar-sync.md)
- [音声アシスタント](concepts/voice-assistant.md)

# 使い方

- [CLI](guide/cli.md)
- [タスクを管理する](guide/tasks.md)
- [習慣を管理する](guide/habits.md)
- [スケジュールを扱う](guide/schedule.md)
- [カレンダーと同期する](guide/sync.md)
- [モバイルアプリ](guide/mobile/index.md)
  - [セットアップ](guide/mobile/setup.md)
  - [ホーム画面](guide/mobile/home.md)
  - [タスク詳細](guide/mobile/task-detail.md)
  - [タスク追加](guide/mobile/task-add.md)
  - [習慣ビュー](guide/mobile/habit-view.md)
  - [習慣詳細](guide/mobile/habit-detail.md)
  - [グラフビュー](guide/mobile/graph-view.md)
  - [設定](guide/mobile/settings.md)
  - [音声アシスタント](guide/mobile/voice-assistant.md)

# セットアップとデプロイ

- [はじめに](setup/index.md)
- [ソースからビルドする](setup/build-from-source.md)
- [ローカルサーバーを動かす](setup/deploy-local.md)
- [Cloudflare Worker にデプロイする](setup/deploy-worker.md)
- [Google Calendar 連携の設定](setup/google-calendar.md)
- [設定ファイルと環境変数](setup/configuration.md)
- [Sentry と Secret の扱い](setup/sentry-and-secrets.md)

# リファレンス

- [環境変数一覧](reference/environment-variables.md)
- [CLI コマンド一覧](reference/cli-commands.md)
- [REST API 概要](reference/api-overview.md)

# LLM / Agent 向け

- [はじめに](llm/index.md)
- [takusu-agent の動作と承認](llm/overview.md)
- [タスクの扱い](llm/tasks.md)
- [習慣の steps と scheduled spans](llm/habit-details.md)
- [スケジュールの生成と調整](llm/schedule.md)
- [作業セッションと進捗](llm/progress.md)
- [検索クエリの構文](llm/search.md)
- [繰り返しルール](llm/rrule.md)
- [記憶の検索と保存](llm/memory.md)
- [スキルの形式と活用](llm/skills.md)
- [曖昧な入力の確認](llm/clarification.md)
- [日付の詳細情報](llm/day-details.md)
