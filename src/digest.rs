//! Periodic digest of GitHub activity per chat. Digests go out at 18:00 UTC
//! and every `DIGEST_INTERVAL_HOURS` after it, each covering the activity
//! recorded since that chat's previous digest. Nothing is posted when there was
//! no activity. The time of the last digest is stored per chat, so a restart
//! neither repeats a digest nor loses the activity of a missed one.

use crate::bots::bot_service::{BotConfig, BotService};
use crate::webhooks::common::{deliver_message, Delivery, DeliveryContext};
use bitsocial_github_alerts::db::DbPool;
use bitsocial_github_alerts::models::{digest_kind, Chat, DigestEvent};
use bitsocial_github_alerts::{
    delete_digest_events_before, find_active_chats, find_digest_events, set_chat_last_digest_at,
};
use chrono::{DateTime, Datelike, Duration, TimeZone, Utc};
use html_escape::{encode_double_quoted_attribute, encode_text};
use std::collections::HashMap;

const DIGEST_ANCHOR_HOUR_UTC: u32 = 18;
const RETENTION_DAYS: i64 = 30;
const RETRY_AFTER_FAILURE: Duration = Duration::minutes(10);
/// Attempts per digest time before a failing chat's digest is dropped, so a
/// permanent Telegram error (a deleted topic) does not retry forever.
const MAX_ATTEMPTS: u32 = 3;
/// Read a window this long after it closes, so rows stamped just before the
/// digest time have committed.
const READ_DELAY: Duration = Duration::seconds(30);

const MAX_REPOS_LISTED: usize = 8;
const MAX_CONTRIBUTORS_LISTED: usize = 10;
const MAX_RELEASES_LISTED: usize = 20;

pub async fn run_digest_scheduler(pool: DbPool, token: String, interval_hours: u32) {
    if interval_hours == 0 {
        tracing::info!("Digest disabled (DIGEST_INTERVAL_HOURS=0)");
        // Activity is still recorded; keep deleting what is past retention.
        loop {
            delete_expired_activity(&pool);
            tokio::time::sleep(std::time::Duration::from_secs(24 * 60 * 60)).await;
        }
    }
    let interval = Duration::hours(i64::from(interval_hours));
    let bot = BotService::new(
        BotConfig {
            bot_name: "Github".to_string(),
            token,
            webhook_base_url: String::new(),
            admin_chat_id: None,
        },
        pool.clone(),
    );
    tracing::info!(
        "Digest every {} hour(s) from {:02}:00 UTC",
        interval_hours,
        DIGEST_ANCHOR_HOUR_UTC
    );

    let mut attempts: (DateTime<Utc>, u32) = (DateTime::<Utc>::MIN_UTC, 0);
    loop {
        let due = latest_digest_time(Utc::now() - READ_DELAY, interval);
        attempts = if attempts.0 == due {
            (due, attempts.1 + 1)
        } else {
            (due, 1)
        };
        let give_up = attempts.1 >= MAX_ATTEMPTS;
        let all_sent = send_due_digests(&pool, &bot, due, interval, give_up).await;
        delete_expired_activity(&pool);

        let next_due = due + interval;
        let wake_at = if all_sent {
            next_due
        } else {
            next_due.min(Utc::now() + RETRY_AFTER_FAILURE)
        };
        let wait = (wake_at + READ_DELAY - Utc::now())
            .to_std()
            .unwrap_or(std::time::Duration::from_secs(1));
        tokio::time::sleep(wait).await;
    }
}

fn delete_expired_activity(pool: &DbPool) {
    if let Err(e) = delete_digest_events_before(pool, Utc::now() - Duration::days(RETENTION_DAYS)) {
        tracing::error!("Failed to delete old digest activity: {}", e);
    }
}

/// The most recent digest time at or before `now`: 18:00 UTC plus a whole
/// number of intervals.
pub fn latest_digest_time(now: DateTime<Utc>, interval: Duration) -> DateTime<Utc> {
    let anchor = Utc
        .with_ymd_and_hms(
            now.year(),
            now.month(),
            now.day(),
            DIGEST_ANCHOR_HOUR_UTC,
            0,
            0,
        )
        .single()
        .expect("UTC has no ambiguous times");
    // Whole seconds rounded down: `TimeDelta::num_seconds` rounds toward zero,
    // which would put a `now` just before the anchor at the anchor.
    let elapsed = now.timestamp() - anchor.timestamp();
    let intervals = elapsed.div_euclid(interval.num_seconds());
    anchor + Duration::seconds(interval.num_seconds() * intervals)
}

/// Send each active chat its digest for the activity up to `due`, unless it
/// already got it. Returns false when a send failed and should be retried.
/// With `give_up`, a failed digest is dropped instead.
async fn send_due_digests(
    pool: &DbPool,
    bot: &BotService,
    due: DateTime<Utc>,
    interval: Duration,
    give_up: bool,
) -> bool {
    let chats = match find_active_chats(pool) {
        Ok(chats) => chats,
        Err(e) => {
            tracing::error!("Failed to load chats for the digest: {}", e);
            return false;
        }
    };

    let mut all_sent = true;
    for chat in chats.iter().filter(|chat| is_due(chat, due)) {
        let start = match chat.last_digest_at {
            Some(last) => last,
            None => {
                // Store the first window's start, so failed sends that run past
                // the next digest time still cover it.
                let start = due - interval;
                if let Err(e) = set_chat_last_digest_at(pool, chat.id, start) {
                    tracing::error!("Failed to store digest time for chat {}: {}", chat.id, e);
                }
                start
            }
        };
        let events = match find_digest_events(pool, chat.id, start, due) {
            Ok(events) => events,
            Err(e) => {
                tracing::error!("Failed to load digest activity for chat {}: {}", chat.id, e);
                all_sent = false;
                continue;
            }
        };

        if let Some(message) = format_digest(&events, start, due, interval) {
            tracing::info!(
                "Sending digest of {} item(s) to chat {}",
                events.len(),
                chat.id
            );
            let delivery = deliver_message(DeliveryContext {
                pool,
                bot,
                chat,
                message,
                link_preview: false,
                source: "github",
                description: "digest",
            })
            .await;
            if delivery == Delivery::Failed && !give_up {
                // Keep last_digest_at so the retry covers this activity too.
                all_sent = false;
                continue;
            }
            if delivery == Delivery::Failed {
                tracing::error!(
                    "Dropping the digest up to {} for chat {} after {} failed attempts",
                    due,
                    chat.id,
                    MAX_ATTEMPTS
                );
            }
        }

        if let Err(e) = set_chat_last_digest_at(pool, chat.id, due) {
            tracing::error!("Failed to store digest time for chat {}: {}", chat.id, e);
            all_sent = false;
        }
    }
    all_sent
}

fn is_due(chat: &Chat, due: DateTime<Utc>) -> bool {
    chat.last_digest_at.is_none_or(|last| last < due)
}

/// The digest message for activity recorded after `start` and up to `end`,
/// or `None` when there was none.
pub fn format_digest(
    events: &[DigestEvent],
    start: DateTime<Utc>,
    end: DateTime<Utc>,
    interval: Duration,
) -> Option<String> {
    if events.is_empty() {
        return None;
    }

    let count = |kind: &str| events.iter().filter(|e| e.kind == kind).count();
    let releases: Vec<&DigestEvent> = events
        .iter()
        .filter(|e| e.kind == digest_kind::RELEASE)
        .collect();

    let title = if end - start == interval && interval == Duration::days(1) {
        "Daily development update".to_string()
    } else if end - start == interval {
        format!("Development update, last {} hours", interval.num_hours())
    } else {
        format!(
            "Development update since {} UTC",
            start.format("%b %-d, %H:%M")
        )
    };
    let mut message = format!("📊 <b>{title}</b>\n");

    let totals: Vec<String> = [
        (count(digest_kind::COMMIT), "commit", "commits"),
        (releases.len(), "release", "releases"),
        (
            count(digest_kind::ISSUE_OPENED),
            "issue opened",
            "issues opened",
        ),
        (
            count(digest_kind::ISSUE_CLOSED),
            "issue closed",
            "issues closed",
        ),
        (
            count(digest_kind::PR_MERGED),
            "pull request merged",
            "pull requests merged",
        ),
    ]
    .into_iter()
    .filter(|(n, _, _)| *n > 0)
    .map(|(n, one, many)| format!("{n} {}", if n == 1 { one } else { many }))
    .collect();
    message.push_str(&totals.join(" · "));
    message.push('\n');

    let repos: Vec<&str> = ranked(events.iter().map(|e| e.repo.as_str()));
    let repo_names: Vec<&str> = repos
        .iter()
        .map(|repo| display_repo_name(repo, &repos))
        .collect();
    message.push_str(&format!(
        "Active repos ({}): {}\n",
        repos.len(),
        listed(&repo_names, MAX_REPOS_LISTED)
    ));

    let contributors = ranked(events.iter().filter_map(|e| e.actor.as_deref()));
    if !contributors.is_empty() {
        message.push_str(&format!(
            "Contributors ({}): {}\n",
            contributors.len(),
            listed(&contributors, MAX_CONTRIBUTORS_LISTED)
        ));
    }

    if !releases.is_empty() {
        message.push_str("\nReleases:\n");
        for release in releases.iter().take(MAX_RELEASES_LISTED) {
            let label = encode_text(&format!(
                "{} {}",
                display_repo_name(&release.repo, &repos),
                release.item
            ))
            .into_owned();
            match &release.url {
                Some(url) => message.push_str(&format!(
                    "🚀 <a href=\"{}\">{label}</a>\n",
                    encode_double_quoted_attribute(url)
                )),
                None => message.push_str(&format!("🚀 {label}\n")),
            }
        }
        if releases.len() > MAX_RELEASES_LISTED {
            message.push_str(&format!(
                "… and {} more\n",
                releases.len() - MAX_RELEASES_LISTED
            ));
        }
    }

    Some(message)
}

fn short_repo_name(full_name: &str) -> &str {
    full_name.rsplit('/').next().unwrap_or(full_name)
}

/// The repository name without its owner, unless another repository in the
/// digest has the same name (e.g. `bitsocialnet/assets`, `pkcprotocol/assets`).
fn display_repo_name<'a>(full_name: &'a str, all: &[&str]) -> &'a str {
    let short = short_repo_name(full_name);
    let collides = all
        .iter()
        .any(|other| *other != full_name && short_repo_name(other) == short);
    if collides {
        full_name
    } else {
        short
    }
}

/// Distinct names, most frequent first, ties by name.
fn ranked<'a>(names: impl Iterator<Item = &'a str>) -> Vec<&'a str> {
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for name in names {
        *counts.entry(name).or_default() += 1;
    }
    let mut ranked: Vec<(&str, usize)> = counts.into_iter().collect();
    ranked.sort_by(|(a, a_count), (b, b_count)| b_count.cmp(a_count).then(a.cmp(b)));
    ranked.into_iter().map(|(name, _)| name).collect()
}

fn listed(names: &[&str], max: usize) -> String {
    let shown: Vec<String> = names
        .iter()
        .take(max)
        .map(|name| encode_text(name).into_owned())
        .collect();
    let mut list = shown.join(", ");
    if names.len() > max {
        list.push_str(&format!(" and {} more", names.len() - max));
    }
    list
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(day: u32, hour: u32, minute: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 11, day, hour, minute, 0)
            .unwrap()
    }

    fn event(kind: &str, repo: &str, item: &str, actor: Option<&str>) -> DigestEvent {
        DigestEvent {
            id: 0,
            chat_id: 1,
            kind: kind.to_string(),
            repo: repo.to_string(),
            item: item.to_string(),
            actor: actor.map(str::to_string),
            url: Some(format!("https://github.com/{repo}/releases/tag/{item}")),
            created_at: at(20, 12, 0),
        }
    }

    #[test]
    fn digest_times_are_anchored_at_18_utc() {
        let day = Duration::days(1);
        assert_eq!(latest_digest_time(at(20, 18, 0), day), at(20, 18, 0));
        assert_eq!(latest_digest_time(at(20, 23, 59), day), at(20, 18, 0));
        assert_eq!(latest_digest_time(at(21, 17, 59), day), at(20, 18, 0));

        let six = Duration::hours(6);
        assert_eq!(latest_digest_time(at(21, 5, 0), six), at(21, 0, 0));
        assert_eq!(latest_digest_time(at(21, 17, 0), six), at(21, 12, 0));
        assert_eq!(latest_digest_time(at(21, 18, 30), six), at(21, 18, 0));
    }

    #[test]
    fn digest_time_is_never_in_the_future() {
        // Within the last second before a digest time, the previous one is due.
        let just_before = at(20, 17, 59) + Duration::milliseconds(59_700);
        assert_eq!(
            latest_digest_time(just_before, Duration::days(1)),
            at(19, 18, 0)
        );
        let just_before_noon = at(20, 11, 59) + Duration::milliseconds(59_700);
        assert_eq!(
            latest_digest_time(just_before_noon, Duration::hours(6)),
            at(20, 6, 0)
        );
    }

    #[test]
    fn repos_sharing_a_name_keep_their_owner() {
        let events = vec![
            event(digest_kind::COMMIT, "bitsocialnet/assets", "a", Some("x")),
            event(digest_kind::COMMIT, "pkcprotocol/assets", "b", Some("x")),
            event(digest_kind::RELEASE, "pkcprotocol/assets", "v1.0.0", None),
            event(digest_kind::COMMIT, "bitsocialnet/5chan", "c", Some("x")),
        ];
        let message =
            format_digest(&events, at(19, 18, 0), at(20, 18, 0), Duration::days(1)).unwrap();
        assert!(
            message.contains("Active repos (3): pkcprotocol/assets, 5chan, bitsocialnet/assets\n")
        );
        assert!(message.contains(">pkcprotocol/assets v1.0.0</a>"));
    }

    #[test]
    fn a_chat_is_due_until_its_digest_covers_the_digest_time() {
        let mut chat = Chat {
            id: 1,
            name: "c".into(),
            telegram_id: "-100".into(),
            webhook_url: None,
            thread_id: None,
            language: "en".into(),
            created_at: at(1, 0, 0),
            updated_at: at(1, 0, 0),
            is_active: true,
            deactivated_at: None,
            last_digest_at: None,
        };
        assert!(is_due(&chat, at(20, 18, 0)));
        chat.last_digest_at = Some(at(19, 18, 0));
        assert!(is_due(&chat, at(20, 18, 0)));
        chat.last_digest_at = Some(at(20, 18, 0));
        assert!(!is_due(&chat, at(20, 18, 0)));
    }

    #[test]
    fn no_digest_without_activity() {
        assert_eq!(
            format_digest(&[], at(19, 18, 0), at(20, 18, 0), Duration::days(1)),
            None
        );
    }

    #[test]
    fn daily_digest_lists_totals_repos_contributors_and_releases() {
        let events = vec![
            event(
                digest_kind::COMMIT,
                "bitsocialnet/5chan",
                "a1",
                Some("tomcasaburi"),
            ),
            event(
                digest_kind::COMMIT,
                "bitsocialnet/5chan",
                "a2",
                Some("tomcasaburi"),
            ),
            event(
                digest_kind::COMMIT,
                "pkcprotocol/pkc-js",
                "b1",
                Some("Rinse12"),
            ),
            event(digest_kind::RELEASE, "bitsocialnet/5chan", "v0.11.0", None),
            event(
                digest_kind::RELEASE,
                "bitsocialnet/bitsocial-cli",
                "v0.19.144",
                None,
            ),
            event(
                digest_kind::ISSUE_OPENED,
                "pkcprotocol/pkc-js",
                "557",
                Some("trackoor"),
            ),
            event(
                digest_kind::ISSUE_CLOSED,
                "pkcprotocol/pkc-js",
                "556",
                Some("Rinse12"),
            ),
            event(
                digest_kind::PR_MERGED,
                "bitsocialnet/seedit",
                "12",
                Some("trackoor"),
            ),
        ];
        let message =
            format_digest(&events, at(19, 18, 0), at(20, 18, 0), Duration::days(1)).unwrap();
        assert_eq!(
            message,
            "📊 <b>Daily development update</b>\n\
             3 commits · 2 releases · 1 issue opened · 1 issue closed · 1 pull request merged\n\
             Active repos (4): 5chan, pkc-js, bitsocial-cli, seedit\n\
             Contributors (3): Rinse12, tomcasaburi, trackoor\n\
             \n\
             Releases:\n\
             🚀 <a href=\"https://github.com/bitsocialnet/5chan/releases/tag/v0.11.0\">5chan v0.11.0</a>\n\
             🚀 <a href=\"https://github.com/bitsocialnet/bitsocial-cli/releases/tag/v0.19.144\">bitsocial-cli v0.19.144</a>\n"
        );
    }

    #[test]
    fn zero_totals_and_empty_sections_are_left_out() {
        let events = vec![event(
            digest_kind::RELEASE,
            "bitsocialnet/5chan",
            "v0.11.1",
            None,
        )];
        let message =
            format_digest(&events, at(19, 18, 0), at(20, 18, 0), Duration::days(1)).unwrap();
        assert!(message.contains("\n1 release\nActive repos (1): 5chan\n"));
        assert!(!message.contains("Contributors"));
        assert!(!message.contains("commit"));
    }

    #[test]
    fn titles_follow_the_window() {
        let events = vec![event(digest_kind::COMMIT, "o/r", "a", Some("x"))];
        let six = Duration::hours(6);
        assert!(format_digest(&events, at(20, 12, 0), at(20, 18, 0), six)
            .unwrap()
            .starts_with("📊 <b>Development update, last 6 hours</b>"));
        // A missed digest (bot down) makes the next one cover a longer window.
        assert!(
            format_digest(&events, at(18, 18, 0), at(20, 18, 0), Duration::days(1))
                .unwrap()
                .starts_with("📊 <b>Development update since Nov 18, 18:00 UTC</b>")
        );
    }

    #[test]
    fn long_lists_are_capped_and_names_escaped() {
        let mut events: Vec<DigestEvent> = (0..12)
            .map(|i| {
                event(
                    digest_kind::COMMIT,
                    &format!("o/repo{i:02}"),
                    "a",
                    Some(&format!("user{i:02}")),
                )
            })
            .collect();
        events.push(event(digest_kind::COMMIT, "o/<b>", "a", Some("<i>")));
        let message =
            format_digest(&events, at(19, 18, 0), at(20, 18, 0), Duration::days(1)).unwrap();
        assert!(message.contains("Active repos (13): &lt;b&gt;, repo00,"));
        assert!(message.contains("repo06 and 5 more\n"));
        assert!(message.contains("Contributors (13): &lt;i&gt;, user00,"));
        assert!(message.contains("user08 and 3 more\n"));
    }
}
