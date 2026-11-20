use crate::observability::alerts::Severity;
use crate::observability::{ALERTS, METRICS};
use crate::utils::telegram_admin::send_message_to_admin;
use bitsocial_github_alerts::db::DbPool;
use bitsocial_github_alerts::{
    deactivate_chat, get_webhook_url_or_create, TopicChange, WebhookGetOrCreateInput,
};
use html_escape::encode_text;
use teloxide::dispatching::{Dispatcher, UpdateFilterExt, UpdateHandler};
use teloxide::dptree::case;
use teloxide::macros::BotCommands;
use teloxide::payloads::SendMessageSetters;
use teloxide::prelude::LoggingErrorHandler;
use teloxide::prelude::{ChatId, ChatMemberUpdated, Message, Requester, ResponseResult, Update};
use teloxide::types::{
    Chat, ChatKind, ChatMemberKind, ChatPublic, MessageKind, ParseMode, PublicChatKind,
    PublicChatSupergroup,
};
use teloxide::{dptree, filter_command, Bot, RequestError};

const ISSUES_URL: &str = "https://github.com/bitsocialnet/bitsocial-github-alerts/issues/new";

#[derive(Debug, Clone)]
pub struct BotConfig {
    pub bot_name: String,
    pub token: String,
    pub webhook_base_url: String,
    pub admin_chat_id: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct BotService {
    pub bot: Bot,
    config: BotConfig,
    pool: DbPool,
}

pub struct StartCommand {
    pub chat_id: i64,
    pub topic: TopicChange,
    pub inviter_username: Option<String>,
    pub chat_title: Option<String>,
}

pub struct TelegramMessage {
    pub chat_id: i64,
    pub thread_id: Option<i32>,
    pub message: String,
    /// Whether Telegram should render a link-preview card for links in the
    /// message. Only release notifications enable this.
    pub link_preview: bool,
}

#[derive(BotCommands, Clone)]
#[command(
    rename_rule = "lowercase",
    description = "These commands are supported:"
)]
enum Command {
    #[command(description = "starts!")]
    Start,
}

/// Where a /start asks alerts to go. Only messages Telegram marks as topic
/// messages name a topic: message_thread_id is also set on replies outside
/// topics, where it is not a valid send destination. In a chat without topics
/// a stored topic is cleared (topics were turned off, or an invalid one was
/// stored). In a forum's General topic the stored topic is kept, because that
/// is where members send /start to see the URL again.
fn start_topic(msg: &Message) -> TopicChange {
    let in_topic = matches!(&msg.kind, MessageKind::Common(common) if common.is_topic_message);
    match msg.thread_id {
        Some(thread_id) if in_topic => TopicChange::Set(thread_id),
        _ if is_forum(&msg.chat) => TopicChange::Keep,
        _ => TopicChange::Clear,
    }
}

fn is_forum(chat: &Chat) -> bool {
    matches!(
        &chat.kind,
        ChatKind::Public(ChatPublic {
            kind: PublicChatKind::Supergroup(PublicChatSupergroup { is_forum: true, .. }),
            ..
        })
    )
}

impl BotService {
    pub fn new(config: BotConfig, pool: DbPool) -> Self {
        BotService {
            bot: Bot::new(&config.token),
            config,
            pool,
        }
    }

    async fn handle_start_command(&self, msg: Message) -> ResponseResult<()> {
        tracing::info!("Start command received");
        let inviter_username = match msg.from() {
            Some(user) => user.username.clone(),
            None => None,
        };

        let topic = start_topic(&msg);
        let chat_title = msg.chat.title().map(|t| t.to_string());

        self.handle_new_chat_and_start_command(StartCommand {
            chat_id: msg.chat.id.0,
            topic,
            inviter_username,
            chat_title,
        })
        .await?;

        Ok(())
    }

    async fn handle_new_chat_and_start_command(
        &self,
        start_command: StartCommand,
    ) -> ResponseResult<()> {
        let StartCommand {
            chat_id,
            topic,
            inviter_username,
            chat_title: _,
        } = start_command;
        let bot_name = &self.config.bot_name;

        // Replies go to the topic the command was sent in.
        let thread_id = match topic {
            TopicChange::Set(thread_id) => Some(thread_id),
            TopicChange::Keep | TopicChange::Clear => None,
        };

        let webhook_info = match get_webhook_url_or_create(
            &self.pool,
            WebhookGetOrCreateInput {
                telegram_chat_id: chat_id.to_string().as_str(),
                topic,
            },
        ) {
            Ok(info) => info,
            Err(e) => {
                tracing::error!("Database error creating webhook: {:?}", e);
                METRICS.increment_errors();
                ALERTS
                    .send_alert(
                        &self.bot,
                        Severity::Error,
                        "Database",
                        &format!("Failed to create webhook for chat {}: {}", chat_id, e),
                    )
                    .await;
                self.send_telegram_message(TelegramMessage {
                    chat_id,
                    thread_id,
                    message: format!(
                        "Hi there! Our bot is currently having some problems. \
                         Please create a Github issue here: {ISSUES_URL}"
                    ),
                    link_preview: false,
                })
                .await?;
                return Ok(());
            }
        };

        let message = if webhook_info.webhook_url.is_empty() {
            tracing::error!("Error creating or getting webhook: {:?}", webhook_info);
            format!(
                "Hi there! \
                 Our bot currently has some problems. \
                 Please create a Github issue here: {ISSUES_URL}"
            )
        } else {
            format!(
                "Organization webhooks automatically cover current and future public repositories \
                 in bitsocialnet, bitsocialforge, and pkcprotocol. Private repositories are excluded.\n\n\
                 New repositories need no webhook setup. Alerts go to the chat selected by each \
                 organization's webhook.\n\n\
                 To connect or change the destination, ask an administrator to configure the \
                 organization webhook with this chat's URL:\n{}/{}/{}\n\n\
                 Administrators must set the signing secret privately and keep SSL verification enabled. \
                 Do not share the secret in this chat or add duplicate repository webhooks.",
                self.config.webhook_base_url,
                bot_name.to_lowercase(),
                webhook_info.webhook_url
            )
        };

        self.send_telegram_message(TelegramMessage {
            chat_id,
            thread_id,
            message,
            link_preview: false,
        })
        .await?;

        if webhook_info.is_new {
            METRICS.increment_new_chat();
            let inviter_username_str = inviter_username.unwrap_or_else(|| "unknown".to_string());

            send_message_to_admin(
                &self.bot,
                format!(
                    "New {bot_name} webhook added: {chat_id} by @{}",
                    encode_text(&inviter_username_str)
                ),
                10,
            )
            .await?;
        }

        Ok(())
    }

    async fn handle_my_chat_member_update(&self, update: ChatMemberUpdated) -> ResponseResult<()> {
        let chat_id = update.chat.id.0;
        let bot_name = &self.config.bot_name;
        let chat_title = update.chat.title().map(|t| t.to_string());

        tracing::info!(
            "Received chat member update from {}: {:#?} {:#?}",
            chat_id,
            update.old_chat_member,
            update.new_chat_member
        );

        let action = membership_action(
            update.chat.is_channel(),
            &update.old_chat_member.kind,
            &update.new_chat_member.kind,
        );

        if action == MembershipAction::Register {
            self.handle_new_chat_and_start_command(StartCommand {
                chat_id,
                topic: TopicChange::Keep,
                inviter_username: update.from.username,
                chat_title,
            })
            .await?
        } else if action == MembershipAction::Deactivate {
            tracing::info!("Bot removed from chat {}", chat_id);
            METRICS.increment_churn();

            match deactivate_chat(&self.pool, &chat_id.to_string()) {
                Ok(_) => {
                    send_message_to_admin(
                        &self.bot,
                        format!("{bot_name} bot removed from chat {chat_id} - chat deactivated"),
                        10,
                    )
                    .await?;
                }
                Err(e) => {
                    tracing::error!("Failed to deactivate chat {}: {:?}", chat_id, e);
                    METRICS.increment_errors();
                    ALERTS
                        .send_alert(
                            &self.bot,
                            Severity::Warning,
                            "Database",
                            &format!("Failed to deactivate chat {}: {}", chat_id, e),
                        )
                        .await;
                }
            }
        }

        Ok(())
    }

    pub async fn send_telegram_message(&self, message: TelegramMessage) -> ResponseResult<()> {
        let TelegramMessage {
            chat_id,
            thread_id,
            message,
            link_preview,
        } = message;

        tracing::info!("Sending message to {}: {}", chat_id, message);
        let bot = &self.bot;
        let chat_id = ChatId(chat_id);

        let mut request = bot
            .send_message(chat_id, &message)
            .parse_mode(ParseMode::Html)
            .disable_web_page_preview(!link_preview);

        if let Some(tid) = thread_id {
            request = request.message_thread_id(tid);
        }

        request.await?;

        Ok(())
    }

    // Message and my_chat_member updates are siblings: a branch nested under
    // filter_message() only ever sees message updates.
    fn update_handler() -> UpdateHandler<RequestError> {
        dptree::entry()
            .branch(
                Update::filter_message().branch(filter_command::<Command, _>().branch(
                    case![Command::Start].endpoint(
                        move |msg: Message, bot: BotService| async move {
                            bot.handle_start_command(msg).await
                        },
                    ),
                )),
            )
            .branch(Update::filter_my_chat_member().endpoint(
                move |upd: ChatMemberUpdated, bot: BotService| async move {
                    bot.handle_my_chat_member_update(upd).await
                },
            ))
    }

    pub async fn run_bot(self) {
        Dispatcher::builder(self.bot.clone(), Self::update_handler())
            .dependencies(dptree::deps![self])
            .default_handler(|_| async {})
            .error_handler(LoggingErrorHandler::with_custom_text(
                "An error has occurred in the dispatcher",
            ))
            .enable_ctrlc_handler()
            .build()
            .dispatch()
            .await;
    }
}

/// What a change of the bot's own membership means for the chat's webhook.
#[derive(Debug, PartialEq, Eq)]
enum MembershipAction {
    /// The bot was added or re-added: register or reactivate the chat.
    Register,
    /// The bot left, was removed or was banned: deactivate the chat.
    Deactivate,
    /// The bot stayed in the chat (or out of it), e.g. it was promoted.
    Ignore,
}

/// Compares whether the bot is in the chat before and after, not the literal
/// statuses: it can be re-added as an administrator, come back after a ban, or
/// be restricted while staying in the chat. Channels are ignored: they cannot
/// run /start, so the bot never registers them, and adding it as a channel
/// administrator must not post the setup message there.
fn membership_action(
    is_channel: bool,
    old: &ChatMemberKind,
    new: &ChatMemberKind,
) -> MembershipAction {
    if is_channel {
        return MembershipAction::Ignore;
    }
    match (old.is_present(), new.is_present()) {
        (false, true) => MembershipAction::Register,
        (true, false) => MembershipAction::Deactivate,
        _ => MembershipAction::Ignore,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use diesel::{r2d2::ConnectionManager, PgConnection};
    use serde_json::json;
    use std::ops::ControlFlow;
    use std::sync::atomic::Ordering;
    use std::sync::Arc;
    use std::time::Duration;
    use teloxide::types::{Administrator, Banned, Restricted, UntilDate, UpdateKind};

    fn administrator() -> ChatMemberKind {
        ChatMemberKind::Administrator(Administrator {
            custom_title: None,
            is_anonymous: false,
            can_be_edited: false,
            can_manage_chat: true,
            can_change_info: false,
            can_post_messages: false,
            can_edit_messages: false,
            can_delete_messages: false,
            can_manage_video_chats: false,
            can_invite_users: false,
            can_restrict_members: false,
            can_pin_messages: false,
            can_manage_topics: false,
            can_promote_members: false,
        })
    }

    fn banned() -> ChatMemberKind {
        ChatMemberKind::Banned(Banned {
            until_date: UntilDate::Forever,
        })
    }

    fn restricted(is_member: bool) -> ChatMemberKind {
        ChatMemberKind::Restricted(Restricted {
            until_date: UntilDate::Forever,
            is_member,
            can_send_messages: true,
            can_send_media_messages: false,
            can_send_other_messages: false,
            can_add_web_page_previews: false,
            can_change_info: false,
            can_invite_users: false,
            can_pin_messages: false,
            can_manage_topics: false,
            can_send_polls: false,
        })
    }

    #[test]
    fn readding_the_bot_registers_the_chat() {
        // Whatever role the bot comes back with, as the 404 hint promises.
        let joins = [
            (ChatMemberKind::Left, ChatMemberKind::Member),
            (ChatMemberKind::Left, administrator()),
            (banned(), ChatMemberKind::Member),
            (banned(), administrator()),
            (restricted(false), ChatMemberKind::Member),
        ];
        for (old, new) in joins {
            assert_eq!(
                membership_action(false, &old, &new),
                MembershipAction::Register,
                "{old:?} -> {new:?}"
            );
        }
    }

    #[test]
    fn removing_the_bot_deactivates_the_chat() {
        let removals = [
            (ChatMemberKind::Member, ChatMemberKind::Left),
            (ChatMemberKind::Member, banned()),
            (administrator(), ChatMemberKind::Left),
            (administrator(), banned()),
            (administrator(), restricted(false)),
        ];
        for (old, new) in removals {
            assert_eq!(
                membership_action(false, &old, &new),
                MembershipAction::Deactivate,
                "{old:?} -> {new:?}"
            );
        }
    }

    #[test]
    fn role_changes_inside_the_chat_change_nothing() {
        let changes = [
            (ChatMemberKind::Member, administrator()),
            (administrator(), ChatMemberKind::Member),
            (ChatMemberKind::Member, restricted(true)),
            (restricted(true), ChatMemberKind::Member),
        ];
        for (old, new) in changes {
            assert_eq!(
                membership_action(false, &old, &new),
                MembershipAction::Ignore,
                "{old:?} -> {new:?}"
            );
        }
    }

    #[test]
    fn channel_membership_changes_are_ignored() {
        // Channels cannot run /start, so the bot never registers them, and
        // adding it as a channel administrator must not post the setup message.
        assert_eq!(
            membership_action(true, &ChatMemberKind::Left, &administrator()),
            MembershipAction::Ignore
        );
        assert_eq!(
            membership_action(true, &administrator(), &ChatMemberKind::Left),
            MembershipAction::Ignore
        );
    }

    #[tokio::test]
    async fn bot_removal_reaches_my_chat_member_handler() {
        // No database is reachable, so deactivation fails fast and only the
        // routing and the removal branch are exercised.
        let pool = diesel::r2d2::Pool::builder()
            .min_idle(Some(0))
            .connection_timeout(Duration::from_millis(100))
            .build_unchecked(ConnectionManager::<PgConnection>::new(
                "postgresql://localhost:1/unused",
            ));
        let service = BotService::new(
            BotConfig {
                bot_name: "Github".to_string(),
                token: "0:test".to_string(),
                webhook_base_url: String::new(),
                admin_chat_id: None,
            },
            Arc::new(pool),
        );
        let bot_user =
            json!({"id": 1, "is_bot": true, "first_name": "Alerts", "username": "alerts_bot"});
        // teloxide parses update kinds from borrowed keys, so parse from a string.
        let update: Update = serde_json::from_str(
            &json!({
                "update_id": 1,
                "my_chat_member": {
                    "chat": {"id": -1001234567890_i64, "type": "supergroup", "title": "Alerts"},
                    "from": {"id": 2, "is_bot": false, "first_name": "Admin"},
                    "date": 1700000000,
                    "old_chat_member": {"user": bot_user, "status": "member"},
                    "new_chat_member": {"user": bot_user, "status": "left"}
                }
            })
            .to_string(),
        )
        .unwrap();
        assert!(matches!(update.kind, UpdateKind::MyChatMember(_)));

        let churn_before = METRICS.churned_chats.load(Ordering::Relaxed);
        let result = BotService::update_handler()
            .dispatch(dptree::deps![update, service])
            .await;

        assert!(
            matches!(result, ControlFlow::Break(Ok(()))),
            "my_chat_member update was not handled"
        );
        assert!(METRICS.churned_chats.load(Ordering::Relaxed) > churn_before);
    }

    fn start_message(extra: serde_json::Value) -> Message {
        let mut message = json!({
            "message_id": 20,
            "date": 1700000000,
            "chat": {"id": -1001234567890_i64, "type": "supergroup", "title": "Alerts"},
            "from": {"id": 2, "is_bot": false, "first_name": "Admin"},
            "text": "/start"
        });
        message
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        serde_json::from_str(&message.to_string()).unwrap()
    }

    fn forum_chat() -> serde_json::Value {
        json!({"id": -1001234567890_i64, "type": "supergroup", "title": "Alerts", "is_forum": true})
    }

    #[test]
    fn start_inside_a_forum_topic_uses_that_topic() {
        let msg = start_message(json!({
            "chat": forum_chat(),
            "message_thread_id": 4,
            "is_topic_message": true
        }));
        assert_eq!(start_topic(&msg), TopicChange::Set(4));
    }

    #[test]
    fn start_as_a_reply_outside_a_topic_uses_no_topic() {
        // Telegram also sets message_thread_id on replies outside forum topics,
        // where it is a message id that sendMessage rejects as a topic.
        let msg = start_message(json!({"message_thread_id": 10}));
        assert_eq!(start_topic(&msg), TopicChange::Clear);
    }

    #[test]
    fn start_in_a_chat_without_topics_clears_the_stored_topic() {
        // Topics were turned off, or an invalid topic was stored before:
        // alerts must go to the chat itself again.
        let chats = [
            json!({"id": -1001234567890_i64, "type": "supergroup", "title": "Alerts"}),
            json!({"id": -1234567890_i64, "type": "group", "title": "Alerts"}),
            json!({"id": 2, "type": "private", "first_name": "Admin"}),
        ];
        for chat in chats {
            let msg = start_message(json!({ "chat": chat }));
            assert_eq!(start_topic(&msg), TopicChange::Clear, "{chat}");
        }
    }

    #[test]
    fn start_in_the_general_topic_keeps_the_stored_topic() {
        // Members use /start in General to see the URL again; that must not
        // pull alerts out of the topic they were sent to.
        let general = start_message(json!({ "chat": forum_chat() }));
        let reply_in_general =
            start_message(json!({"chat": forum_chat(), "message_thread_id": 10}));
        assert_eq!(start_topic(&general), TopicChange::Keep);
        assert_eq!(start_topic(&reply_in_general), TopicChange::Keep);
    }
}
