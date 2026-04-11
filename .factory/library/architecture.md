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

All sinks include:
- Rate limit handling (429 → Retry-After → backoff)
- Circuit breaker (sustained failures → cool-down → probe → resume)
- Best-effort independent delivery (one sink failure doesn't affect others)

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

1. Environment variables (`HERMIP_*`)
2. CLI flags (`--config`, `--port`)
3. Local config (`./hermip.toml`)
4. Global config (`~/.config/hermip/hermip.toml`)
5. Built-in defaults

Legacy ClawHip `[discord]` format is transparently mapped to new Hermip config structure.

## Key Invariants

- Events are never dropped intentionally — only on sink delivery failure after circuit breaker exhaustion
- Multi-delivery is independent — one sink failure does not affect others
- Route matching is AND-logic — all conditions must match
- Config backward compatibility is guaranteed for ClawHip format
- The daemon must remain stable under invalid inputs (no crashes from bad events)
