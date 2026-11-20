-- Activity collected for each chat's periodic digest. One row per counted
-- item, so redelivered webhooks and commits pushed twice count once.
CREATE TABLE digest_events (
    id BIGSERIAL PRIMARY KEY,
    chat_id INTEGER NOT NULL REFERENCES chats (id) ON DELETE CASCADE,
    kind VARCHAR(32) NOT NULL,
    repo VARCHAR(255) NOT NULL,
    item VARCHAR(255) NOT NULL,
    actor VARCHAR(255),
    url TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (chat_id, kind, repo, item)
);

CREATE INDEX digest_events_chat_id_created_at_idx ON digest_events (chat_id, created_at);

-- End of the activity the chat's last digest covered. Existing chats start
-- now and new chats when they register, so a first digest covers only time
-- the bot was recording.
ALTER TABLE chats ADD COLUMN last_digest_at TIMESTAMPTZ NOT NULL DEFAULT NOW();
