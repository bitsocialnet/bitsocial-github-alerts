use html_escape::encode_text;
use std::collections::HashMap;
use std::env;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use teloxide::prelude::*;
use teloxide::types::ParseMode;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Severity {
    Critical,
    Error,
    Warning,
    Info,
}

impl Severity {
    pub fn emoji(&self) -> &'static str {
        match self {
            Severity::Critical => "🚨",
            Severity::Error => "❌",
            Severity::Warning => "⚠️",
            Severity::Info => "ℹ️",
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Severity::Critical => "CRITICAL",
            Severity::Error => "ERROR",
            Severity::Warning => "WARNING",
            Severity::Info => "INFO",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AlertKey {
    pub severity: Severity,
    pub category: String,
}

pub struct AlertManager {
    rate_limit: Duration,
    last_alerts: Mutex<HashMap<AlertKey, Instant>>,
}

impl Default for AlertManager {
    fn default() -> Self {
        Self::new()
    }
}

impl AlertManager {
    pub fn new() -> Self {
        Self {
            rate_limit: Duration::from_secs(60),
            last_alerts: Mutex::new(HashMap::new()),
        }
    }

    pub fn with_rate_limit(rate_limit: Duration) -> Self {
        Self {
            rate_limit,
            last_alerts: Mutex::new(HashMap::new()),
        }
    }

    pub fn should_alert(&self, severity: Severity, category: &str) -> bool {
        let key = AlertKey {
            severity,
            category: category.to_string(),
        };

        let mut last_alerts = match self.last_alerts.lock() {
            Ok(guard) => guard,
            Err(poisoned) => {
                tracing::warn!("AlertManager mutex poisoned, recovering");
                poisoned.into_inner()
            }
        };
        let now = Instant::now();

        // Drop expired keys so per-chat keys do not accumulate.
        last_alerts.retain(|_, last_time| now.duration_since(*last_time) < self.rate_limit);

        if let Some(last_time) = last_alerts.get(&key) {
            if now.duration_since(*last_time) < self.rate_limit {
                return false;
            }
        }

        last_alerts.insert(key, now);
        true
    }

    fn chat_throttle_key(category: &str, chat_id: i64) -> String {
        format!("{}:{}", category, chat_id)
    }

    pub fn format_alert(&self, severity: Severity, category: &str, message: &str) -> String {
        format!(
            "{} <b>[{}]</b> {}\n{}",
            severity.emoji(),
            severity.label(),
            category,
            encode_text(message)
        )
    }

    pub fn clear_expired(&self) {
        let mut last_alerts = match self.last_alerts.lock() {
            Ok(guard) => guard,
            Err(poisoned) => {
                tracing::warn!("AlertManager mutex poisoned, recovering");
                poisoned.into_inner()
            }
        };
        let now = Instant::now();

        last_alerts.retain(|_, last_time| now.duration_since(*last_time) < self.rate_limit);
    }

    pub async fn send_alert(&self, bot: &Bot, severity: Severity, category: &str, message: &str) {
        self.send_throttled(bot, severity, category, category, message)
            .await;
    }

    /// Like `send_alert`, but throttled per chat. Use it for one-time changes
    /// of a chat's state (deactivation, migration), where a notice about one
    /// chat must not suppress the notice about another.
    pub async fn send_chat_alert(
        &self,
        bot: &Bot,
        severity: Severity,
        category: &str,
        chat_id: i64,
        message: &str,
    ) {
        let throttle_key = Self::chat_throttle_key(category, chat_id);
        self.send_throttled(bot, severity, &throttle_key, category, message)
            .await;
    }

    async fn send_throttled(
        &self,
        bot: &Bot,
        severity: Severity,
        throttle_key: &str,
        category: &str,
        message: &str,
    ) {
        if env::var("ADMIN_LOGS").unwrap_or_default() != "ACTIVE" {
            return;
        }

        if !self.should_alert(severity, throttle_key) {
            tracing::debug!("Alert rate-limited: {} - {}", category, message);
            return;
        }

        let admin_chat_id: i64 = match env::var("TELEGRAM_ADMIN_CHAT_ID")
            .ok()
            .and_then(|s| s.parse().ok())
        {
            Some(id) => id,
            None => {
                tracing::error!("TELEGRAM_ADMIN_CHAT_ID not set or invalid");
                return;
            }
        };

        let formatted = self.format_alert(severity, category, message);

        if let Err(e) = bot
            .send_message(ChatId(admin_chat_id), &formatted)
            .parse_mode(ParseMode::Html)
            .await
        {
            tracing::error!("Failed to send alert: {}", e);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread::sleep;

    #[test]
    fn test_severity_emoji() {
        assert_eq!(Severity::Critical.emoji(), "🚨");
        assert_eq!(Severity::Error.emoji(), "❌");
        assert_eq!(Severity::Warning.emoji(), "⚠️");
        assert_eq!(Severity::Info.emoji(), "ℹ️");
    }

    #[test]
    fn test_severity_label() {
        assert_eq!(Severity::Critical.label(), "CRITICAL");
        assert_eq!(Severity::Error.label(), "ERROR");
        assert_eq!(Severity::Warning.label(), "WARNING");
        assert_eq!(Severity::Info.label(), "INFO");
    }

    #[test]
    fn test_format_alert() {
        let manager = AlertManager::new();
        let formatted = manager.format_alert(Severity::Error, "Database", "Connection failed");

        assert!(formatted.contains("❌"));
        assert!(formatted.contains("[ERROR]"));
        assert!(formatted.contains("Database"));
        assert!(formatted.contains("Connection failed"));
    }

    #[test]
    fn test_rate_limiting_allows_first_alert() {
        let manager = AlertManager::new();
        assert!(manager.should_alert(Severity::Error, "test"));
    }

    #[test]
    fn test_rate_limiting_blocks_duplicate() {
        let manager = AlertManager::with_rate_limit(Duration::from_millis(100));

        assert!(manager.should_alert(Severity::Error, "test"));
        assert!(!manager.should_alert(Severity::Error, "test"));
    }

    #[test]
    fn test_rate_limiting_allows_after_expiry() {
        let manager = AlertManager::with_rate_limit(Duration::from_millis(50));

        assert!(manager.should_alert(Severity::Error, "test"));
        sleep(Duration::from_millis(60));
        assert!(manager.should_alert(Severity::Error, "test"));
    }

    #[test]
    fn test_different_categories_not_rate_limited() {
        let manager = AlertManager::new();

        assert!(manager.should_alert(Severity::Error, "database"));
        assert!(manager.should_alert(Severity::Error, "webhook"));
    }

    #[test]
    fn test_different_severities_not_rate_limited() {
        let manager = AlertManager::new();

        assert!(manager.should_alert(Severity::Error, "test"));
        assert!(manager.should_alert(Severity::Warning, "test"));
    }

    #[test]
    fn test_chat_alerts_are_throttled_per_chat() {
        let manager = AlertManager::new();
        let key = |chat_id| AlertManager::chat_throttle_key("Bot-Deactivated", chat_id);

        // Two chats deactivated within the window: both notices must go out.
        assert!(manager.should_alert(Severity::Warning, &key(-1001)));
        assert!(manager.should_alert(Severity::Warning, &key(-1002)));
        // A repeat for the same chat is still throttled.
        assert!(!manager.should_alert(Severity::Warning, &key(-1001)));
    }

    #[test]
    fn test_expired_keys_are_dropped() {
        let manager = AlertManager::with_rate_limit(Duration::from_millis(50));
        let key = |chat_id| AlertManager::chat_throttle_key("Bot-Deactivated", chat_id);

        assert!(manager.should_alert(Severity::Warning, &key(-1001)));
        sleep(Duration::from_millis(60));
        assert!(manager.should_alert(Severity::Warning, &key(-1002)));

        let last_alerts = manager.last_alerts.lock().unwrap();
        assert_eq!(last_alerts.len(), 1);
    }

    #[test]
    fn test_clear_expired() {
        let manager = AlertManager::with_rate_limit(Duration::from_millis(50));

        manager.should_alert(Severity::Error, "test1");
        manager.should_alert(Severity::Warning, "test2");

        sleep(Duration::from_millis(60));
        manager.clear_expired();

        let last_alerts = manager.last_alerts.lock().unwrap();
        assert!(last_alerts.is_empty());
    }
}
