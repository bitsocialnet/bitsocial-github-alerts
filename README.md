# bitsocial-github-alerts

A Telegram bot that keeps a chat, group, or forum topic up to date with GitHub activity: it posts releases and outside contributions as they happen, and sums up everything else in a daily digest. Run by the [Bitsocial](https://github.com/bitsocialnet) team for its repositories, but anyone can self-host it.

This is a fork of [mhkafadar/notifine](https://github.com/mhkafadar/notifine), stripped down to GitHub + Telegram only, with compact message formatting and added support for GitHub release notifications. All credit for the original architecture goes to the notifine authors. This repository carries no license of its own because upstream notifine has none; licensing follows upstream.

## Features

- **Releases** — a one-liner when a minor or major release is published (`x.y.0`, any tag prefix such as `v` or `challenge-v`), with tag link, release name, pre-release marker, and the first line of prose in the release notes (headings, lists, and changelog headers are skipped). Patch releases appear only in the digest
- **Outside contributions** — issues opened, and pull requests opened or merged, by anyone who is not a bot or listed in `CORE_GITHUB_LOGINS`
- **Daily digest** — at 18:00 UTC (or every `DIGEST_INTERVAL_HOURS`), one message per chat with commits that landed on default branches, active repositories, contributors, releases, issues opened and closed, and pull requests merged, plus the release list. Nothing is posted when there was no activity. Activity is stored in Postgres for 30 days, and each chat's last digest time is stored, so a restart neither repeats a digest nor loses a missed one
- **Quiet by default** — pushes, branches, tags, and core maintainers' and bots' issues and pull requests are never posted on their own; they are only counted in the digest
- **Dependabot suppression** — events from Dependabot and activity on `dependabot/*` branches are acknowledged without posting or counting them
- **Explicit suppression** — include `[skip github-alerts]` in a commit message to leave that push out, or in an issue or pull request body (an HTML comment works) to leave out every event about that issue or pull request, from both alerts and the digest. Release notes are not checked, because generated notes quote commit messages
- **Supported events**: push (digest only), release, issues, pull requests, comments (issue/PR review/commit), check runs, workflow runs, wiki edits, ping
- **Branch filtering** — `?branch=` / `?exclude_branch=` glob patterns on the webhook URL
- **Authenticated deliveries** — verifies HMAC-SHA256 signatures and accepts only the GitHub organization IDs of `bitsocialnet`, `bitsocialforge`, and `pkcprotocol`
- **Public repositories only** — private repositories and missing or inconsistent visibility are acknowledged without payload logging, storage, or Telegram notifications
- **Forum topics** — run `/start` inside a Telegram topic to receive notifications there
- Built with Rust (actix-web + teloxide + diesel/Postgres)

## Usage

Organization webhooks cover current and future public repositories in `bitsocialnet`, `bitsocialforge`, and `pkcprotocol`. New repositories need no individual webhook or secret setup. Private repositories are excluded from notifications.

To configure an organization's Telegram destination (organization owners only):

1. Open the destination chat or topic with the bot (or add it to a group) and send `/start`.
2. The bot replies with that chat's webhook URL of the form `https://github.bitsocial.net/github/<token>`; sending `/start` alone does not change the organization's destination.
3. In the organization's **Settings → Webhooks**, configure one webhook with the URL, content type `application/json`, and the desired events. The hosted setup subscribes to pushes, issues, pull requests, and releases; pushes only feed the digest.
4. Set **Secret** to the server's `GITHUB_WEBHOOK_SECRET`, supplied privately by the operator. Keep SSL verification enabled. Never put the secret in the URL or a repository.
5. Verify a successful organization-hook delivery, then disable repository hooks pointing to the same destination to prevent duplicate alerts.

Both JSON and form-encoded deliveries are supported. The server validates the signature over the original request bytes before parsing, filtering, or sending notifications. Missing or invalid signatures return `401`; owners outside the allowlist return `403`. The allowlist uses immutable organization IDs, so another account cannot gain access by taking an old organization name. Organization-hook pings are also supported. Self-hosters serving other organizations must change the IDs in `src/webhooks/github/security.rs`.

GitHub sends organization webhooks for private repositories too. After authentication and owner checks, the bot returns `200` without processing or logging their contents. Repository events require `private: false`; if `visibility` is present, it must be `public`. Missing or inconsistent visibility is ignored. Organization pings without a repository remain supported.

### Branch filtering

```
# Only main
https://github.bitsocial.net/github/<token>?branch=main

# Multiple branches and wildcards
https://github.bitsocial.net/github/<token>?branch=main,release/*

# Exclude noisy branches (exclusions take precedence)
https://github.bitsocial.net/github/<token>?exclude_branch=feature/*,dependabot/*
```

Applies to pull request and workflow run events.

## Self-hosting

### Environment variables

| Variable | Required | Description |
| --- | --- | --- |
| `DATABASE_URL` | yes | Postgres connection string |
| `WEBHOOK_BASE_URL` | yes | Public base URL GitHub uses to reach the server, e.g. `https://github.bitsocial.net` |
| `GITHUB_TELOXIDE_TOKEN` | yes | Telegram bot token from [@BotFather](https://t.me/BotFather) |
| `GITHUB_WEBHOOK_SECRET` | yes | Random signing secret of at least 32 bytes, identical to the Secret setting on every connected GitHub webhook |
| `PORT` | no | HTTP listen port (default `8080`) |
| `CORE_GITHUB_LOGINS` | no | Comma-separated GitHub logins of core maintainers, e.g. `tomcasaburi,Rinse12`. Their issues and pull requests are only counted in the digest. Unset means every person's are posted |
| `DIGEST_INTERVAL_HOURS` | no | Hours between digests, counted from 18:00 UTC: a divisor of 24 (default `24`), or `0` to turn the digest off |
| `ADMIN_LOGS` | no | `ACTIVE` to send admin logs to Telegram (default `NOT_ACTIVE`) |
| `TELEGRAM_ADMIN_CHAT_ID` | no | Chat id that receives admin logs |
| `ADMIN_LOG_LEVEL` | no | 0–255 verbosity threshold for admin logs (default `50`) |

Database migrations are embedded and run automatically at startup.

Generate the webhook secret with `openssl rand -hex 32` and store it privately. The app refuses to start with a missing or short secret. For an existing deployment, configure the secret on every connected GitHub webhook and in the server environment before deploying signature enforcement. New webhooks also require this secret; copying the URL alone is insufficient. To rotate it, coordinate updates to the server and all connected hooks.

### Docker Compose (production)

A production compose file is provided in [`deploy/docker-compose.yml`](deploy/docker-compose.yml). It runs the prebuilt image `ghcr.io/bitsocialnet/bitsocial-github-alerts:latest` next to a Postgres 17 container and binds the app to `127.0.0.1:8090` (put a reverse proxy such as Caddy or nginx in front).

```bash
mkdir bitsocial-github-alerts && cd bitsocial-github-alerts
curl -fsSLO https://raw.githubusercontent.com/bitsocialnet/bitsocial-github-alerts/master/deploy/docker-compose.yml
cat > .env <<'ENV'
GITHUB_TELOXIDE_TOKEN=<bot token>
GITHUB_WEBHOOK_SECRET=<random webhook secret>
WEBHOOK_BASE_URL=https://github.example.com
DATABASE_PASSWORD=<random password>
ENV
docker compose up -d
```

`DATABASE_URL` is derived from `DATABASE_PASSWORD` inside the compose file; the other variables come from `.env`.

### Local development

```bash
cp .env.example .env   # fill in values
docker compose up -d bitsocial-github-alerts-db
cargo run
```

## Docker image

Images are published to GHCR on every push to `master` and on version tags:

```
ghcr.io/bitsocialnet/bitsocial-github-alerts:latest
```

## Credits

Forked from [mhkafadar/notifine](https://github.com/mhkafadar/notifine). Report bugs for this fork by [creating an issue](https://github.com/bitsocialnet/bitsocial-github-alerts/issues/new).
