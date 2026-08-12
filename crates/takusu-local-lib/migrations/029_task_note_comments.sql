-- WI-2: migrate `task_note` memories to task comments, then drop the kind.
--
-- Mapping (source -> author): agent_inferred -> agent, user_confirmed -> user,
-- imported -> system. Rows whose subject task no longer exists are dropped.
-- Migrated comments preserve the memory's created_at and get consecutive seq
-- values per task. An idempotency-safe prefix prevents duplicate rows if this
-- file is ever re-applied over already-migrated data.

-- 1. Backfill comments from task_note memories (only when the task still exists).
--    seq is offset past any pre-existing comments on the task so the migrated
--    rows cannot collide with the unique (task_id, seq) index (#1456 review).
INSERT INTO task_comments (id, task_id, author, content, seq, created_at)
SELECT 'migrated-' || m.id,
       m.subject_id,
       CASE m.source
           WHEN 'agent_inferred' THEN 'agent'
           WHEN 'user_confirmed' THEN 'user'
           WHEN 'imported' THEN 'system'
       END,
       m.content,
       ROW_NUMBER() OVER (PARTITION BY m.subject_id ORDER BY m.created_at, m.id)
           + COALESCE((SELECT MAX(tc.seq) FROM task_comments tc WHERE tc.task_id = m.subject_id), 0),
       m.created_at
FROM memories m
WHERE m.kind = 'task_note'
  AND m.subject_type = 'task'
  AND EXISTS (SELECT 1 FROM tasks t WHERE t.id = m.subject_id)
  AND NOT EXISTS (SELECT 1 FROM task_comments tc WHERE tc.id = 'migrated-' || m.id);

-- 2. Remove the now-migrated task_note rows.
DELETE FROM memories WHERE kind = 'task_note';

-- 3. Rebuild `memories` to drop 'task_note' from the kind CHECK constraint.
--    Nothing references `memories`, so the rename is safe without touching
--    foreign_key pragmas.
ALTER TABLE memories RENAME TO memories_old;

CREATE TABLE memories (
    id                 TEXT PRIMARY KEY,
    kind               TEXT NOT NULL CHECK(kind IN ('proper_noun', 'fact')),
    key                TEXT NOT NULL,
    normalized_key     TEXT NOT NULL,
    content            TEXT NOT NULL,
    normalized_content TEXT NOT NULL,
    subject_type       TEXT NOT NULL DEFAULT '',
    subject_id         TEXT NOT NULL DEFAULT '',
    source             TEXT NOT NULL CHECK(source IN ('user_confirmed', 'agent_inferred', 'imported')),
    revision           INTEGER NOT NULL DEFAULT 1 CHECK(revision >= 1),
    created_at         TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at         TEXT NOT NULL DEFAULT (datetime('now')),
    last_used_at       TEXT
);

INSERT INTO memories (id, kind, key, normalized_key, content, normalized_content, subject_type, subject_id, source, revision, created_at, updated_at, last_used_at)
SELECT id, kind, key, normalized_key, content, normalized_content, subject_type, subject_id, source, revision, created_at, updated_at, last_used_at
FROM memories_old;

DROP TABLE memories_old;

CREATE UNIQUE INDEX IF NOT EXISTS uq_memories_logical_key
    ON memories(kind, normalized_key, subject_type, subject_id);
CREATE INDEX IF NOT EXISTS idx_memories_normalized_key
    ON memories(normalized_key);
CREATE INDEX IF NOT EXISTS idx_memories_subject
    ON memories(subject_type, subject_id);
CREATE INDEX IF NOT EXISTS idx_memories_kind_updated
    ON memories(kind, updated_at DESC);