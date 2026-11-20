//! Turns webhook deliveries into the activity a chat's digest counts: commits
//! that land on a default branch, published releases, issues opened and
//! closed, and merged pull requests. Bot activity is not counted, except that
//! a release published by a bot is still a release.

use super::policy::{is_bot_login, GitHubUser};
use super::webhook_handlers::utils::parse_webhook_payload;
use bitsocial_github_alerts::models::{digest_kind, NewDigestEvent};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct Repository {
    full_name: String,
    #[serde(default)]
    default_branch: Option<String>,
}

#[derive(Debug, Deserialize)]
struct PushPayload {
    #[serde(rename = "ref")]
    git_ref: String,
    #[serde(default)]
    deleted: bool,
    #[serde(default)]
    commits: Vec<PushCommit>,
    repository: Repository,
    sender: GitHubUser,
}

#[derive(Debug, Deserialize)]
struct PushCommit {
    id: String,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    author: Option<CommitAuthor>,
}

#[derive(Debug, Deserialize)]
struct CommitAuthor {
    /// GitHub login of the commit author; absent when the author's email is
    /// not linked to a GitHub account (agents, some GitHub App bots).
    #[serde(default)]
    username: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    email: Option<String>,
}

impl CommitAuthor {
    /// GitHub App bots commit as `name[bot]` with a
    /// `id+name[bot]@users.noreply.github.com` email, often with no linked login.
    fn is_bot(&self) -> bool {
        self.username.as_deref().is_some_and(is_bot_login)
            || self.name.as_deref().is_some_and(is_bot_login)
            || self
                .email
                .as_deref()
                .is_some_and(|email| email.to_ascii_lowercase().contains("[bot]@"))
    }
}

#[derive(Debug, Deserialize)]
struct IssuesPayload {
    action: String,
    issue: Thread,
    repository: Repository,
    sender: GitHubUser,
    #[serde(default)]
    changes: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct PullRequestPayload {
    action: String,
    pull_request: PullRequest,
    repository: Repository,
}

#[derive(Debug, Deserialize)]
struct Thread {
    number: i64,
    html_url: String,
    user: GitHubUser,
}

#[derive(Debug, Deserialize)]
struct PullRequest {
    number: i64,
    html_url: String,
    user: GitHubUser,
    #[serde(default)]
    merged: bool,
}

#[derive(Debug, Deserialize)]
struct ReleasePayload {
    action: String,
    release: Release,
    repository: Repository,
}

#[derive(Debug, Deserialize)]
struct Release {
    tag_name: String,
    html_url: String,
    #[serde(default)]
    author: Option<GitHubUser>,
}

/// Activity in one delivery, ready to be stored for `chat_id`. Empty when the
/// delivery has nothing a digest counts or cannot be parsed.
pub fn digest_activity(event: &str, body: &[u8], chat_id: i32) -> Vec<NewDigestEvent> {
    let new_event = |kind: &str, repo: &str, item: String, actor, url| NewDigestEvent {
        chat_id,
        kind: kind.to_string(),
        repo: repo.to_string(),
        item,
        actor,
        url,
    };

    match event {
        "push" => {
            let Ok(push) = parse_webhook_payload::<PushPayload>(body) else {
                return Vec::new();
            };
            if push.deleted || !pushes_to_default_branch(&push) {
                return Vec::new();
            }
            // Every commit the push brings onto the default branch counts,
            // including ones first pushed to a feature branch (GitHub marks
            // those `distinct: false`); otherwise merged pull requests would
            // vanish from the count. The stored SHA makes each commit count once.
            push.commits
                .iter()
                .filter_map(|commit| {
                    // Bot commits are skipped. Others are credited to their
                    // author's login, or, without one, to the person who
                    // pushed them; a bot pushing them gets no credit.
                    if commit.author.as_ref().is_some_and(CommitAuthor::is_bot) {
                        return None;
                    }
                    let actor = match commit.author.as_ref().and_then(|a| a.username.as_deref()) {
                        Some(author) => Some(author),
                        None if push.sender.is_bot() => None,
                        None => Some(push.sender.login.as_str()),
                    };
                    Some(new_event(
                        digest_kind::COMMIT,
                        &push.repository.full_name,
                        commit.id.clone(),
                        actor.map(str::to_string),
                        commit.url.clone(),
                    ))
                })
                .collect()
        }
        "issues" => {
            let Ok(issues) = parse_webhook_payload::<IssuesPayload>(body) else {
                return Vec::new();
            };
            let issue = &issues.issue;
            let (kind, actor) = match issues.action.as_str() {
                // The destination copy of a transferred issue is not a new issue.
                "opened" if is_transfer_destination(issues.changes.as_ref()) => return Vec::new(),
                "opened" => (digest_kind::ISSUE_OPENED, &issue.user),
                "closed" => (digest_kind::ISSUE_CLOSED, &issues.sender),
                _ => return Vec::new(),
            };
            if actor.is_bot() {
                return Vec::new();
            }
            vec![new_event(
                kind,
                &issues.repository.full_name,
                issue.number.to_string(),
                Some(actor.login.clone()),
                Some(issue.html_url.clone()),
            )]
        }
        "pull_request" => {
            let Ok(pr) = parse_webhook_payload::<PullRequestPayload>(body) else {
                return Vec::new();
            };
            let pull_request = &pr.pull_request;
            if pr.action != "closed" || !pull_request.merged || pull_request.user.is_bot() {
                return Vec::new();
            }
            vec![new_event(
                digest_kind::PR_MERGED,
                &pr.repository.full_name,
                pull_request.number.to_string(),
                Some(pull_request.user.login.clone()),
                Some(pull_request.html_url.clone()),
            )]
        }
        "release" => {
            let Ok(release) = parse_webhook_payload::<ReleasePayload>(body) else {
                return Vec::new();
            };
            if release.action != "published" {
                return Vec::new();
            }
            let actor = release
                .release
                .author
                .as_ref()
                .filter(|author| !author.is_bot())
                .map(|author| author.login.clone());
            vec![new_event(
                digest_kind::RELEASE,
                &release.repository.full_name,
                release.release.tag_name.clone(),
                actor,
                Some(release.release.html_url.clone()),
            )]
        }
        _ => Vec::new(),
    }
}

fn pushes_to_default_branch(push: &PushPayload) -> bool {
    let Some(branch) = push.git_ref.strip_prefix("refs/heads/") else {
        return false; // tags
    };
    match push.repository.default_branch.as_deref() {
        Some(default_branch) => branch == default_branch,
        None => branch == "master" || branch == "main",
    }
}

fn is_transfer_destination(changes: Option<&serde_json::Value>) -> bool {
    changes.is_some_and(|changes| changes.get("old_issue").is_some())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};

    const CHAT: i32 = 7;

    fn activity(event: &str, payload: Value) -> Vec<NewDigestEvent> {
        digest_activity(event, payload.to_string().as_bytes(), CHAT)
    }

    fn push(git_ref: &str, sender: &str, commits: Value) -> Value {
        json!({
            "ref": git_ref,
            "deleted": false,
            "commits": commits,
            "repository": {"full_name": "bitsocialnet/5chan", "default_branch": "master"},
            "sender": {"login": sender, "type": if sender.ends_with("[bot]") { "Bot" } else { "User" }},
        })
    }

    fn commit(id: &str, username: Option<&str>, distinct: bool) -> Value {
        json!({
            "id": id,
            "url": format!("https://github.com/bitsocialnet/5chan/commit/{id}"),
            "distinct": distinct,
            "author": {"name": "Someone", "username": username},
        })
    }

    #[test]
    fn counts_every_commit_landing_on_the_default_branch() {
        let events = activity(
            "push",
            push(
                "refs/heads/master",
                "tomcasaburi",
                json!([
                    commit("aaa", Some("trackoor"), false),
                    commit("bbb", None, true),
                ]),
            ),
        );
        assert_eq!(events.len(), 2);
        assert!(events.iter().all(|e| e.kind == digest_kind::COMMIT
            && e.repo == "bitsocialnet/5chan"
            && e.chat_id == CHAT));
        // A merged contributor's commit credits them; an unlinked author
        // (an agent commit) credits whoever pushed it.
        assert_eq!(events[0].item, "aaa");
        assert_eq!(events[0].actor.as_deref(), Some("trackoor"));
        assert_eq!(events[1].actor.as_deref(), Some("tomcasaburi"));
    }

    #[test]
    fn ignores_feature_branches_tags_deletions_and_bots() {
        let commits = json!([commit("aaa", Some("tomcasaburi"), true)]);
        assert!(activity(
            "push",
            push("refs/heads/feat/x", "tomcasaburi", commits.clone())
        )
        .is_empty());
        assert!(activity(
            "push",
            push("refs/tags/v1.0.0", "tomcasaburi", commits.clone())
        )
        .is_empty());
        // A bot's own commits are skipped, linked or not; people's commits it
        // pushes count, credited to their login if they have one.
        let release_bot_commit = json!({
            "id": "ddd",
            "distinct": true,
            "author": {
                "name": "pkc-js-release-bot[bot]",
                "email": "3394437+pkc-js-release-bot[bot]@users.noreply.github.com",
            },
        });
        let pushed_by_bot = activity(
            "push",
            push(
                "refs/heads/master",
                "pkc-js-release-bot[bot]",
                json!([
                    commit("aaa", None, true),
                    commit("bbb", Some("github-actions[bot]"), true),
                    commit("ccc", Some("trackoor"), true),
                    release_bot_commit
                ]),
            ),
        );
        let items: Vec<(&str, Option<&str>)> = pushed_by_bot
            .iter()
            .map(|e| (e.item.as_str(), e.actor.as_deref()))
            .collect();
        assert_eq!(items, vec![("aaa", None), ("ccc", Some("trackoor"))]);

        let mut deleted = push("refs/heads/master", "tomcasaburi", commits);
        deleted["deleted"] = json!(true);
        assert!(activity("push", deleted).is_empty());

        // A bot-authored commit inside a person's push is not counted either.
        let events = activity(
            "push",
            push(
                "refs/heads/master",
                "tomcasaburi",
                json!([
                    commit("aaa", Some("github-actions[bot]"), false),
                    commit("bbb", Some("tomcasaburi"), true)
                ]),
            ),
        );
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].item, "bbb");
    }

    #[test]
    fn falls_back_to_master_or_main_without_a_default_branch() {
        let mut payload = push(
            "refs/heads/main",
            "Rinse12",
            json!([commit("aaa", None, true)]),
        );
        payload["repository"]
            .as_object_mut()
            .unwrap()
            .remove("default_branch");
        assert_eq!(activity("push", payload).len(), 1);
    }

    fn issue(action: &str, author: &str, sender: &str) -> Value {
        json!({
            "action": action,
            "issue": {
                "number": 557,
                "html_url": "https://github.com/pkcprotocol/pkc-js/issues/557",
                "user": {"login": author, "type": "User"},
            },
            "repository": {"full_name": "pkcprotocol/pkc-js"},
            "sender": {"login": sender, "type": "User"},
        })
    }

    #[test]
    fn counts_issues_opened_and_closed() {
        let opened = activity("issues", issue("opened", "trackoor", "trackoor"));
        assert_eq!(opened.len(), 1);
        assert_eq!(opened[0].kind, digest_kind::ISSUE_OPENED);
        assert_eq!(opened[0].item, "557");
        assert_eq!(opened[0].actor.as_deref(), Some("trackoor"));

        let closed = activity("issues", issue("closed", "trackoor", "Rinse12"));
        assert_eq!(closed[0].kind, digest_kind::ISSUE_CLOSED);
        assert_eq!(closed[0].actor.as_deref(), Some("Rinse12"));

        for action in ["reopened", "edited", "labeled", "transferred"] {
            assert!(activity("issues", issue(action, "trackoor", "trackoor")).is_empty());
        }
    }

    #[test]
    fn a_transferred_issue_is_not_counted_as_opened() {
        let mut payload = issue("opened", "trackoor", "trackoor");
        payload["changes"] = json!({"old_issue": {"number": 1}});
        assert!(activity("issues", payload).is_empty());
    }

    fn pull_request(action: &str, merged: bool, author: &str) -> Value {
        json!({
            "action": action,
            "pull_request": {
                "number": 12,
                "html_url": "https://github.com/bitsocialnet/seedit/pull/12",
                "merged": merged,
                "user": {"login": author, "type": if author.ends_with("[bot]") { "Bot" } else { "User" }},
            },
            "repository": {"full_name": "bitsocialnet/seedit"},
        })
    }

    #[test]
    fn counts_merged_pull_requests_by_people() {
        let merged = activity("pull_request", pull_request("closed", true, "trackoor"));
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].kind, digest_kind::PR_MERGED);
        assert_eq!(merged[0].actor.as_deref(), Some("trackoor"));

        assert!(activity("pull_request", pull_request("closed", false, "trackoor")).is_empty());
        assert!(activity("pull_request", pull_request("opened", false, "trackoor")).is_empty());
        assert!(activity(
            "pull_request",
            pull_request("closed", true, "release-please[bot]")
        )
        .is_empty());
    }

    fn release(action: &str, author: &str) -> Value {
        json!({
            "action": action,
            "release": {
                "tag_name": "v0.19.144",
                "html_url": "https://github.com/bitsocialnet/bitsocial-cli/releases/tag/v0.19.144",
                "author": {"login": author, "type": if author.ends_with("[bot]") { "Bot" } else { "User" }},
            },
            "repository": {"full_name": "bitsocialnet/bitsocial-cli"},
        })
    }

    #[test]
    fn counts_published_releases_including_bot_ones() {
        let events = activity("release", release("published", "github-actions[bot]"));
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind, digest_kind::RELEASE);
        assert_eq!(events[0].item, "v0.19.144");
        assert_eq!(events[0].actor, None);

        let events = activity("release", release("published", "tomcasaburi"));
        assert_eq!(events[0].actor.as_deref(), Some("tomcasaburi"));

        for action in ["created", "edited", "prereleased", "released"] {
            assert!(activity("release", release(action, "tomcasaburi")).is_empty());
        }
    }

    #[test]
    fn other_events_and_bad_payloads_record_nothing() {
        assert!(activity("ping", json!({"zen": "hi"})).is_empty());
        assert!(activity("push", json!({"unexpected": true})).is_empty());
        assert!(digest_activity("release", b"not json", CHAT).is_empty());
    }
}
