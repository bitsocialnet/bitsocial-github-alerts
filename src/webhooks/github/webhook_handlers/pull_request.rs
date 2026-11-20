use super::utils::parse_webhook_payload;
use crate::utils::branch_filter::BranchFilter;
use crate::webhooks::github::policy::{AlertPolicy, GitHubUser};
use actix_web::web;
use html_escape::encode_text;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct PullRequestEvent {
    action: String,
    pull_request: PullRequest,
    repository: Repository,
    sender: GitHubUser,
}

#[derive(Debug, Deserialize)]
struct PullRequest {
    html_url: String,
    number: i64,
    title: String,
    merged: bool,
    user: GitHubUser,
    head: Branch,
    base: Branch,
}

#[derive(Debug, Deserialize)]
struct Branch {
    label: String,
}

#[derive(Debug, Deserialize)]
struct Repository {
    name: String,
    html_url: String,
}

/// Announce pull requests by outside contributors when they are opened and
/// when they are merged. Core maintainers' and bots' pull requests, and other
/// actions, are left to the digest.
pub fn handle_pull_request_event(
    body: &web::Bytes,
    branch_filter: Option<&BranchFilter>,
    policy: &AlertPolicy,
) -> String {
    let pr_event: PullRequestEvent = match parse_webhook_payload(body) {
        Ok(event) => event,
        Err(e) => {
            tracing::error!("Failed to parse pull request event: {}", e);
            tracing::error!("Raw payload: {}", String::from_utf8_lossy(body));
            return String::new();
        }
    };

    let pull_request = &pr_event.pull_request;
    let pr_title = encode_text(&pull_request.title);
    let pr_url = &pull_request.html_url;
    let pr_number = pull_request.number;
    let repository_name = encode_text(&pr_event.repository.name);
    let repository_url = &pr_event.repository.html_url;
    let author = encode_text(&pull_request.user.login);
    let sender = encode_text(&pr_event.sender.login);
    let source_branch = encode_text(&pull_request.head.label);
    let target_branch = encode_text(&pull_request.base.label);

    // Extract branch name from target branch label (format: "owner:branch-name")
    let target_branch_name = target_branch.split(':').next_back().unwrap_or("");

    // Apply branch filter if provided (filter based on target branch)
    if let Some(filter) = branch_filter {
        if !filter.should_process(target_branch_name) {
            tracing::info!(
                "Filtered out pull request event for target branch: {}",
                target_branch_name
            );
            return String::new();
        }
    }

    if !policy.is_outside_contributor(&pull_request.user) {
        return String::new();
    }

    match pr_event.action.as_str() {
        "opened" => format!(
            "<b>{author}</b> opened a new pull request <a href=\"{pr_url}\">#{pr_number}</a> in <a href=\"{repository_url}\">{repository_name}</a>:\n\
            {pr_title}\n\
            {source_branch} → {target_branch}"
        ),
        "closed" if pull_request.merged => format!(
            "<b>{sender}</b> merged <b>{author}</b>'s pull request <a href=\"{pr_url}\">#{pr_number}</a> in <a href=\"{repository_url}\">{repository_name}</a>:\n\
            {pr_title}"
        ),
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use actix_web::web::Bytes;
    use serde_json::json;

    fn policy() -> AlertPolicy {
        AlertPolicy::from_core_logins("tomcasaburi,Rinse12")
    }

    fn payload(action: &str, merged: bool, author: &str, sender: &str) -> Bytes {
        Bytes::from(
            json!({
                "action": action,
                "pull_request": {
                    "html_url": "https://github.com/bitsocialnet/seedit/pull/12",
                    "number": 12,
                    "title": "Fix the <b>catalog</b>",
                    "merged": merged,
                    "user": {"login": author, "type": if author.ends_with("[bot]") { "Bot" } else { "User" }},
                    "head": {"label": format!("{author}:fix-catalog")},
                    "base": {"label": "bitsocialnet:master"},
                },
                "repository": {
                    "name": "seedit",
                    "html_url": "https://github.com/bitsocialnet/seedit",
                },
                "sender": {"login": sender},
            })
            .to_string(),
        )
    }

    #[test]
    fn test_outside_contributor_pull_request_opened() {
        let message = handle_pull_request_event(
            &payload("opened", false, "trackoor", "trackoor"),
            None,
            &policy(),
        );
        assert_eq!(
            message,
            "<b>trackoor</b> opened a new pull request \
             <a href=\"https://github.com/bitsocialnet/seedit/pull/12\">#12</a> in \
             <a href=\"https://github.com/bitsocialnet/seedit\">seedit</a>:\n\
             Fix the &lt;b&gt;catalog&lt;/b&gt;\n\
             trackoor:fix-catalog → bitsocialnet:master"
        );
    }

    #[test]
    fn test_outside_contributor_pull_request_merged() {
        let message = handle_pull_request_event(
            &payload("closed", true, "trackoor", "Rinse12"),
            None,
            &policy(),
        );
        assert_eq!(
            message,
            "<b>Rinse12</b> merged <b>trackoor</b>'s pull request \
             <a href=\"https://github.com/bitsocialnet/seedit/pull/12\">#12</a> in \
             <a href=\"https://github.com/bitsocialnet/seedit\">seedit</a>:\n\
             Fix the &lt;b&gt;catalog&lt;/b&gt;"
        );
    }

    #[test]
    fn test_core_and_bot_pull_requests_not_announced() {
        for author in ["tomcasaburi", "RINSE12", "release-please[bot]"] {
            for (action, merged) in [("opened", false), ("closed", true)] {
                assert_eq!(
                    handle_pull_request_event(
                        &payload(action, merged, author, author),
                        None,
                        &policy()
                    ),
                    "",
                    "{author} {action}"
                );
            }
        }
    }

    #[test]
    fn test_other_actions_not_announced() {
        for (action, merged) in [
            ("closed", false),
            ("reopened", false),
            ("synchronize", false),
            ("edited", false),
        ] {
            assert_eq!(
                handle_pull_request_event(
                    &payload(action, merged, "trackoor", "trackoor"),
                    None,
                    &policy()
                ),
                "",
                "{action}"
            );
        }
    }
}
