# Environment Variable Migration: CLAWHIP_* → HERMIP_*

## Status: Complete

All CLAWHIP_* environment variable fallbacks have been removed from production code. HERMIP_* is now the sole env var namespace.

## Files Modified

| File | Change |
|---|---|
| `src/config.rs` | Replaced `env_var_or_fallback(primary, fallback)` with `env_var_non_empty(name)`. Removed CLAWHIP_DAEMON_URL and CLAWHIP_GITHUB_TOKEN fallbacks from `daemon_base_url()` and `monitor_github_token()`. |
| `src/lifecycle.rs` | Removed `SKIP_STAR_PROMPT_ENV_LEGACY` constant and `.or_else(|| env::var(CLAWHIP_SKIP_STAR_PROMPT))` fallback. |
| `src/plugins.rs` | Removed `PLUGIN_DIR_ENV_LEGACY` constant and loop over both env names. Now checks only `HERMIP_PLUGIN_DIR`. |
| `src/native_hooks.rs` | Removed `CLAWHIP_DIR`, `CLAWHIP_PROJECT_FILE` const aliases. Removed `process.env.CLAWHIP_PROVIDER` fallback from embedded JS. |
| `src/source/git.rs` | Already used only `HERMIP_GIT_BIN` (no change needed). Added test. |
| `src/source/tmux.rs` | Already used only `HERMIP_TMUX_BIN` (no change needed). Added test. |
| `src/dynamic_tokens.rs` | Already used only `HERMIP_TMUX_BIN` (no change needed). Added test. |
| `src/discord.rs` | Already used only `HERMIP_DISCORD_API_BASE` (no change needed). Added test. |

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
| `HERMIP_CONFIG` | Config file path override | auto-detected |
| `HERMIP_SKIP_STAR_PROMPT` | Skip GitHub star prompt | not set |
| `HERMIP_PLUGIN_DIR` | Plugin directory override | auto-detected |
| `HERMIP_DISCORD_BOT_TOKEN` | Discord bot token (in DISCORD_TOKEN_ENV_VARS) | — |
| `DISCORD_TOKEN` | Discord bot token (legacy compat, in DISCORD_TOKEN_ENV_VARS) | — |

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
