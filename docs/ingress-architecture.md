# Ingress Architecture

## Overview

Hermip supports multiple provider-native hook integrations through a flexible ingress architecture. Providers can be enabled via compile-time feature flags, allowing users to customize which integrations are available.

## Provider Abstraction

### CLI Layer: `HookProvider` Enum
Located in `src/cli.rs`, this enum represents providers in CLI commands:

```rust
pub enum HookProvider {
    #[cfg(feature = "codex-hook")]
    Codex,
    #[cfg(feature = "claude-hook")]
    ClaudeCode,
    #[value(name = "hermes", alias = "hermes-agent")]
    Hermes,
}
```

### Internal Layer: `ProviderKind` Enum
Located in `src/hooks/prompt_deliver.rs`, this enum represents providers internally:

```rust
pub enum ProviderKind {
    #[cfg(feature = "claude-hook")]
    Omc,  // Claude Code
    #[cfg(feature = "codex-hook")]
    Omx,  // Codex
}
```

## Ingress Methods

### Native Hook Ingress
Primary ingress for provider-native hook payloads:

```bash
hermip native hook --provider <provider> --file payload.json
hermip native hook --provider <provider> --payload '{"..."}'
cat payload.json | hermip native hook --provider <provider>
```

Supported providers depend on enabled features:
- `hermes` (always available)
- `codex` (requires `codex-hook` feature)
- `claude-code` (requires `claude-hook` feature)

### Hook Installation
Install provider-specific hook forwarding:

```bash
hermip hooks install --provider <provider> --scope <project|global>
hermip hooks install --all  # Install all enabled providers
```

## Shared Hook Events

All providers support a set of shared events defined in `SHARED_HOOK_EVENTS`:

- `SessionStart`
- `PreToolUse`
- `PostToolUse`
- `UserPromptSubmit`
- `Stop`

These events are provider-agnostic and normalized into a common routing contract.

## Provider-Specific Integration Points

### Hook Installation
Each provider has provider-specific installation logic:

- **Codex**: Writes to `.codex/hooks.json` or `<repo>/.codex/hooks.json`
- **Claude Code**: Writes to `~/.claude/settings.json` (global-only)
- **Hermes**: Writes to `.hermes/plugins/hermip/` as a Python plugin

### Hook Detection
The system detects hook setups by scanning:
- Project directory for provider-specific config files
- Global home directory for provider-specific config files
- Presence of hermip's native hook script (`.hermip/hooks/native-hook.mjs`)

### Normalization
Provider-specific payloads are normalized via:
- Field mapping from provider-specific paths to common contract
- Provider-specific augmentation logic
- Shared event type mapping

## Extensibility for New Providers

To add a new provider (e.g., Droid):

1. **Add feature flag** in `Cargo.toml`:
   ```toml
   [features]
   droid-hook = []
   ```

2. **Add enum variant** in `HookProvider`:
   ```rust
   #[cfg(feature = "droid-hook")]
   Droid,
   ```

3. **Add enum variant** in `ProviderKind` (if needed for internal logic)

4. **Add hook installation logic** in `src/hooks/mod.rs`:
   - Provider-specific file paths
   - Provider-specific hook command generation
   - Provider-specific matcher logic (if needed)

5. **Add hook detection logic** in `src/hooks/prompt_deliver.rs`:
   - Provider-specific config file detection
   - Provider-specific hook command validation

6. **Add normalization logic** in `src/native_hooks.rs`:
   - Provider-specific field mappings
   - Provider-specific augmentation

## Feature Flag Strategy

- **Default features**: Only `hermes` is enabled by default
- **Optional features**: `codex-hook`, `claude-hook` available for users who need them
- **Future providers**: Should follow the same pattern - opt-in via feature flags

This ensures:
- Minimal default build size
- Users only compile what they need
- Backward compatibility for users who need legacy providers
