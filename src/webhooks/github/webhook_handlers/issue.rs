use super::utils::parse_webhook_payload;
use actix_web::web;
use html_escape::encode_text;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct IssueEvent {
    action: String,
    issue: Issue,
    repository: Repository,
    sender: Sender,
    changes: Option<Changes>,
}

#[derive(Debug, Deserialize)]
struct Issue {
    html_url: String,
    number: i64,
    title: String,
}

#[derive(Debug, Deserialize)]
struct Repository {
    name: String,
    html_url: String,
}

#[derive(Debug, Deserialize)]
struct Sender {
    login: String,
}

/// Transfer metadata. GitHub sends `new_issue`/`new_repository` to the source
/// repository on "transferred", and `old_issue`/`old_repository` to the
/// destination repository on "opened".
#[derive(Debug, Deserialize)]
struct Changes {
    new_issue: Option<TransferredIssue>,
    new_repository: Option<Repository>,
    old_issue: Option<TransferredIssue>,
}

#[derive(Debug, Deserialize)]
struct TransferredIssue {
    html_url: String,
    number: i64,
}

pub fn handle_issue_event(body: &web::Bytes) -> String {
    let issue_event: IssueEvent = match parse_webhook_payload(body) {
        Ok(event) => event,
        Err(e) => {
            tracing::error!("Failed to parse issue event: {}", e);
            tracing::error!("Raw payload: {}", String::from_utf8_lossy(body));
            return String::new();
        }
    };

    let action = &issue_event.action;
    let issue_title = encode_text(&issue_event.issue.title);
    let issue_url = &issue_event.issue.html_url;
    let issue_number = issue_event.issue.number;
    let repository_name = encode_text(&issue_event.repository.name);
    let repository_url = &issue_event.repository.html_url;
    let sender = encode_text(&issue_event.sender.login);
    let changes = issue_event.changes.as_ref();

    match action.as_str() {
        // A transfer fires twice: "transferred" on the source repository, where the
        // sender is whoever moved the issue, and "opened" on the destination, where the
        // sender is the original author. Only the source event identifies the actor, so
        // announce the transfer there and drop the destination copy to avoid reporting
        // a transfer as a brand new issue. A transfer out of a repository this bot does
        // not receive events for is therefore not announced at all.
        "transferred" => {
            let new_issue = changes.and_then(|c| c.new_issue.as_ref());
            let new_repository = changes.and_then(|c| c.new_repository.as_ref());

            match (new_issue, new_repository) {
                (Some(new_issue), Some(new_repository)) => {
                    let new_issue_url = &new_issue.html_url;
                    let new_issue_number = new_issue.number;
                    let new_repository_name = encode_text(&new_repository.name);
                    format!(
                        "<b>{sender}</b> transferred issue <a href=\"{issue_url}\">#{issue_number}</a> in <a href=\"{repository_url}\">{repository_name}</a> to <a href=\"{new_issue_url}\">{new_repository_name}#{new_issue_number}</a>:\n{issue_title}"
                    )
                }
                _ => format!(
                    "<b>{sender}</b> transferred issue <a href=\"{issue_url}\">#{issue_number}</a> out of <a href=\"{repository_url}\">{repository_name}</a>:\n{issue_title}"
                ),
            }
        }
        "opened" if changes.is_some_and(|c| c.old_issue.is_some()) => String::new(),
        "opened" => format!(
            "<b>{sender}</b> opened a new issue <a href=\"{issue_url}\">#{issue_number}</a> in <a href=\"{repository_url}\">{repository_name}</a>:\n{issue_title}"
        ),
        "closed" => format!(
            "<b>{sender}</b> closed issue <a href=\"{issue_url}\">#{issue_number}</a> in <a href=\"{repository_url}\">{repository_name}</a>"
        ),
        "reopened" => format!(
            "<b>{sender}</b> reopened issue <a href=\"{issue_url}\">#{issue_number}</a> in <a href=\"{repository_url}\">{repository_name}</a>"
        ),
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use actix_web::web::Bytes;
    use serde_json::{json, Value};

    fn payload(action: &str, sender: &str, changes: Option<Value>) -> Bytes {
        let mut event = json!({
            "action": action,
            "issue": {
                "html_url": "https://github.com/bitsocialnet/bitsocial-web/issues/17",
                "number": 17,
                "title": "Add community settings UI for configurable page sorts (feeds)",
            },
            "repository": {
                "name": "bitsocial-web",
                "html_url": "https://github.com/bitsocialnet/bitsocial-web",
            },
            "sender": {
                "login": sender
            }
        });

        if let Some(changes) = changes {
            event["changes"] = changes;
        }

        Bytes::from(event.to_string())
    }

    fn transferred_changes() -> Value {
        json!({
            "new_issue": {
                "html_url": "https://github.com/bitsocialnet/seedit/issues/842",
                "number": 842,
                "title": "Add community settings UI for configurable page sorts (feeds)",
            },
            "new_repository": {
                "name": "seedit",
                "html_url": "https://github.com/bitsocialnet/seedit",
            }
        })
    }

    fn received_transfer_changes() -> Value {
        json!({
            "old_issue": {
                "html_url": "https://github.com/bitsocialnet/bitsocial-web/issues/17",
                "number": 17,
                "title": "Add community settings UI for configurable page sorts (feeds)",
            },
            "old_repository": {
                "name": "bitsocial-web",
                "html_url": "https://github.com/bitsocialnet/bitsocial-web",
            }
        })
    }

    #[test]
    fn test_transferred_names_the_actor_and_both_ends() {
        let message = handle_issue_event(&payload(
            "transferred",
            "tomcasaburi",
            Some(transferred_changes()),
        ));
        assert_eq!(
            message,
            "<b>tomcasaburi</b> transferred issue \
             <a href=\"https://github.com/bitsocialnet/bitsocial-web/issues/17\">#17</a> in \
             <a href=\"https://github.com/bitsocialnet/bitsocial-web\">bitsocial-web</a> to \
             <a href=\"https://github.com/bitsocialnet/seedit/issues/842\">seedit#842</a>:\n\
             Add community settings UI for configurable page sorts (feeds)"
        );
    }

    #[test]
    fn test_transferred_without_destination_details() {
        let message = handle_issue_event(&payload("transferred", "tomcasaburi", None));
        assert!(message.starts_with("<b>tomcasaburi</b> transferred issue "));
        assert!(message.contains(
            "out of <a href=\"https://github.com/bitsocialnet/bitsocial-web\">bitsocial-web</a>"
        ));
    }

    #[test]
    fn test_received_transfer_is_not_announced_as_opened() {
        // The destination repository gets "opened" with the original author as sender;
        // the source repository's "transferred" event carries the real actor.
        assert_eq!(
            handle_issue_event(&payload(
                "opened",
                "Rinse12",
                Some(received_transfer_changes())
            )),
            ""
        );
    }

    #[test]
    fn test_genuinely_opened_issue_still_announced() {
        let message = handle_issue_event(&payload("opened", "Rinse12", None));
        assert_eq!(
            message,
            "<b>Rinse12</b> opened a new issue \
             <a href=\"https://github.com/bitsocialnet/bitsocial-web/issues/17\">#17</a> in \
             <a href=\"https://github.com/bitsocialnet/bitsocial-web\">bitsocial-web</a>:\n\
             Add community settings UI for configurable page sorts (feeds)"
        );
    }

    #[test]
    fn test_closed_and_reopened_unchanged() {
        assert!(handle_issue_event(&payload("closed", "tomcasaburi", None))
            .starts_with("<b>tomcasaburi</b> closed issue "));
        assert!(
            handle_issue_event(&payload("reopened", "tomcasaburi", None))
                .starts_with("<b>tomcasaburi</b> reopened issue ")
        );
    }

    #[test]
    fn test_other_actions_ignored() {
        for action in ["edited", "labeled", "assigned", "milestoned"] {
            assert_eq!(
                handle_issue_event(&payload(action, "tomcasaburi", None)),
                ""
            );
        }
    }
}
