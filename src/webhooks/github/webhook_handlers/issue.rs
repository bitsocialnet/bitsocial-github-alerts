use super::utils::parse_webhook_payload;
use crate::webhooks::github::policy::{AlertPolicy, GitHubUser};
use actix_web::web;
use html_escape::encode_text;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct IssueEvent {
    action: String,
    issue: Issue,
    repository: Repository,
    changes: Option<Changes>,
}

#[derive(Debug, Deserialize)]
struct Issue {
    html_url: String,
    number: i64,
    title: String,
    user: GitHubUser,
}

#[derive(Debug, Deserialize)]
struct Repository {
    name: String,
    html_url: String,
}

/// Transfer metadata: GitHub sends `old_issue` to the destination repository
/// on "opened" when an issue is transferred there.
#[derive(Debug, Deserialize)]
struct Changes {
    old_issue: Option<serde_json::Value>,
}

/// Announce issues opened by outside contributors. Core maintainers' and
/// bots' issues, and closes, reopens and transfers, are left to the digest.
pub fn handle_issue_event(body: &web::Bytes, policy: &AlertPolicy) -> String {
    let issue_event: IssueEvent = match parse_webhook_payload(body) {
        Ok(event) => event,
        Err(e) => {
            tracing::error!("Failed to parse issue event: {}", e);
            tracing::error!("Raw payload: {}", String::from_utf8_lossy(body));
            return String::new();
        }
    };

    let issue = &issue_event.issue;
    // A transferred issue arrives at its destination as "opened" with the
    // original author; it is not a new issue.
    let is_transfer = issue_event
        .changes
        .as_ref()
        .is_some_and(|changes| changes.old_issue.is_some());

    if issue_event.action != "opened" || is_transfer || !policy.is_outside_contributor(&issue.user)
    {
        return String::new();
    }

    let author = encode_text(&issue.user.login);
    let issue_title = encode_text(&issue.title);
    let issue_url = &issue.html_url;
    let issue_number = issue.number;
    let repository_name = encode_text(&issue_event.repository.name);
    let repository_url = &issue_event.repository.html_url;

    format!(
        "<b>{author}</b> opened a new issue <a href=\"{issue_url}\">#{issue_number}</a> in <a href=\"{repository_url}\">{repository_name}</a>:\n{issue_title}"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use actix_web::web::Bytes;
    use serde_json::{json, Value};

    fn policy() -> AlertPolicy {
        AlertPolicy::from_core_logins("tomcasaburi,Rinse12")
    }

    fn payload(action: &str, author: &str, changes: Option<Value>) -> Bytes {
        let mut event = json!({
            "action": action,
            "issue": {
                "html_url": "https://github.com/pkcprotocol/pkc-js/issues/557",
                "number": 557,
                "title": "README `require('@pkcprotocol/pkc-js')` examples fail",
                "user": {"login": author, "type": if author.ends_with("[bot]") { "Bot" } else { "User" }},
            },
            "repository": {
                "name": "pkc-js",
                "html_url": "https://github.com/pkcprotocol/pkc-js",
            },
            "sender": {"login": author}
        });

        if let Some(changes) = changes {
            event["changes"] = changes;
        }

        Bytes::from(event.to_string())
    }

    #[test]
    fn test_outside_contributor_issue_announced() {
        let message = handle_issue_event(&payload("opened", "trackoor", None), &policy());
        assert_eq!(
            message,
            "<b>trackoor</b> opened a new issue \
             <a href=\"https://github.com/pkcprotocol/pkc-js/issues/557\">#557</a> in \
             <a href=\"https://github.com/pkcprotocol/pkc-js\">pkc-js</a>:\n\
             README `require('@pkcprotocol/pkc-js')` examples fail"
        );
    }

    #[test]
    fn test_core_and_bot_issues_not_announced() {
        for author in ["tomcasaburi", "rinse12", "github-actions[bot]"] {
            assert_eq!(
                handle_issue_event(&payload("opened", author, None), &policy()),
                "",
                "{author}"
            );
        }
    }

    #[test]
    fn test_received_transfer_is_not_announced_as_opened() {
        let changes = json!({"old_issue": {"number": 17}});
        assert_eq!(
            handle_issue_event(&payload("opened", "trackoor", Some(changes)), &policy()),
            ""
        );
    }

    #[test]
    fn test_other_actions_not_announced() {
        for action in [
            "closed",
            "reopened",
            "transferred",
            "edited",
            "labeled",
            "assigned",
        ] {
            assert_eq!(
                handle_issue_event(&payload(action, "trackoor", None), &policy()),
                "",
                "{action}"
            );
        }
    }

    #[test]
    fn test_html_escaping() {
        let mut event: Value =
            serde_json::from_slice(&payload("opened", "trackoor", None)).unwrap();
        event["issue"]["title"] = json!("<b>x</b> & y");
        let message = handle_issue_event(&Bytes::from(event.to_string()), &policy());
        assert!(message.ends_with("&lt;b&gt;x&lt;/b&gt; &amp; y"));
    }
}
