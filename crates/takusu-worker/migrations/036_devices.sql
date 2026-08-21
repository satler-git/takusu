-- Multi-device arbitration for resident authority (WI-11).
-- Settings-side column only; the device table lives in 037_devices.sql so the
-- two migrations are guarded individually by the D1 migration table.

ALTER TABLE settings ADD COLUMN device_priority TEXT NOT NULL DEFAULT '["desktop","android"]';
