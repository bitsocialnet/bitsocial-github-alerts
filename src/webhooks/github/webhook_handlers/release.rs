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

    // Patch releases are listed in the digest instead of posted on their own.
    if is_patch_release(&release_event.release.tag_name) {
        tracing::info!(
            "Leaving patch release {} of {} to the digest",
            release_event.release.tag_name,
            release_event.repository.full_name
        );
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

/// True when the tag's version (the first `x.y.z` in it, so prefixes such as
/// `v` or `challenge-v` are fine) has a non-zero patch number. Tags without
/// such a version are not treated as patch releases.
pub fn is_patch_release(tag: &str) -> bool {
    tag.split(|c: char| !c.is_ascii_digit() && c != '.')
        .map(|run| run.trim_matches('.'))
        .find_map(|run| {
            let parts: Vec<&str> = run.split('.').collect();
            (parts.len() >= 3 && parts.iter().all(|part| !part.is_empty()))
                .then(|| parts[2].parse::<u64>().ok())
                .flatten()
        })
        .is_some_and(|patch| patch != 0)
}

/// First line of the first prose paragraph in the release body, truncated to
/// MAX_BODY_CHARS characters. Headings (including changelog headers such as
/// `## <small>0.19.132 (2026-11-12)</small>`), list items and their wrapped
/// lines, quotes, tables, code blocks, HTML and GitHub's "Full Changelog" link
/// are skipped; empty when the body has no prose.
fn summarize_body(body: &str) -> String {
    let lines: Vec<&str> = body.lines().map(str::trim).collect();
    let mut in_code_block = false;
    // Whether the current line starts a new block: at the start, and after a
    // blank line, heading, rule or code block. A prose-looking line that does
    // not start a block continues the one above, e.g. a wrapped list item.
    let mut starts_block = true;
    let mut first_line = "";
    for (index, line) in lines.iter().enumerate() {
        if line.starts_with("```") || line.starts_with("~~~") {
            in_code_block = !in_code_block;
            starts_block = !in_code_block;
            continue;
        }
        if in_code_block {
            continue;
        }
        if line.is_empty() {
            starts_block = true;
            continue;
        }
        // A line underlined with === or --- is a (setext) heading.
        let is_setext_heading = lines
            .get(index + 1)
            .is_some_and(|next| is_horizontal_line(next));
        if starts_block && !is_setext_heading && is_prose(line) {
            first_line = line;
            break;
        }
        starts_block = line.starts_with('#') || is_horizontal_line(line);
    }

    if first_line.chars().count() > MAX_BODY_CHARS {
        let truncated: String = first_line.chars().take(MAX_BODY_CHARS).collect();
        format!("{}…", truncated.trim_end())
    } else {
        first_line.to_string()
    }
}

/// Horizontal rules and setext heading underlines: `---`, `===`, `***`, `___`.
fn is_horizontal_line(line: &str) -> bool {
    !line.is_empty()
        && line
            .chars()
            .all(|c| matches!(c, '-' | '=' | '*' | '_' | ' '))
}

fn is_prose(line: &str) -> bool {
    const STRUCTURE_PREFIXES: [&str; 8] =
        ["#", "* ", "- ", "+ ", ">", "|", "<", "**Full Changelog**"];
    if line.is_empty()
        || STRUCTURE_PREFIXES
            .iter()
            .any(|prefix| line.starts_with(prefix))
    {
        return false;
    }
    if is_horizontal_line(line) {
        return false;
    }
    // Ordered list items: "1. " or "1) ".
    let digits = line.chars().take_while(char::is_ascii_digit).count();
    if digits > 0 && (line[digits..].starts_with(". ") || line[digits..].starts_with(") ")) {
        return false;
    }
    line.chars().any(char::is_alphabetic)
}

#[cfg(test)]
mod tests {
    use super::*;
    use actix_web::web::Bytes;
    use serde_json::json;

    fn payload_for(
        tag: &str,
        action: &str,
        prerelease: bool,
        name: Option<&str>,
        body: Option<&str>,
    ) -> Bytes {
        Bytes::from(
            json!({
                "action": action,
                "release": {
                    "tag_name": tag,
                    "name": name,
                    "html_url": format!("https://github.com/bitsocialnet/example/releases/tag/{tag}"),
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

    fn payload(action: &str, prerelease: bool, name: Option<&str>, body: Option<&str>) -> Bytes {
        payload_for("v1.2.0", action, prerelease, name, body)
    }

    fn summary_of(body: &str) -> Option<String> {
        handle_release_event(&payload("published", false, None, Some(body)))
            .lines()
            .nth(1)
            .map(str::to_string)
    }

    #[test]
    fn test_published_release() {
        let message = handle_release_event(&payload("published", false, Some("v1.2.0"), None));
        assert_eq!(
            message,
            "🚀 <b>bitsocialnet/example</b> — new release \
             <a href=\"https://github.com/bitsocialnet/example/releases/tag/v1.2.0\">v1.2.0</a>\n"
        );
    }

    #[test]
    fn test_release_with_distinct_name() {
        let message = handle_release_event(&payload("published", false, Some("Big Update"), None));
        assert!(message.contains("v1.2.0</a> (Big Update)"));
    }

    #[test]
    fn test_prerelease_suffix() {
        let message = handle_release_event(&payload("published", true, None, None));
        assert!(message.contains("(pre-release)"));
    }

    #[test]
    fn test_patch_releases_are_left_to_the_digest() {
        for tag in ["v0.19.144", "challenge-v0.1.8", "1.2.3", "v1.0.1-rc.1"] {
            assert_eq!(
                handle_release_event(&payload_for(tag, "published", false, None, None)),
                "",
                "{tag}"
            );
        }
        for tag in ["v0.11.0", "v1.0.0", "v2.0.0-rc.1", "v3", "v1.4", "nightly"] {
            assert!(
                !handle_release_event(&payload_for(tag, "published", false, None, None)).is_empty(),
                "{tag}"
            );
        }
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
    fn test_summary_skips_changelog_headers_to_the_first_prose_line() {
        assert_eq!(
            summary_of(
                "## <small>0.19.132 (2026-11-12)</small>\n\n### Features\n\n\
                 This release adds category multiboards."
            )
            .as_deref(),
            Some("This release adds category multiboards.")
        );
        assert_eq!(
            summary_of("Title\n=====\n\n---\n\n**Breaking:** nodes must upgrade.").as_deref(),
            Some("**Breaking:** nodes must upgrade.")
        );
        assert_eq!(
            summary_of("## v0.11.0\nFirst line of the notes\ncontinues here.").as_deref(),
            Some("First line of the notes")
        );
    }

    #[test]
    fn test_wrapped_list_items_are_not_prose() {
        // tipping-challenge v0.2.0: a breaking-change bullet wrapped onto a second line.
        let body = "## 0.2.0 (2026-11-20)\n\n### ⚠ BREAKING CHANGES\n\n\
                    * export only the challenge\n\
                    * a transaction without the suffix, or sent by another\n\
                    account, no longer verifies.\n\
                    * spec the parent binding for tip transactions\n";
        assert_eq!(summary_of(body), None);
        assert_eq!(
            summary_of("* a list item\n  that wraps\n\nThen a paragraph.").as_deref(),
            Some("Then a paragraph.")
        );
        assert_eq!(
            summary_of("```\nnot this\n```\nBut this.").as_deref(),
            Some("But this.")
        );
    }

    #[test]
    fn test_no_summary_without_prose() {
        // semantic-release and GitHub-generated notes: headings, lists and links only.
        for body in [
            "## <small>0.19.144 (2026-11-20)</small>\n\n\
             * chore(ci): skip Claude reviews on draft PRs ([ad7dd82](https://example.com))\n  \
             * pkc-js [0.0.153](https://example.com) (2026-11-17)",
            "## [0.0.153](https://example.com/compare) (2026-11-17)\n\n### Bug Fixes\n\n\
             * **comment-update:** sign and store extra keys",
            "## What's Changed\n- Add feature by @a in #1\n1. step one\n\n\
             **Full Changelog**: https://github.com/o/r/compare/v1...v2",
            "<!-- notes -->\n```\ncode only\n```\n> quoted\n| a | b |",
        ] {
            assert_eq!(summary_of(body), None, "{body}");
        }
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
            Some("Text with <script>alert(1)</script>"),
        ));
        assert!(message.contains("&lt;b&gt;bold&lt;/b&gt; &amp; co"));
        assert!(message.contains("&lt;script&gt;"));
        assert!(!message.contains("<script>"));
    }
}
