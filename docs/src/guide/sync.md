# カレンダーと同期する

## Google Calendar 同期

**Google Calendar** との連携は、**OAuth2** という認可プロトコルを使って行います。
認証は CLI またはモバイルアプリのどちらかで行えます。

### 1. Google Cloud Console で OAuth2 クライアントを作成

1. [Google Cloud Console](https://console.cloud.google.com/) でプロジェクトを作成します。
2. 「API とサービス」→「認証情報」→「OAuth クライアント ID」を作成します。
3. アプリケーションタイプは **Web アプリケーション** を選択します。
4. 許可されたリダイレクト URI に `http://127.0.0.1:8765/callback` を追加します（CLI の `sync login` 用）。
5. クライアント ID とクライアントシークレットを取得します。
6. モバイルアプリを使う場合は、同じプロジェクトに **Android** クライアント ID も作成し、パッケージ名と SHA-1 を登録します。

### 2. CLI で認証

```sh
cargo run -p takusu-cli -- sync login --client-id <ID> --client-secret <SECRET>
```

ブラウザが開き、認証後に **`refresh_token`** が取得されます。
**`refresh_token`** は、以降の同期でアクセストークンを再発行するために使います。
この値は `tokens` テーブルに保存されます。

### 3. モバイルアプリで認証

モバイルの設定画面でクライアント ID とクライアントシークレットを保存してから、「Googleでログイン」を押します。
端末のネイティブ Google サインインが起動し、認可コードをバックエンドへ送信して `refresh_token` を取得します。

### 4. 同期を実行

```sh
cargo run -p takusu-cli -- sync trigger
```

`sync trigger` を実行すると同期が始まります。
また、`schedule generate` または `schedule move` 実行時にも自動的に同期されます。

`sync delete-all` で Google Calendar 側の takusu 同期イベントをすべて削除できます。
`sync mappings` で Google Calendar イベントとのマッピング一覧を確認できます。
`sync login` では `--calendar-id` や `--port`、`--no-browser` などを指定できます。

## iCalendar インポート

**iCalendar** は、カレンダー情報を表す標準ファイル形式です。
iCalendar ファイルをインポートすると、固定予定としてタスクを登録できます。

```sh
# ファイルパスを指定
cargo run -p takusu-cli -- task import-ical calendar.ics

# 標準入力から読み込む
cargo run -p takusu-cli -- task import-ical - < calendar.ics
```

## 注意点

同期処理はその場で実行されます。
失敗した場合は、次回の同期で再試行されます。
「Google Calendar」側でイベントを削除しても、takusu 側には反映されません。
`sync delete-all` を使うと、「Google Calendar」側の takusu 同期イベントをすべて削除できます。

詳細は [Google Calendar 設定](../setup/google-calendar.md) を参照してください。
