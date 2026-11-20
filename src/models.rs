use crate::schema::{chats, webhooks};
use chrono::{DateTime, Utc};
use diesel::pg::data_types::PgTimestamp;
use diesel::prelude::*;

#[derive(Queryable, Identifiable, Selectable)]
#[diesel(table_name = crate::schema::chats)]
#[diesel(check_for_backend(diesel::pg::Pg))]
pub struct Chat {
    pub id: i32,
    pub name: String,
    pub telegram_id: String,
    pub webhook_url: Option<String>,
    pub thread_id: Option<String>,
    pub language: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub is_active: bool,
    pub deactivated_at: Option<DateTime<Utc>>,
    pub last_digest_at: Option<DateTime<Utc>>,
}

#[derive(Insertable)]
#[diesel(table_name = chats)]
#[diesel(check_for_backend(diesel::pg::Pg))]
pub struct NewChat<'a> {
    pub name: &'a str,
    pub telegram_id: &'a str,
    pub webhook_url: Option<&'a str>,
    pub thread_id: Option<&'a str>,
    pub language: &'a str,
}

#[derive(Queryable, Associations, Identifiable)]
#[diesel(belongs_to(Chat))]
#[diesel(table_name = webhooks)]
#[diesel(check_for_backend(diesel::pg::Pg))]
pub struct Webhook {
    pub id: i32,
    pub name: String,
    pub webhook_url: String,
    pub created_at: PgTimestamp,
    pub updated_at: PgTimestamp,
    pub chat_id: Option<i32>,
}

#[derive(Insertable)]
#[diesel(table_name = webhooks)]
#[diesel(check_for_backend(diesel::pg::Pg))]
pub struct NewWebhook<'a> {
    pub name: &'a str,
    pub webhook_url: &'a str,
    pub chat_id: Option<i32>,
}

/// Kinds of activity a digest counts. Stored as text in `digest_events.kind`.
pub mod digest_kind {
    pub const COMMIT: &str = "commit";
    pub const RELEASE: &str = "release";
    pub const ISSUE_OPENED: &str = "issue_opened";
    pub const ISSUE_CLOSED: &str = "issue_closed";
    pub const PR_MERGED: &str = "pr_merged";
}

#[derive(Queryable, Selectable, Debug, Clone, PartialEq)]
#[diesel(table_name = crate::schema::digest_events)]
#[diesel(check_for_backend(diesel::pg::Pg))]
pub struct DigestEvent {
    pub id: i64,
    pub chat_id: i32,
    pub kind: String,
    /// Full repository name, e.g. `bitsocialnet/5chan`.
    pub repo: String,
    /// Commit SHA, release tag, or issue or pull request number.
    pub item: String,
    /// GitHub login credited for the activity, if it is a person.
    pub actor: Option<String>,
    pub url: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Insertable, Debug, Clone, PartialEq)]
#[diesel(table_name = crate::schema::digest_events)]
#[diesel(check_for_backend(diesel::pg::Pg))]
pub struct NewDigestEvent {
    pub chat_id: i32,
    pub kind: String,
    pub repo: String,
    pub item: String,
    pub actor: Option<String>,
    pub url: Option<String>,
}
