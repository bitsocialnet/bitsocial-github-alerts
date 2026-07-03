use crate::observability::alerts::Severity;
use crate::observability::{ALERTS, METRICS};
use crate::utils::telegram_admin::send_message_to_admin;
use bitsocial_github_alerts::db::DbPool;
use bitsocial_github_alerts::{
    deactivate_chat, get_webhook_url_or_create, WebhookGetOrCreateInput,
};
use html_escape::encode_text;
use teloxide::dispatching::{Dispatcher, UpdateFilterExt};
use teloxide::dptree::case;
use teloxide::macros::BotCommands;
use teloxide::payloads::SendMessageSetters;
use teloxide::prelude::LoggingErrorHandler;
use teloxide::prelude::{ChatId, ChatMemberUpdated, Message, Requester, ResponseResult, Update};
use teloxide::types::{ChatMemberKind, ParseMode};
use teloxide::{dptree, filter_command, Bot};

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
    pub thread_id: Option<i32>,
    pub inviter_username: Option<String>,
    pub chat_title: Option<String>,
}

pub struct TelegramMessage {
    pub chat_id: i64,
    pub thread_id: Option<i32>,
    pub message: String,
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

        let thread_id = msg.thread_id;
        let chat_title = msg.chat.title().map(|t| t.to_string());

        self.handle_new_chat_and_start_command(StartCommand {
            chat_id: msg.chat.id.0,
            thread_id,
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
            thread_id,
            inviter_username,
            chat_title: _,
        } = start_command;
        let bot_name = &self.config.bot_name;

        let thread_id_str = thread_id.map(|tid| tid.to_string());
        let thread_id_ref = thread_id_str.as_deref();

        let webhook_info = match get_webhook_url_or_create(
            &self.pool,
            WebhookGetOrCreateInput {
                telegram_chat_id: chat_id.to_string().as_str(),
                telegram_thread_id: thread_id_ref,
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
                "Hi there! \
                 To setup notifications for \
                 this chat your {} project(repo), \
                 open Settings -> Webhooks and add this \
                 URL: {}/{}/{}",
                bot_name,
                self.config.webhook_base_url,
                bot_name.to_lowercase(),
                webhook_info.webhook_url
            )
        };

        self.send_telegram_message(TelegramMessage {
            chat_id,
            thread_id,
            message,
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

        let old_kind = &update.old_chat_member.kind;
        let new_kind = &update.new_chat_member.kind;

        if *old_kind == ChatMemberKind::Left && *new_kind == ChatMemberKind::Member {
            self.handle_new_chat_and_start_command(StartCommand {
                chat_id,
                thread_id: None,
                inviter_username: update.from.username,
                chat_title,
            })
            .await?
        } else if matches!(
            old_kind,
            ChatMemberKind::Member | ChatMemberKind::Administrator { .. }
        ) && matches!(
            new_kind,
            ChatMemberKind::Left | ChatMemberKind::Banned { .. }
        ) {
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
        } = message;

        tracing::info!("Sending message to {}: {}", chat_id, message);
        let bot = &self.bot;
        let chat_id = ChatId(chat_id);

        let mut request = bot
            .send_message(chat_id, &message)
            .parse_mode(ParseMode::Html);

        if let Some(tid) = thread_id {
            request = request.message_thread_id(tid);
        }

        request.await?;

        Ok(())
    }

    pub async fn run_bot(self) {
        let handler = Update::filter_message()
            .branch(filter_command::<Command, _>().branch(
                case![Command::Start].endpoint(move |msg: Message, bot: BotService| async move {
                    bot.handle_start_command(msg).await
                }),
            ))
            .branch(Update::filter_my_chat_member().endpoint(
                move |upd: ChatMemberUpdated, bot: BotService| async move {
                    bot.handle_my_chat_member_update(upd).await
                },
            ));

        Dispatcher::builder(self.bot.clone(), handler)
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
