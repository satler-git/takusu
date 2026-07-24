# モバイルアプリ

`takusu` のモバイルアプリは、**Expo** と **React Native** で構築されています。
ネイティブ処理は Rust 製の `takusu-android` クレートを使います。
Android の実機またはエミュレータで動作します。

## 主な画面

- [セットアップ](setup.md)
- [ホーム画面](home.md)
- [タスク詳細](task-detail.md)
- [タスク追加](task-add.md)
- [習慣ビュー](habit-view.md)
- [習慣詳細](habit-detail.md)
- [グラフビュー](graph-view.md)
- [設定](settings.md)
- [音声アシスタント](voice-assistant.md)

## 共通 UI

各画面に共通して配置されている UI は以下の通りです。

- **画面上部**：ハンバーガーメニュー、検索ボタン、同期ボタン
- **画面中央**：各ビュー固有のコンテンツ
- **画面下部**：タスク追加ボタン、タスク実行開始/Done ボタン
- **画面右側中央**：ナビゲーションボタン（上スクロール、下スクロール、カレンダー）
- **画面左側下**：ビュー切り替えボタン（task、habit、graph）
