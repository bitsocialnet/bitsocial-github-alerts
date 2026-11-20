use crate::webhooks::github::policy::AlertPolicy;
use crate::webhooks::github::security::WebhookSecurity;
use std::env;

/// Default hours between digests: one a day.
const DEFAULT_DIGEST_INTERVAL_HOURS: u32 = 24;

#[derive(Debug, Clone)]
pub struct AppConfig {
    pub database_url: String,
    pub webhook_base_url: String,

    pub port: u16,
    pub admin_logs: String,
    pub admin_chat_id: Option<i64>,
    pub admin_log_level: u8,

    pub github_token: String,
    pub github_webhook_security: WebhookSecurity,

    /// Built from `CORE_GITHUB_LOGINS`: whose issues and pull requests are
    /// only counted in the digest instead of posted as they happen.
    pub alert_policy: AlertPolicy,
    /// `DIGEST_INTERVAL_HOURS`: 0 turns the digest off; otherwise a divisor of 24.
    pub digest_interval_hours: u32,
}

#[derive(Debug)]
pub struct ConfigError {
    pub missing_vars: Vec<String>,
    pub invalid_vars: Vec<(String, String)>,
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if !self.missing_vars.is_empty() {
            writeln!(f, "Missing required environment variables:")?;
            for var in &self.missing_vars {
                writeln!(f, "  - {}", var)?;
            }
        }
        if !self.invalid_vars.is_empty() {
            writeln!(f, "Invalid environment variables:")?;
            for (var, err) in &self.invalid_vars {
                writeln!(f, "  - {}: {}", var, err)?;
            }
        }
        Ok(())
    }
}

impl std::error::Error for ConfigError {}

fn get_required(name: &str, missing: &mut Vec<String>) -> Option<String> {
    match env::var(name) {
        Ok(v) if !v.is_empty() => Some(v),
        _ => {
            missing.push(name.to_string());
            None
        }
    }
}

fn get_optional(name: &str) -> Option<String> {
    env::var(name).ok().filter(|v| !v.is_empty())
}

impl AppConfig {
    pub fn from_env() -> Result<Self, ConfigError> {
        let mut missing = Vec::new();
        let mut invalid = Vec::new();

        let database_url = get_required("DATABASE_URL", &mut missing);
        let webhook_base_url = get_required("WEBHOOK_BASE_URL", &mut missing);
        let github_token = get_required("GITHUB_TELOXIDE_TOKEN", &mut missing);
        let github_webhook_security =
            get_required("GITHUB_WEBHOOK_SECRET", &mut missing).and_then(|secret| {
                match WebhookSecurity::new(&secret) {
                    Ok(security) => Some(security),
                    Err(error) => {
                        invalid.push(("GITHUB_WEBHOOK_SECRET".into(), error.into()));
                        None
                    }
                }
            });

        let admin_logs = get_optional("ADMIN_LOGS").unwrap_or_else(|| "NOT_ACTIVE".into());

        let admin_chat_id = get_optional("TELEGRAM_ADMIN_CHAT_ID").and_then(|s| {
            s.parse::<i64>()
                .map_err(|e| {
                    invalid.push(("TELEGRAM_ADMIN_CHAT_ID".into(), e.to_string()));
                })
                .ok()
        });

        let admin_log_level = env::var("ADMIN_LOG_LEVEL")
            .unwrap_or_else(|_| "50".into())
            .parse::<u8>()
            .unwrap_or(50);

        let port = env::var("PORT")
            .unwrap_or_else(|_| "8080".into())
            .parse::<u16>()
            .map_err(|e| {
                invalid.push(("PORT".into(), e.to_string()));
            })
            .unwrap_or(8080);

        let alert_policy =
            AlertPolicy::from_core_logins(&get_optional("CORE_GITHUB_LOGINS").unwrap_or_default());

        let digest_interval_hours = get_optional("DIGEST_INTERVAL_HOURS")
            .map(|value| {
                parse_digest_interval_hours(&value).unwrap_or_else(|error| {
                    invalid.push(("DIGEST_INTERVAL_HOURS".into(), error));
                    DEFAULT_DIGEST_INTERVAL_HOURS
                })
            })
            .unwrap_or(DEFAULT_DIGEST_INTERVAL_HOURS);

        if !missing.is_empty() || !invalid.is_empty() {
            return Err(ConfigError {
                missing_vars: missing,
                invalid_vars: invalid,
            });
        }

        Ok(Self {
            database_url: database_url.unwrap(),
            webhook_base_url: webhook_base_url.unwrap(),
            port,
            admin_logs,
            admin_chat_id,
            admin_log_level,
            github_token: github_token.unwrap(),
            github_webhook_security: github_webhook_security.unwrap(),
            alert_policy,
            digest_interval_hours,
        })
    }

    pub fn is_admin_logs_active(&self) -> bool {
        self.admin_logs == "ACTIVE"
    }
}

/// Digests run at fixed UTC times every day, so the interval must divide 24.
fn parse_digest_interval_hours(value: &str) -> Result<u32, String> {
    let hours: u32 = value
        .trim()
        .parse()
        .map_err(|_| format!("'{value}' is not a whole number of hours"))?;
    if hours == 0 || 24 % hours == 0 {
        Ok(hours)
    } else {
        Err(format!(
            "{hours} does not divide 24; use 0 (off), 1, 2, 3, 4, 6, 8, 12 or 24"
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digest_interval_must_divide_a_day() {
        assert_eq!(parse_digest_interval_hours("24"), Ok(24));
        assert_eq!(parse_digest_interval_hours(" 6 "), Ok(6));
        assert_eq!(parse_digest_interval_hours("0"), Ok(0));
        assert!(parse_digest_interval_hours("5").is_err());
        assert!(parse_digest_interval_hours("48").is_err());
        assert!(parse_digest_interval_hours("daily").is_err());
    }
}
