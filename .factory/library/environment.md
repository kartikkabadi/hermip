# Environment

**What belongs here:** Env vars, external dependencies, setup notes.
**What does NOT belong here:** Service ports/commands (use services.yaml).

---

## Required Environment Variables

- `HERMIP_DISCORD_WEBHOOK_URL` — Discord webhook URL for notification delivery (or set in hermip.toml)
- `HERMIP_SLACK_WEBHOOK_URL` — Slack webhook URL for notification delivery (or set in hermip.toml)
- `HERMIP_DISCORD_BOT_TOKEN` — Discord bot token for REST API delivery (alternative to webhook)
- `HERMIP_DAEMON_PORT` — Override daemon port (default: 25294)

## External Dependencies

- **Rust toolchain** — Stable channel, edition 2024 (rustc 1.94.1+)
- **Tokio** — Async runtime (version matching ClawHip)
- **Axum** — HTTP framework for daemon API
- **clap** — CLI argument parsing
- **serde/serde_json** — Serialization
- **reqwest** — HTTP client for Discord/Slack delivery

## Setup Notes

1. Build with `cargo build --release` — binary at `./target/release/hermip`
2. Default config path: `./hermip.toml` (local) or `~/.config/hermip/hermip.toml` (global)
3. Default daemon port: 25294
4. For Hermes Agent integration, files go to `~/.hermes/plugins/hermip/`
5. For Droid Mission Mode, `.factory/` directory contains skill definitions and services.yaml

## Platform

- macOS (primary development)
- Linux x86_64 and ARM64 (target)
- No Windows support planned
