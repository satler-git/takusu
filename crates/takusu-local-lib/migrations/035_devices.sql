-- Multi-device arbitration for resident authority (WI-11).
-- Settings-side column only; the device table lives in 036_devices.sql so
-- each step can be guarded independently.

ALTER TABLE settings ADD COLUMN device_priority TEXT NOT NULL DEFAULT '["desktop","android"]';
