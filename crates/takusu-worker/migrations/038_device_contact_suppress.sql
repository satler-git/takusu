-- Timed contact suppression per device (WI-17).
-- This is a follow-up to 037_devices.sql which did not include the column.

ALTER TABLE devices ADD COLUMN contact_suppress_until TEXT;
