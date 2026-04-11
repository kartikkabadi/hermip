# User Testing

**What belongs here:** Testing surface, required testing tools, resource cost classification per surface.

---

## Validation Surfaces

### Surface 1: CLI (hermip commands)

**Testing tool:** tuistory  
**What to test:** All hermip CLI subcommands (daemon start/stop, hooks install/uninstall, config show/set/verify-bindings, send, release preflight, --version, --help)  
**Setup required:** Built hermip binary in PATH

### Surface 2: HTTP API (hermip daemon)

**Testing tool:** curl  
**What to test:** GET /health, POST /api/event, error handling (400, 404, 415), SIGTERM shutdown, port binding  
**Setup required:** Running hermip daemon on port 25294

### Surface 3: Event Pipeline (sources, router, renderer, sink)

**Testing tool:** curl (for source injection), tuistory (for tmux), agent-browser (for Discord/Slack delivery verification)  
**What to test:** Event normalization, routing, rendering, delivery, rate limiting, circuit breaker  
**Setup required:** Running daemon with test configuration, Discord/Slack webhook URLs

### Surface 4: Hermes Agent Integration (plugin, skill, hooks)

**Testing tool:** tuistory (CLI commands), agent-browser (Discord verification)  
**What to test:** Plugin installation, hook registration, event emission, skill discoverability  
**Setup required:** Hermes Agent installed, hermip daemon running

### Surface 5: Droid Mission Integration (event types, factory infrastructure)

**Testing tool:** curl (event injection), tuistory (CLI)  
**What to test:** All 11 Droid event types, per-milestone routing, per-feature routing, .factory/skills/  
**Setup required:** Running daemon with routing rules configured

### Surface 6: Configuration (TOML, env vars, legacy format)

**Testing tool:** tuistory  
**What to test:** Config loading, precedence, defaults, env var overrides, legacy compat  
**Setup required:** Various hermip.toml test configurations

## Validation Concurrency

- **CLI surface:** Lightweight — max 5 concurrent validators
- **HTTP API surface:** Lightweight — max 5 concurrent validators
- **Event pipeline surface:** Moderate — max 3 concurrent validators (Discord/Slack rate limits)
- **Hermes integration surface:** Moderate — max 3 concurrent validators (requires Hermes Agent)
- **Droid integration surface:** Lightweight — max 5 concurrent validators
- **Config surface:** Lightweight — max 5 concurrent validators

## Resource Cost Estimate

- **Hermip daemon:** ~10-20MB RAM, minimal CPU
- **Test configuration:** Multiple hermip.toml files (~1KB each)
- **Discord/Slack delivery:** Requires real webhook URLs for end-to-end testing
- **Hermes Agent:** If installed, adds ~200-500MB RAM
