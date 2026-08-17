-- #1470: Google Calendar 同期用の共通イベント設定（色・公開範囲・予定/空き状態）を追加する。
ALTER TABLE google_cal_settings ADD COLUMN color_id INTEGER;
ALTER TABLE google_cal_settings ADD COLUMN visibility TEXT;
ALTER TABLE google_cal_settings ADD COLUMN transparency TEXT;
