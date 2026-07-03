# bitsocial-github-alerts

A Telegram bot that posts compact GitHub notifications — pushes, releases, issues, pull requests, and more — to any chat, group, or forum topic. Run by the [Bitsocial](https://github.com/bitsocialnet) team for its repositories, but anyone can self-host it.

This is a fork of [mhkafadar/notifine](https://github.com/mhkafadar/notifine), stripped down to GitHub + Telegram only, with compact message formatting and added support for GitHub release notifications. All credit for the original architecture goes to the notifine authors. This repository carries no license of its own because upstream notifine has none; licensing follows upstream.

## Features

- **Compact messages** — pushes show at most 5 commits (first line of each commit message, truncated to 72 chars) plus an "… and N more" line
- **Release notifications** — one-liner when a release is published, with tag link, release name, pre-release marker, and the first line of the release notes
- **Supported events**: push (incl. branch create/delete and force-push), release, issues, pull requests, comments (issue/PR review/commit), check runs, workflow runs, wiki edits, ping
- **Branch filtering** — `?branch=` / `?exclude_branch=` glob patterns on the webhook URL
- **Forum topics** — run `/start` inside a Telegram topic to receive notifications there
- Built with Rust (actix-web + teloxide + diesel/Postgres)

## Usage

1. Open a chat with the bot (or add it to a group) and send `/start`
2. The bot replies with a webhook URL of the form `https://github.bitsocial.net/github/<token>`
3. In your GitHub repo, open **Settings → Webhooks → Add webhook**, paste the URL, set content type to `application/json`, and pick the events you want

### Branch filtering

```
# Only main
https://github.bitsocial.net/github/<token>?branch=main

# Multiple branches and wildcards
https://github.bitsocial.net/github/<token>?branch=main,release/*

# Exclude noisy branches (exclusions take precedence)
https://github.bitsocial.net/github/<token>?exclude_branch=feature/*,dependabot/*
```

Applies to push, pull request, workflow run, and create/delete events.

## Self-hosting

### Environment variables

| Variable | Required | Description |
| --- | --- | --- |
| `DATABASE_URL` | yes | Postgres connection string |
| `WEBHOOK_BASE_URL` | yes | Public base URL GitHub uses to reach the server, e.g. `https://github.bitsocial.net` |
| `GITHUB_TELOXIDE_TOKEN` | yes | Telegram bot token from [@BotFather](https://t.me/BotFather) |
| `PORT` | no | HTTP listen port (default `8080`) |
| `ADMIN_LOGS` | no | `ACTIVE` to send admin logs to Telegram (default `NOT_ACTIVE`) |
| `TELEGRAM_ADMIN_CHAT_ID` | no | Chat id that receives admin logs |
| `ADMIN_LOG_LEVEL` | no | 0–255 verbosity threshold for admin logs (default `50`) |

Database migrations are embedded and run automatically at startup.

### Docker Compose (production)

A production compose file is provided in [`deploy/docker-compose.yml`](deploy/docker-compose.yml). It runs the prebuilt image `ghcr.io/bitsocialnet/bitsocial-github-alerts:latest` next to a Postgres 17 container and binds the app to `127.0.0.1:8090` (put a reverse proxy such as Caddy or nginx in front).

```bash
mkdir bitsocial-github-alerts && cd bitsocial-github-alerts
curl -fsSLO https://raw.githubusercontent.com/bitsocialnet/bitsocial-github-alerts/main/deploy/docker-compose.yml
cat > .env <<'ENV'
GITHUB_TELOXIDE_TOKEN=<bot token>
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

Images are published to GHCR on every push to `main` and on version tags:

```
ghcr.io/bitsocialnet/bitsocial-github-alerts:latest
```

## Credits

Forked from [mhkafadar/notifine](https://github.com/mhkafadar/notifine). Report bugs for this fork by [creating an issue](https://github.com/bitsocialnet/bitsocial-github-alerts/issues/new).
