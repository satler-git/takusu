-- #1470: Google Calendar 同期用の共通リマインダー時間（分）を設定できるようにする。
ALTER TABLE google_cal_settings ADD COLUMN reminder_minutes INTEGER;
