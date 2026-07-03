use super::utils::parse_webhook_payload;
use crate::utils::branch_filter::BranchFilter;
use actix_web::web;
use html_escape::encode_text;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct PushEvent {
    repository: Repository,
    sender: Sender,
    forced: bool,
    commits: Vec<Commit>,
    #[serde(rename = "ref")]
    ref_field: String,
    before: String,
    after: String,
}

#[derive(Debug, Deserialize)]
struct Commit {
    message: String,
    url: String,
    author: Author,
}

#[derive(Debug, Deserialize)]
struct Author {
    name: String,
}

#[derive(Debug, Deserialize)]
struct Repository {
    html_url: String,
    name: String,
}

#[derive(Debug, Deserialize)]
struct Sender {
    login: String,
}

const MAX_COMMITS_SHOWN: usize = 5;
const MAX_COMMIT_LINE_CHARS: usize = 72;

pub fn handle_push_event(body: &web::Bytes, branch_filter: Option<&BranchFilter>) -> String {
    let push_event: PushEvent = match parse_webhook_payload(body) {
        Ok(event) => event,
        Err(e) => {
            tracing::error!("Failed to parse push event: {}", e);
            tracing::error!("Raw payload: {}", String::from_utf8_lossy(body));
            return String::new();
        }
    };

    // Extract branch name from ref field (refs/heads/branch-name)
    let branch_name = push_event
        .ref_field
        .split("refs/heads/")
        .last()
        .unwrap_or("");

    // Apply branch filter if provided
    if let Some(filter) = branch_filter {
        if !filter.should_process(branch_name) {
            tracing::info!("Filtered out push event for branch: {}", branch_name);
            return String::new();
        }
    }

    let CreateFirstRow {
        first_row,
        delete_branch_event,
    } = create_first_row(&push_event);

    if delete_branch_event {
        return first_row;
    }

    let mut commit_paragraph = first_row;

    // Newest first, capped at MAX_COMMITS_SHOWN commits
    for commit in push_event.commits.iter().rev().take(MAX_COMMITS_SHOWN) {
        let commit_url = &commit.url;
        let summary = commit_summary(&commit.message);
        let commit_message = encode_text(&summary);
        let commit_author_name = encode_text(&commit.author.name);

        commit_paragraph.push_str(&format!(
            "<b>{commit_author_name}</b>: \
            <a href=\"{commit_url}\">{commit_message}</a>\n",
        ));
    }

    let total_commits = push_event.commits.len();
    if total_commits > MAX_COMMITS_SHOWN {
        commit_paragraph.push_str(&format!(
            "… and {} more\n",
            total_commits - MAX_COMMITS_SHOWN
        ));
    }

    commit_paragraph
}

/// First line of a commit message, truncated to MAX_COMMIT_LINE_CHARS characters.
fn commit_summary(message: &str) -> String {
    let first_line = message.lines().next().unwrap_or("").trim_end();

    if first_line.chars().count() > MAX_COMMIT_LINE_CHARS {
        let truncated: String = first_line.chars().take(MAX_COMMIT_LINE_CHARS - 1).collect();
        format!("{}…", truncated.trim_end())
    } else {
        first_line.to_string()
    }
}

struct CreateFirstRow {
    first_row: String,
    delete_branch_event: bool,
}

fn create_first_row(push_event: &PushEvent) -> CreateFirstRow {
    let branch_name_raw = push_event
        .ref_field
        .split("refs/heads/")
        .last()
        .unwrap_or(&push_event.ref_field);
    let project_url = &push_event.repository.html_url;
    let branch_url = format!("{project_url}/tree/{branch_name_raw}");

    let branch_name = encode_text(branch_name_raw);
    let project_name = encode_text(&push_event.repository.name);
    let sender = encode_text(&push_event.sender.login);
    let mut delete_branch_event = false;
    let commits_length = push_event.commits.len();
    let commit_or_commits = if push_event.commits.len() > 1 {
        "commits"
    } else {
        "commit"
    };

    let first_row = if push_event.forced {
        format!(
            "<b>{sender}</b> force pushed to <a href=\"{branch_url}\">{project_name}:{branch_name}</a>\n\n"
        )
    } else if push_event.before == "0000000000000000000000000000000000000000" {
        format!(
            "<b>{sender}</b> created branch <a href=\"{branch_url}\">{branch_name}</a> \
              and pushed {commits_length} {commit_or_commits} to \
            <a href=\"{branch_url}\">{project_name}:{branch_name}</a>\n\n"
        )
    } else if push_event.after == "0000000000000000000000000000000000000000" {
        delete_branch_event = true;
        format!(
            "<b>{sender}</b> deleted branch <a href=\"{branch_url}\">{project_name}:{branch_name}</a>\n\n"
        )
    } else {
        format!(
            "<b>{sender}</b> pushed {commits_length} {commit_or_commits} to \
            <a href=\"{branch_url}\">{project_name}:{branch_name}</a>\n\n"
        )
    };

    CreateFirstRow {
        first_row,
        delete_branch_event,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use actix_web::web::Bytes;
    use serde_json::json;

    fn push_payload(commit_messages: &[&str]) -> Bytes {
        let commits: Vec<_> = commit_messages
            .iter()
            .enumerate()
            .map(|(i, m)| {
                json!({
                    "message": m,
                    "url": format!("https://github.com/bitsocialnet/example/commit/{i}"),
                    "author": {"name": "octocat"}
                })
            })
            .collect();

        Bytes::from(
            json!({
                "ref": "refs/heads/main",
                "before": "1111111111111111111111111111111111111111",
                "after": "2222222222222222222222222222222222222222",
                "forced": false,
                "commits": commits,
                "repository": {
                    "html_url": "https://github.com/bitsocialnet/example",
                    "name": "example"
                },
                "sender": {"login": "octocat"}
            })
            .to_string(),
        )
    }

    #[test]
    fn test_commit_list_capped_at_five() {
        let messages: Vec<String> = (1..=8).map(|i| format!("commit {i}")).collect();
        let refs: Vec<&str> = messages.iter().map(|s| s.as_str()).collect();
        let result = handle_push_event(&push_payload(&refs), None);

        assert!(result.contains("pushed 8 commits"));
        // Newest five commits shown
        for i in 4..=8 {
            assert!(
                result.contains(&format!("commit {i}")),
                "missing commit {i}"
            );
        }
        assert!(!result.contains("commit 3</a>"));
        assert!(result.contains("… and 3 more"));
    }

    #[test]
    fn test_no_more_line_for_few_commits() {
        let result = handle_push_event(&push_payload(&["one", "two"]), None);
        assert!(!result.contains("more"));
    }

    #[test]
    fn test_commit_summary_first_line_only() {
        let result = handle_push_event(
            &push_payload(&["short title\n\nlong body with details"]),
            None,
        );
        assert!(result.contains("short title"));
        assert!(!result.contains("long body"));
    }

    #[test]
    fn test_commit_summary_truncated() {
        let long = "x".repeat(120);
        let summary = commit_summary(&long);
        assert!(summary.chars().count() <= MAX_COMMIT_LINE_CHARS);
        assert!(summary.ends_with('…'));
    }
}
