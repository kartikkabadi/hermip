# Architecture

**What belongs here:** How the Hermip system works — components, relationships, data flows, invariants.
**What does NOT belong here:** Service ports/commands (use services.yaml), env vars (use environment.md).

---

## System Overview

Hermip is a daemon-first event-to-channel notification router. It prevents context pollution in AI agent sessions by routing operational notifications (git commits, GitHub events, tmux alerts, Hermes Agent hooks, Droid mission events) to dedicated Discord/Slack channels.

## Component Architecture

```
Event Sources → mpsc queue → Dispatcher → Router → Renderer → Sink
   (git,                                           (compact,   (Discord REST,
    github,                                         alert,       Discord Webhook,
    tmux,                                           inline,      Slack Webhook)
    hermes,                                         raw)
    droid)
```

### HTTP API (Daemon Endpoints)

| Endpoint | Method | Purpose |
|---|---|---|
| `/health` | GET | Health check (returns `{"status": "ok"}`) |
| `/api/status` | GET | Detailed health with version, config info |
| `/api/event` | POST | Canonical event ingestion (requires `Content-Type: application/json`) |
| `/event` | POST | Legacy alias for `/api/event` |
| `/events` | POST | Legacy alias for `/api/event` |
| `/api/native/hook` | POST | Provider-specific native hook endpoint |
| `/native/hook` | POST | Legacy alias for `/api/native/hook` |

Non-JSON content-type returns 415 Unsupported Media Type. Invalid JSON returns 400 Bad Request. Unknown routes return 404.

### Sources (Event Producers)

Each source implements the `Source` trait. Sources produce typed events and push them into the shared Tokio mpsc channel.

- **GitSource** — Watches a local git repository for commits and branch changes
- **GitHubSource** — Receives GitHub webhook payloads (issues, PRs, CI)
- **TmuxSource** — Monitors tmux sessions for keyword matches and stale panes
- **HermesSource** — Receives events from Hermes Agent plugin hooks (`hermes.session.started`, `hermes.tool.called`, etc.)
- **DroidSource** — Receives Droid Mission events (`mission.started`, `feature.completed`, `validation.passed`, etc.)

### Dispatcher (Queue Consumer)

Consumes events from the mpsc channel. For each event:
1. Passes to the Router to resolve deliveries
2. For each delivery, applies the appropriate Renderer
3. Passes rendered output to the Sink for delivery

Best-effort multi-delivery: one failed delivery does not block others.

### Router (Rule Engine)

Resolves 0..N deliveries per event based on routing rules. Rules match on:
- `event_type` (e.g., `mission.started`)
- `source` (e.g., `droid`, `hermes`, `git`)
- `metadata` fields (e.g., `milestone_name`, `feature_id`)

AND-logic: all conditions must match. Matching does not stop at first rule.

Dynamic template tokens in routing targets: `{repo}`, `{number}`, `{sh:...}`, `{env:...}`

### Renderers (Formatters)

Four render styles:
- **Compact** — Single-line summary
- **Alert** — Highlighted/urgent notification
- **Inline** — Context-friendly brief format
- **Raw** — Passthrough JSON

### Sinks (Delivery Transports)

Each sink implements the `Sink` trait:
- **Discord REST** — Bot token + channel ID delivery with rate limit handling
- **Discord Webhook** — URL-based delivery
- **Slack Webhook** — URL-based delivery

**Discord REST and Discord Webhook** sinks include:
- Rate limit handling (429 → Retry-After → backoff, up to 3 retries)
- Circuit breaker (sustained failures → cool-down → probe → resume)
- Dead Letter Queue (DLQ) for exhausted retries
- Best-effort independent delivery (one sink failure doesn't affect others)

**Slack Webhook** sink includes equivalent resilience matching Discord sink behavior:
- Rate limit handling (429 → Retry-After header → backoff, up to 3 retries)
- Exponential backoff with jitter for non-429 server errors
- Circuit breaker (sustained failures → cool-down → probe → resume)
- Dead Letter Queue (DLQ) for exhausted retries

The DLQ is in-memory only — entries are not persisted across daemon restarts.

## Hermes Agent Integration

### Plugin Structure

```
~/.hermes/plugins/hermip/
├── plugin.yaml      # Manifest: name, version, provides_hooks, provides_tools
├── __init__.py      # register(ctx) function
├── hooks.py         # Lifecycle hook handlers
└── tools.py         # Tool implementations
```

### Hook Event Mapping

| Hermes Hook | Hermip Event Type |
|---|---|
| on_session_start | hermes.session.started |
| on_session_end | hermes.session.ended |
| pre_tool_call | hermes.tool.called |
| post_tool_call | hermes.tool.returned |
| pre_llm_call | hermes.llm.call |
| post_llm_call | hermes.llm.response |

### Skill Structure

```
skills/hermip/SKILL.md   # YAML frontmatter + Markdown instructions
```

## Droid Mission Integration

### Event Types

| Event Type | Required Metadata |
|---|---|
| mission.started | milestone_name, mission_id, status |
| mission.completed | milestone_name, mission_id, status |
| mission.failed | milestone_name, mission_id, status |
| feature.started | feature_id, milestone_name, status |
| feature.completed | feature_id, milestone_name, status |
| feature.failed | feature_id, milestone_name, status |
| validation.passed | feature_id, status |
| validation.failed | feature_id, status |
| worker.started | worker_id, status |
| worker.completed | worker_id, status |
| discovery.issues | issues (list) |

## Configuration Hierarchy (highest to lowest precedence)

1. Environment variables (`HERMIP_*`; deprecated `CLAWHIP_*` as fallback)
2. CLI flags (`--config`, `--port`)
3. Local config (`./hermip.toml`)
4. Global config (`~/.config/hermip/hermip.toml`)
5. Built-in defaults

Legacy ClawHip `[discord]` format is transparently mapped to new Hermip config structure.

CLAWHIP_* environment variables are deprecated but functional: when both `HERMIP_*` and `CLAWHIP_*` are set, `HERMIP_*` wins. When only `CLAWHIP_*` is set, it works as a fallback with a deprecation warning printed to stderr.

### Default Webhook URLs

The `[defaults]` section supports `webhook_discord` and `webhook_slack` fields, which provide default webhook URLs for routes that don't specify their own. These are overridden by `HERMIP_DISCORD_WEBHOOK_URL` and `HERMIP_SLACK_WEBHOOK_URL` env vars respectively.

The router resolves webhook targets with this precedence:
1. Route-level `webhook` or `slack_webhook` field
2. `defaults.webhook_discord` / `defaults.webhook_slack` (from env var or TOML)
3. Error if no webhook configured

## Design Decisions

### Unknown source event handling: permissive normalization (not rejection)

When an incoming event has a kind that is not recognized by the typed event model (`EventBody`), the system **normalizes it to `EventBody::Custom`** rather than rejecting it with an error. This is an intentional design decision.

**Rationale:**

- Hermip is an agent-first, extensible event gateway. Custom events are a first-class concept — `IncomingEvent::custom()` is a core constructor, and `EventBody::Custom` is a variant in the typed event model alongside all built-in variants.
- Rejecting unknown sources would break backward compatibility for any external tool, script, or plugin that emits events with custom kinds (e.g., `plugin.custom`, `deploy.completed`, `monitor.alert`).
- The router supports wildcard matching (`event = "*"`) and custom event routing, which would be impossible if unknown kinds were rejected at ingress.
- The `hermip explain` command explicitly works with partial or unknown payloads — rejection would make debugging route rules harder.
- Typed event kinds are an internal normalization layer, not an ingress validation gate. The system validates structure (required fields for known kinds) but does not gate on kind identity.

**Valid event kind prefixes (recognized by the typed model):**

- `git.*` — git commit and branch-change events
- `github.*` — GitHub issue, PR, CI, and release events
- `tmux.*` — tmux keyword and stale-session events
- `agent.*` / `session.*` — agent lifecycle events (started, blocked, finished, failed, etc.)
- `workspace.*` — workspace session, turn, skill, and metrics events
- `custom` — user-defined events with arbitrary payloads

Any event kind that does not match a known prefix is normalized into `EventBody::Custom` with the original kind preserved in `body.kind`. This ensures the event remains routable and renderable while signaling to downstream consumers that it is not a typed built-in event.

**Testing:** The test `keeps_unknown_events_as_custom` in `src/event/compat.rs` validates this behavior explicitly.

## Key Invariants

- Events are never dropped intentionally — only on sink delivery failure after circuit breaker exhaustion
- Multi-delivery is independent — one sink failure does not affect others
- Route matching is AND-logic — all conditions must match
- Config backward compatibility is guaranteed for ClawHip format
- The daemon must remain stable under invalid inputs (no crashes from bad events)
- Unknown event kinds are normalized to `EventBody::Custom` (permissive, not rejected)

## Feature Flags

Hermip uses compile-time feature flags to gate optional provider integrations:

| Feature | Description | Default |
|---|---|---|
| `hermes` | Hermes Agent hook provider (always available) | Yes |
| `codex-hook` | Codex (OpenAI) hook provider | Yes |
| `claude-hook` | Claude Code hook provider | Yes |

**Default build** (`cargo build --release`): All three providers are enabled. This maintains full backward compatibility.

**Hermes-only build** (`cargo build --release --no-default-features --features hermes`): Only the Hermes provider is available. The `deliver` subcommand and `prompt_deliver` module are excluded. The `--provider` flag only accepts `hermes`.

**Key gating rules:**
- `HookProvider::Codex` variant is gated with `#[cfg(feature = "codex-hook")]`
- `HookProvider::ClaudeCode` variant is gated with `#[cfg(feature = "claude-hook")]`
- `CODEX_HOOKS_FILE` and `CLAUDE_SETTINGS_FILE` constants are gated with their respective features
- The `prompt_deliver` module is gated with `#[cfg(any(feature = "codex-hook", feature = "claude-hook"))]`
- The `Deliver` CLI subcommand is gated with `#[cfg(any(feature = "codex-hook", feature = "claude-hook"))]`
- Tests that reference gated variants use `#[cfg(feature = "...")]` attributes
