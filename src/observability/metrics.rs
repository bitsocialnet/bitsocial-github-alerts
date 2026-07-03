use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

pub struct Metrics {
    pub messages_sent: AtomicU64,
    pub webhooks_received: AtomicU64,
    pub github_webhooks: AtomicU64,
    pub github_messages_sent: AtomicU64,
    pub new_chats: AtomicU64,
    pub churned_chats: AtomicU64,
    pub errors: AtomicU64,
    pub start_time: Instant,
}

#[derive(Debug, Clone)]
pub struct MetricsSnapshot {
    pub messages_sent: u64,
    pub webhooks_received: u64,
    pub github_webhooks: u64,
    pub github_messages_sent: u64,
    pub new_chats: u64,
    pub churned_chats: u64,
    pub errors: u64,
    pub uptime_secs: u64,
}

impl Default for Metrics {
    fn default() -> Self {
        Self::new()
    }
}

impl Metrics {
    pub fn new() -> Self {
        Self {
            messages_sent: AtomicU64::new(0),
            webhooks_received: AtomicU64::new(0),
            github_webhooks: AtomicU64::new(0),
            github_messages_sent: AtomicU64::new(0),
            new_chats: AtomicU64::new(0),
            churned_chats: AtomicU64::new(0),
            errors: AtomicU64::new(0),
            start_time: Instant::now(),
        }
    }

    pub fn increment_messages_sent(&self) {
        self.messages_sent.fetch_add(1, Ordering::Relaxed);
    }

    pub fn increment_messages_sent_for_bot(&self, bot_name: &str) {
        self.messages_sent.fetch_add(1, Ordering::Relaxed);
        if bot_name.eq_ignore_ascii_case("github") {
            self.github_messages_sent.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub fn increment_webhooks(&self, source: &str) {
        self.webhooks_received.fetch_add(1, Ordering::Relaxed);
        if source.eq_ignore_ascii_case("github") {
            self.github_webhooks.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub fn increment_new_chat(&self) {
        self.new_chats.fetch_add(1, Ordering::Relaxed);
    }

    pub fn increment_churn(&self) {
        self.churned_chats.fetch_add(1, Ordering::Relaxed);
    }

    pub fn increment_errors(&self) {
        self.errors.fetch_add(1, Ordering::Relaxed);
    }

    pub fn snapshot(&self) -> MetricsSnapshot {
        MetricsSnapshot {
            messages_sent: self.messages_sent.load(Ordering::Relaxed),
            webhooks_received: self.webhooks_received.load(Ordering::Relaxed),
            github_webhooks: self.github_webhooks.load(Ordering::Relaxed),
            github_messages_sent: self.github_messages_sent.load(Ordering::Relaxed),
            new_chats: self.new_chats.load(Ordering::Relaxed),
            churned_chats: self.churned_chats.load(Ordering::Relaxed),
            errors: self.errors.load(Ordering::Relaxed),
            uptime_secs: self.start_time.elapsed().as_secs(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_increment_webhooks() {
        let metrics = Metrics::new();
        metrics.increment_webhooks("github");
        metrics.increment_webhooks("other");

        let snapshot = metrics.snapshot();
        assert_eq!(snapshot.webhooks_received, 2);
        assert_eq!(snapshot.github_webhooks, 1);
    }

    #[test]
    fn test_increment_messages_sent_for_bot() {
        let metrics = Metrics::new();
        metrics.increment_messages_sent_for_bot("Github");
        metrics.increment_messages_sent();

        let snapshot = metrics.snapshot();
        assert_eq!(snapshot.messages_sent, 2);
        assert_eq!(snapshot.github_messages_sent, 1);
    }

    #[test]
    fn test_new_chat_and_churn() {
        let metrics = Metrics::new();
        metrics.increment_new_chat();
        metrics.increment_churn();
        metrics.increment_errors();

        let snapshot = metrics.snapshot();
        assert_eq!(snapshot.new_chats, 1);
        assert_eq!(snapshot.churned_chats, 1);
        assert_eq!(snapshot.errors, 1);
    }
}
