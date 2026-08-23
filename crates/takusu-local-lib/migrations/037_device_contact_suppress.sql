-- Contact suppression state per device (WI-17).
-- Timed "ほっといて" window is scoped to the device that requested it;
-- the resident evaluator reads it when applying the contact policy.

ALTER TABLE devices ADD COLUMN contact_suppress_until TEXT;
