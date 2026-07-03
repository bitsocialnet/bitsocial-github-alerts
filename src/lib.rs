use chrono::{DateTime, Utc};
use diesel::prelude::*;
use rand::distributions::Alphanumeric;
use rand::{thread_rng, Rng};

pub mod db;
pub mod models;
pub mod schema;

use self::models::*;
use db::{DbError, PgPool};

#[derive(Debug)]
pub struct WebhookInfo {
    pub webhook_url: String,
    pub is_new: bool,
}

pub fn create_webhook(
    pool: &PgPool,
    webhook_url: &str,
    name: &str,
    chat_id: i32,
) -> Result<Webhook, DbError> {
    use self::schema::webhooks;

    let conn = &mut pool.get()?;

    let new_webhook = NewWebhook {
        webhook_url,
        name,
        chat_id: Some(chat_id),
    };

    Ok(diesel::insert_into(webhooks::table)
        .values(&new_webhook)
        .get_result(conn)?)
}

pub struct WebhookGetOrCreateInput<'a> {
    pub telegram_chat_id: &'a str,
    pub telegram_thread_id: Option<&'a str>,
}

pub fn get_webhook_url_or_create(
    pool: &PgPool,
    input: WebhookGetOrCreateInput,
) -> Result<WebhookInfo, DbError> {
    let WebhookGetOrCreateInput {
        telegram_chat_id,
        telegram_thread_id,
    } = input;

    use self::schema::chats;

    let conn = &mut pool.get()?;

    let result: Option<Chat> = chats::dsl::chats
        .filter(chats::dsl::telegram_id.eq(telegram_chat_id.to_string()))
        .first::<Chat>(conn)
        .optional()?;

    if let Some(chat) = result {
        if !chat.is_active {
            reactivate_chat(pool, telegram_chat_id)?;
        }

        if let Some(thread_id) = telegram_thread_id {
            if let Some(ref c) = find_chat_by_id(pool, chat.id)? {
                if c.thread_id.is_none() {
                    update_chat_thread_id(pool, c, thread_id)?;
                }
            }
        }

        match find_webhook_by_chat_id(pool, chat.id)? {
            Some(webhook) => Ok(WebhookInfo {
                webhook_url: webhook.webhook_url,
                is_new: false,
            }),
            None => {
                let random_string = create_random_string();
                let new_webhook = create_webhook(pool, &random_string, "new_chat", chat.id)?;
                Ok(WebhookInfo {
                    webhook_url: new_webhook.webhook_url,
                    is_new: true,
                })
            }
        }
    } else {
        let random_string = create_random_string();
        let name = "new_chat";
        let new_chat = create_chat(
            pool,
            CreateChatInput {
                telegram_chat_id,
                name,
                webhook_url: Some(&random_string),
                telegram_thread_id,
                language: "en",
            },
        )?;
        let new_webhook = create_webhook(pool, &random_string, name, new_chat.id)?;

        Ok(WebhookInfo {
            webhook_url: new_webhook.webhook_url,
            is_new: true,
        })
    }
}

pub struct CreateChatInput<'a> {
    pub telegram_chat_id: &'a str,
    pub name: &'a str,
    pub webhook_url: Option<&'a str>,
    pub telegram_thread_id: Option<&'a str>,
    pub language: &'a str,
}

pub fn create_chat(pool: &PgPool, create_chat_input: CreateChatInput) -> Result<Chat, DbError> {
    let CreateChatInput {
        telegram_chat_id,
        name,
        webhook_url,
        telegram_thread_id,
        language,
    } = create_chat_input;

    use self::schema::chats::table;

    let conn = &mut pool.get()?;

    let new_chat = NewChat {
        telegram_id: telegram_chat_id,
        name,
        webhook_url,
        thread_id: telegram_thread_id,
        language,
    };

    Ok(diesel::insert_into(table)
        .values(&new_chat)
        .get_result(conn)?)
}

pub fn update_chat_thread_id(
    pool: &PgPool,
    chat: &Chat,
    telegram_thread_id: &str,
) -> Result<Chat, DbError> {
    use self::schema::chats::dsl::*;

    let conn = &mut pool.get()?;

    Ok(diesel::update(chat)
        .set(thread_id.eq(telegram_thread_id))
        .get_result::<Chat>(conn)?)
}

pub fn find_webhook_by_webhook_url(pool: &PgPool, url: &str) -> Result<Option<Webhook>, DbError> {
    use schema::webhooks::dsl::*;

    let conn = &mut pool.get()?;

    Ok(webhooks
        .filter(webhook_url.eq(url))
        .first::<Webhook>(conn)
        .optional()?)
}

pub fn find_chat_by_id(pool: &PgPool, chat_id: i32) -> Result<Option<Chat>, DbError> {
    use schema::chats::dsl::*;

    let conn = &mut pool.get()?;

    Ok(chats
        .filter(id.eq(chat_id))
        .first::<Chat>(conn)
        .optional()?)
}

pub fn find_chat_by_telegram_chat_id(
    pool: &PgPool,
    telegram_chat_id: &str,
) -> Result<Option<Chat>, DbError> {
    use schema::chats::dsl::*;

    let conn = &mut pool.get()?;

    Ok(chats
        .filter(telegram_id.eq(telegram_chat_id))
        .first::<Chat>(conn)
        .optional()?)
}

pub fn find_webhook_by_chat_id(pool: &PgPool, chat_id: i32) -> Result<Option<Webhook>, DbError> {
    use schema::webhooks;

    let conn = &mut pool.get()?;

    Ok(webhooks::dsl::webhooks
        .filter(webhooks::dsl::chat_id.eq(chat_id))
        .first::<Webhook>(conn)
        .optional()?)
}

fn create_random_string() -> String {
    thread_rng()
        .sample_iter(&Alphanumeric)
        .take(8)
        .map(char::from)
        .collect()
}

pub fn deactivate_chat(pool: &PgPool, telegram_chat_id: &str) -> Result<Option<Chat>, DbError> {
    use self::schema::chats::dsl::*;

    let conn = &mut pool.get()?;

    let result = diesel::update(chats.filter(telegram_id.eq(telegram_chat_id)))
        .set((is_active.eq(false), deactivated_at.eq(Some(Utc::now()))))
        .get_result::<Chat>(conn)
        .optional()?;

    Ok(result)
}

pub fn reactivate_chat(pool: &PgPool, telegram_chat_id: &str) -> Result<Option<Chat>, DbError> {
    use self::schema::chats::dsl::*;

    let conn = &mut pool.get()?;

    let result = diesel::update(chats.filter(telegram_id.eq(telegram_chat_id)))
        .set((is_active.eq(true), deactivated_at.eq(None::<DateTime<Utc>>)))
        .get_result::<Chat>(conn)
        .optional()?;

    Ok(result)
}

/// Update the stored Telegram chat id after a group -> supergroup migration.
/// Returns true when a chat row was updated.
pub fn migrate_chat_telegram_id(
    pool: &PgPool,
    old_chat_id: i64,
    new_chat_id: i64,
) -> Result<bool, DbError> {
    use self::schema::chats::dsl::*;

    let conn = &mut pool.get()?;

    let updated = diesel::update(chats.filter(telegram_id.eq(old_chat_id.to_string())))
        .set(telegram_id.eq(new_chat_id.to_string()))
        .execute(conn)?;

    Ok(updated > 0)
}
