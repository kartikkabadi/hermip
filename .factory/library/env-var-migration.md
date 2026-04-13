# Environment Variable Migration: CLAWHIP_* → HERMIP_*

## Status: Complete (with backward-compat fallbacks)

HERMIP_* vars are primary. CLAWHIP_* vars are deprecated but functional as backward-compat fallbacks with deprecation warnings. When both are set, HERMIP_* always wins.

## Files Modified

| File | Change |
|---|---|
| `src/config.rs` | Added `deprecated_fallback()` and `env_var_non_empty_deprecated()` helpers. Added CLAWHIP_* fallbacks in `apply_hermip_env_overrides_with()`, `daemon_base_url()`, `monitor_github_token()`, and `discord_token_from_env_with()`. Added `defaults.webhook_discord` and `defaults.webhook_slack` fields. Added HERMIP_DISCORD_WEBHOOK_URL and HERMIP_SLACK_WEBHOOK_URL overrides. Updated DISCORD_TOKEN_ENV_VARS to include CLAWHIP_DISCORD_BOT_TOKEN. |
| `src/lifecycle.rs` | Added `SKIP_STAR_PROMPT_ENV_LEGACY` constant and CLAWHIP_SKIP_STAR_PROMPT fallback with deprecation warning. |
| `src/plugins.rs` | Added `PLUGIN_DIR_ENV_LEGACY` constant and CLAWHIP_PLUGIN_DIR fallback with deprecation warning. |
| `src/native_hooks.rs` | Added `process.env.CLAWHIP_PROVIDER` fallback in embedded JS. |
| `src/source/git.rs` | Added CLAWHIP_GIT_BIN fallback with deprecation warning. |
| `src/source/tmux.rs` | Added CLAWHIP_TMUX_BIN fallback with deprecation warning. |
| `src/dynamic_tokens.rs` | Added CLAWHIP_TMUX_BIN fallback with deprecation warning. |
| `src/discord.rs` | Added CLAWHIP_DISCORD_API_BASE fallback with deprecation warning. Made MAX_ATTEMPTS pub(crate) for resilience parity test. |
| `src/router.rs` | Added default webhook URL fallback from `defaults.webhook_discord` and `defaults.webhook_slack` when no route-level webhook is configured. |

## HERMIP_* Environment Variables (Primary)

| Variable | Purpose | Default |
|---|---|---|
| `HERMIP_GIT_BIN` | Git binary path | `"git"` |
| `HERMIP_TMUX_BIN` | Tmux binary path | `"tmux"` |
| `HERMIP_DISCORD_API_BASE` | Discord API base URL | `"https://discord.com/api/v10"` |
| `HERMIP_DAEMON_URL` | Daemon base URL override | config `[daemon].base_url` |
| `HERMIP_GITHUB_TOKEN` | GitHub API token override | config `[monitors].github_token` |
| `HERMIP_DAEMON_PORT` | Daemon port override | config `[daemon].port` |
| `HERMIP_DAEMON_BASE_URL` | Daemon base URL (apply_hermip_env_overrides) | config `[daemon].base_url` |
| `HERMIP_DEFAULTS_CHANNEL` | Default channel override | config `[defaults].channel` |
| `HERMIP_DEFAULTS_FORMAT` | Default format override | config `[defaults].format` |
| `HERMIP_PROVIDERS_DISCORD_TOKEN` | Discord bot token override | config `[providers.discord].bot_token` |
| `HERMIP_DEFAULTS_CHANNEL_NAME` | Default channel name override | config `[defaults].channel_name` |
| `HERMIP_DISCORD_WEBHOOK_URL` | Default Discord webhook URL | config `[defaults].webhook_discord` |
| `HERMIP_SLACK_WEBHOOK_URL` | Default Slack webhook URL | config `[defaults].webhook_slack` |
| `HERMIP_CONFIG` | Config file path override | auto-detected |
| `HERMIP_SKIP_STAR_PROMPT` | Skip GitHub star prompt | not set |
| `HERMIP_PLUGIN_DIR` | Plugin directory override | auto-detected |
| `HERMIP_DISCORD_BOT_TOKEN` | Discord bot token (in DISCORD_TOKEN_ENV_VARS) | — |
| `DISCORD_TOKEN` | Discord bot token (legacy compat, in DISCORD_TOKEN_ENV_VARS) | — |

## CLAWHIP_* Deprecated Fallbacks (VAL-CROSS-001)

| Legacy Variable | HERMIP_* Equivalent | Deprecation Warning |
|---|---|---|
| `CLAWHIP_DAEMON_PORT` | `HERMIP_DAEMON_PORT` | Yes |
| `CLAWHIP_DAEMON_URL` | `HERMIP_DAEMON_URL` | Yes |
| `CLAWHIP_DISCORD_BOT_TOKEN` | `HERMIP_DISCORD_BOT_TOKEN` | Yes |
| `CLAWHIP_DISCORD_WEBHOOK_URL` | `HERMIP_DISCORD_WEBHOOK_URL` | Yes |
| `CLAWHIP_SLACK_WEBHOOK_URL` | `HERMIP_SLACK_WEBHOOK_URL` | Yes |
| `CLAWHIP_GITHUB_TOKEN` | `HERMIP_GITHUB_TOKEN` | Yes |
| `CLAWHIP_GIT_BIN` | `HERMIP_GIT_BIN` | Yes |
| `CLAWHIP_TMUX_BIN` | `HERMIP_TMUX_BIN` | Yes |
| `CLAWHIP_DISCORD_API_BASE` | `HERMIP_DISCORD_API_BASE` | Yes |
| `CLAWHIP_SKIP_STAR_PROMPT` | `HERMIP_SKIP_STAR_PROMPT` | Yes |
| `CLAWHIP_PLUGIN_DIR` | `HERMIP_PLUGIN_DIR` | Yes |
| `CLAWHIP_PROVIDER` | `HERMIP_PROVIDER` | Yes (JS only) |

## Resilience Parity (VAL-CROSS-002)

Discord and Slack sinks have identical resilience behavior:
- 3 retries with jitter (MAX_ATTEMPTS=3, JITTER_MS=50)
- Circuit breaker (CIRCUIT_FAILURE_THRESHOLD=3, CIRCUIT_COOLDOWN_SECS=5)
- Dead Letter Queue (in-memory)
- Rate limit handling (429 → Retry-After → backoff)
- Exponential backoff for non-429 server errors (Slack only; Discord uses JSON body for retry_after)

## Testing Pattern for Env Var Tests

When writing tests that set env vars, always save and restore the previous value to avoid flaky parallel test failures:

```rust
#[test]
fn my_env_test() {
    let previous = std::env::var_os("HERMIP_SOME_VAR");
    unsafe { std::env::remove_var("HERMIP_SOME_VAR"); }

    // Test default behavior
    assert_eq!(some_fn(), "default");

    // Test override
    unsafe { std::env::set_var("HERMIP_SOME_VAR", "custom"); }
    assert_eq!(some_fn(), "custom");

    // Restore
    unsafe {
        if let Some(prev) = previous {
            std::env::set_var("HERMIP_SOME_VAR", prev);
        } else {
            std::env::remove_var("HERMIP_SOME_VAR");
        }
    }
}
```

For testing env var overrides with the injectable `_with` pattern (preferred):

```rust
#[test]
fn my_injectable_env_test() {
    let mut config = AppConfig::default();
    config.apply_hermip_env_overrides_with(|name| {
        (name == "HERMIP_SOME_VAR").then(|| "custom".to_string())
    });
    assert_eq!(config.some_field, "custom");
}
```
