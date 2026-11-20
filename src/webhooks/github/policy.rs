use serde::Deserialize;
use std::collections::HashSet;

/// A GitHub account as it appears in webhook payloads (`sender`, `issue.user`,
/// `pull_request.user`, `release.author`).
#[derive(Debug, Clone, Deserialize)]
pub struct GitHubUser {
    pub login: String,
    #[serde(rename = "type", default)]
    pub account_type: Option<String>,
}

impl GitHubUser {
    pub fn is_bot(&self) -> bool {
        is_bot_login(&self.login) || self.account_type.as_deref() == Some("Bot")
    }
}

/// GitHub App accounts, such as `github-actions[bot]`, end in `[bot]`.
pub fn is_bot_login(login: &str) -> bool {
    login.to_ascii_lowercase().ends_with("[bot]")
}

/// Decides whose issues and pull requests are posted as they happen. Activity
/// by core maintainers and bots only reaches chats through the digest.
#[derive(Debug, Clone, Default)]
pub struct AlertPolicy {
    core_logins: HashSet<String>,
}

impl AlertPolicy {
    /// Build from a comma-separated list of GitHub logins, as in
    /// `CORE_GITHUB_LOGINS=tomcasaburi,Rinse12`. Logins are case-insensitive.
    pub fn from_core_logins(list: &str) -> Self {
        Self {
            core_logins: list
                .split(',')
                .map(|login| login.trim().to_ascii_lowercase())
                .filter(|login| !login.is_empty())
                .collect(),
        }
    }

    /// True when the author is neither a core maintainer nor a bot.
    pub fn is_outside_contributor(&self, author: &GitHubUser) -> bool {
        !author.is_bot()
            && !self
                .core_logins
                .contains(&author.login.to_ascii_lowercase())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user(login: &str, account_type: Option<&str>) -> GitHubUser {
        GitHubUser {
            login: login.to_string(),
            account_type: account_type.map(str::to_string),
        }
    }

    #[test]
    fn core_logins_are_trimmed_and_case_insensitive() {
        let policy = AlertPolicy::from_core_logins(" tomcasaburi , Rinse12,,");
        assert!(!policy.is_outside_contributor(&user("TomCasaburi", Some("User"))));
        assert!(!policy.is_outside_contributor(&user("rinse12", Some("User"))));
        assert!(policy.is_outside_contributor(&user("trackoor", Some("User"))));
    }

    #[test]
    fn bots_are_never_outside_contributors() {
        let policy = AlertPolicy::from_core_logins("");
        assert!(!policy.is_outside_contributor(&user("github-actions[bot]", Some("Bot"))));
        assert!(!policy.is_outside_contributor(&user("pkc-js-release-bot[bot]", None)));
        assert!(!policy.is_outside_contributor(&user("some-app", Some("Bot"))));
    }

    #[test]
    fn an_empty_core_list_treats_every_person_as_outside() {
        let policy = AlertPolicy::from_core_logins("");
        assert!(policy.is_outside_contributor(&user("tomcasaburi", None)));
    }
}
