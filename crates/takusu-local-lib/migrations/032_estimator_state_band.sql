-- Add progress-based band and next-crossing timestamp to estimator tables.
ALTER TABLE estimator_state ADD COLUMN band TEXT;
ALTER TABLE estimator_state ADD COLUMN next_crossing_time TEXT;
ALTER TABLE estimator_observations ADD COLUMN band TEXT;
ALTER TABLE estimator_observations ADD COLUMN next_crossing_time TEXT;
