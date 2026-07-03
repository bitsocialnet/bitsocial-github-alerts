use super::utils::parse_webhook_payload;
use actix_web::web;
use html_escape::encode_text;
use serde::Deserialize;

const MAX_BODY_CHARS: usize = 200;

#[derive(Debug, Deserialize)]
pub struct ReleaseEvent {
    action: String,
    release: Release,
    repository: Repository,
}

#[derive(Debug, Deserialize)]
struct Release {
    tag_name: String,
    name: Option<String>,
    html_url: String,
    prerelease: bool,
    body: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Repository {
    full_name: String,
}

pub fn handle_release_event(body: &web::Bytes) -> String {
    let release_event: ReleaseEvent = match parse_webhook_payload(body) {
        Ok(event) => event,
        Err(e) => {
            tracing::error!("Failed to parse release event: {}", e);
            tracing::error!("Raw payload: {}", String::from_utf8_lossy(body));
            return String::new();
        }
    };

    // GitHub sends "published" for both releases and pre-releases.
    // Ignore created/edited/deleted/prereleased/released to avoid duplicates.
    if release_event.action != "published" {
        return String::new();
    }

    let release = &release_event.release;
    let repo_name = encode_text(&release_event.repository.full_name);
    let tag_name = encode_text(&release.tag_name);
    let html_url = &release.html_url;

    let mut message =
        format!("🚀 <b>{repo_name}</b> — new release <a href=\"{html_url}\">{tag_name}</a>");

    // Append the release name when it adds information beyond the tag
    if let Some(name) = release.name.as_deref() {
        let name = name.trim();
        if !name.is_empty() && name != release.tag_name {
            message.push_str(&format!(" ({})", encode_text(name)));
        }
    }

    if release.prerelease {
        message.push_str(" (pre-release)");
    }

    if let Some(body) = release.body.as_deref() {
        let summary = summarize_body(body);
        if !summary.is_empty() {
            message.push('\n');
            message.push_str(&encode_text(&summary));
        }
    }

    message.push('\n');
    message
}

/// First line of the release body, truncated to MAX_BODY_CHARS characters.
fn summarize_body(body: &str) -> String {
    let first_line = body.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
    let first_line = first_line.trim();

    if first_line.chars().count() > MAX_BODY_CHARS {
        let truncated: String = first_line.chars().take(MAX_BODY_CHARS).collect();
        format!("{}…", truncated.trim_end())
    } else {
        first_line.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use actix_web::web::Bytes;
    use serde_json::json;

    fn payload(action: &str, prerelease: bool, name: Option<&str>, body: Option<&str>) -> Bytes {
        Bytes::from(
            json!({
                "action": action,
                "release": {
                    "tag_name": "v1.2.3",
                    "name": name,
                    "html_url": "https://github.com/bitsocialnet/example/releases/tag/v1.2.3",
                    "prerelease": prerelease,
                    "body": body,
                },
                "repository": {
                    "full_name": "bitsocialnet/example"
                },
                "sender": {
                    "login": "octocat"
                }
            })
            .to_string(),
        )
    }

    #[test]
    fn test_published_release() {
        let message = handle_release_event(&payload("published", false, Some("v1.2.3"), None));
        assert_eq!(
            message,
            "🚀 <b>bitsocialnet/example</b> — new release \
             <a href=\"https://github.com/bitsocialnet/example/releases/tag/v1.2.3\">v1.2.3</a>\n"
        );
    }

    #[test]
    fn test_release_with_distinct_name() {
        let message = handle_release_event(&payload("published", false, Some("Big Update"), None));
        assert!(message.contains("v1.2.3</a> (Big Update)"));
    }

    #[test]
    fn test_prerelease_suffix() {
        let message = handle_release_event(&payload("published", true, None, None));
        assert!(message.contains("(pre-release)"));
    }

    #[test]
    fn test_body_first_line_included() {
        let message = handle_release_event(&payload(
            "published",
            false,
            None,
            Some("Fixes a crash on startup.\n\nMore details below."),
        ));
        assert!(message.ends_with("Fixes a crash on startup.\n"));
        assert!(!message.contains("More details below"));
    }

    #[test]
    fn test_long_body_truncated() {
        let long_body = "a".repeat(500);
        let message = handle_release_event(&payload("published", false, None, Some(&long_body)));
        assert!(message.contains('…'));
        let body_line = message.lines().nth(1).unwrap();
        assert!(body_line.chars().count() <= MAX_BODY_CHARS + 1);
    }

    #[test]
    fn test_non_published_actions_ignored() {
        for action in ["created", "edited", "deleted", "prereleased", "released"] {
            assert_eq!(
                handle_release_event(&payload(action, false, None, None)),
                ""
            );
        }
    }

    #[test]
    fn test_html_escaping() {
        let message = handle_release_event(&payload(
            "published",
            false,
            Some("<b>bold</b> & co"),
            Some("<script>alert(1)</script>"),
        ));
        assert!(message.contains("&lt;b&gt;bold&lt;/b&gt; &amp; co"));
        assert!(message.contains("&lt;script&gt;"));
        assert!(!message.contains("<script>"));
    }
}
