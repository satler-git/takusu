# カレンダー連携

takusu は外部カレンダーと双方向または単方向で連携できます。

## Google Calendar 連携

`google-cal` クレートと `takusu-local` の `/api/sync/*` エンドポイントで、Google Calendar との同期を行います。

### 同期の流れ

1. `takusu-local` 内の `active` スケジュールを取得する。
2. 既存の `google_cal_events` マッピングを取得する。
3. 削除、更新、作成を `BatchOp` に変換する。
4. Google Calendar Batch API に送信する（1 リクエスト最大 1000 件）。
5. `Content-ID` から対応する操作を特定する。
6. 更新失敗時は `delete` から `create` にフォールバックする。
7. マッピングを upsert と delete する。

### 認証

Google Calendar API へのアクセスには OAuth2 refresh_token が必要です。
CLI で取得したトークンを `takusu-local` の SQLite または `takusu-worker` の D1 に保存します。

詳細は [Google Calendar 設定](../setup/google-calendar.md) を参照してください。

## iCalendar（iCal）インポート

`takusu-ical` を使って iCalendar 形式のファイルをインポートできます。

対応は次の通りです。

- `BEGIN:VEVENT` と `END:VEVENT` に対応する。
- `DTSTART` と `DTEND`（UTC、オフセット、TZID、日付のみ）に対応する。
- `DURATION` に対応する。
- UID の重複をスキップする。
- `LINE FOLDING` に対応する。

```sh
# ファイルパスを指定
cargo run -p takusu-cli -- task import-ical calendar.ics

# 標準入力から読み込む
cargo run -p takusu-cli -- task import-ical - < calendar.ics
```

## 同期の注意点

- 同期はスケジュール操作のたびに **同期的** に実行されます（fire-and-forget ではありません）。
- Google Calendar 側で削除や変更したイベントは、takusu 側には反映されません。
- `takusu sync delete-all` を使うと、takusu から Google Calendar に同期したすべてのイベントを削除できます。
