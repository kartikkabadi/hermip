use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::Result;
use crate::events::MessageFormat;
use crate::source::workspace::{default_workspace_debounce_ms, default_workspace_watch_dirs};

/// Check a primary env var first, falling back to a legacy name.
/// Returns `Some(value)` if either is set and non-empty, preferring the primary.
/// Returns `None` if neither is set.
fn env_var_or_fallback(primary: &str, fallback: &str) -> Option<String> {
    env::var(primary)
        .ok()
        .filter(|v| !v.trim().is_empty())
        .or_else(|| env::var(fallback).ok().filter(|v| !v.trim().is_empty()))
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AppConfig {
    #[serde(default, skip_serializing_if = "DiscordConfig::is_empty")]
    pub discord: DiscordConfig,
    #[serde(default, skip_serializing_if = "ProvidersConfig::is_empty")]
    pub providers: ProvidersConfig,
    #[serde(default)]
    pub dispatch: DispatchConfig,
    #[serde(default)]
    pub daemon: DaemonConfig,
    #[serde(default)]
    pub defaults: DefaultsConfig,
    #[serde(default)]
    pub routes: Vec<RouteRule>,
    #[serde(default)]
    pub monitors: MonitorConfig,
    #[serde(default, skip_serializing_if = "CronConfig::is_empty")]
    pub cron: CronConfig,
    #[serde(default, skip_serializing_if = "crate::update::UpdateConfig::is_empty")]
    pub update: crate::update::UpdateConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProvidersConfig {
    #[serde(default)]
    pub discord: DiscordConfig,
    #[serde(default)]
    pub slack: SlackConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DiscordConfig {
    #[serde(alias = "token")]
    pub bot_token: Option<String>,
    #[serde(alias = "default_channel")]
    pub legacy_default_channel: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SlackConfig {}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaemonConfig {
    #[serde(default = "default_bind_host")]
    pub bind_host: String,
    #[serde(default = "default_port")]
    pub port: u16,
    #[serde(default = "default_base_url")]
    pub base_url: String,
}

impl DiscordConfig {
    fn is_empty(&self) -> bool {
        self.bot_token.is_none() && self.legacy_default_channel.is_none()
    }
}

impl ProvidersConfig {
    fn is_empty(&self) -> bool {
        self.discord.is_empty() && self.slack.is_empty()
    }
}

impl Default for DaemonConfig {
    fn default() -> Self {
        Self {
            bind_host: default_bind_host(),
            port: default_port(),
            base_url: default_base_url(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DispatchConfig {
    #[serde(default = "default_ci_batch_window_secs")]
    pub ci_batch_window_secs: u64,
    #[serde(default = "default_routine_batch_window_secs")]
    pub routine_batch_window_secs: u64,
}

impl Default for DispatchConfig {
    fn default() -> Self {
        Self {
            ci_batch_window_secs: default_ci_batch_window_secs(),
            routine_batch_window_secs: default_routine_batch_window_secs(),
        }
    }
}

impl DispatchConfig {
    pub fn ci_batch_window(&self) -> Duration {
        Duration::from_secs(self.ci_batch_window_secs.max(1))
    }

    pub fn routine_batch_window(&self) -> Option<Duration> {
        (self.routine_batch_window_secs > 0)
            .then(|| Duration::from_secs(self.routine_batch_window_secs))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DefaultsConfig {
    pub channel: Option<String>,
    /// Human-readable channel name hint for the default channel (binding verification).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel_name: Option<String>,
    #[serde(default)]
    pub format: MessageFormat,
}

impl Default for DefaultsConfig {
    fn default() -> Self {
        Self {
            channel: None,
            channel_name: None,
            format: MessageFormat::Compact,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RouteRule {
    pub event: String,
    #[serde(default)]
    pub filter: BTreeMap<String, String>,
    #[serde(default = "default_sink_name")]
    pub sink: String,
    pub channel: Option<String>,
    /// Human-readable Discord channel name hint for binding verification.
    /// When set, `hermip config verify-bindings` compares the live channel
    /// name against this value to detect drift.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel_name: Option<String>,
    pub webhook: Option<String>,
    pub slack_webhook: Option<String>,
    pub mention: Option<String>,
    #[serde(default)]
    pub allow_dynamic_tokens: bool,
    pub format: Option<MessageFormat>,
    pub template: Option<String>,
}

impl Default for RouteRule {
    fn default() -> Self {
        Self {
            event: String::new(),
            filter: BTreeMap::new(),
            sink: default_sink_name(),
            channel: None,
            channel_name: None,
            webhook: None,
            slack_webhook: None,
            mention: None,
            allow_dynamic_tokens: false,
            format: None,
            template: None,
        }
    }
}

impl SlackConfig {
    fn is_empty(&self) -> bool {
        true
    }
}

impl RouteRule {
    pub fn effective_sink(&self) -> &str {
        let sink = self.sink.trim();
        if self.slack_webhook_target().is_some() && (sink.is_empty() || sink == "discord") {
            "slack"
        } else if sink.is_empty() {
            "discord"
        } else {
            sink
        }
    }

    pub fn discord_webhook_target(&self) -> Option<&str> {
        (self.effective_sink() == "discord")
            .then(|| non_empty_trimmed(self.webhook.as_deref()))
            .flatten()
    }

    pub fn slack_webhook_target(&self) -> Option<&str> {
        non_empty_trimmed(self.slack_webhook.as_deref()).or_else(|| {
            (self.sink.trim() == "slack").then(|| non_empty_trimmed(self.webhook.as_deref()))?
        })
    }

    fn has_any_webhook_target(&self) -> bool {
        self.discord_webhook_target().is_some() || self.slack_webhook_target().is_some()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MonitorConfig {
    #[serde(default = "default_poll_interval")]
    pub poll_interval_secs: u64,
    pub github_token: Option<String>,
    #[serde(default = "default_github_api_base")]
    pub github_api_base: String,
    #[serde(default)]
    pub git: GitMonitorConfig,
    #[serde(default)]
    pub tmux: TmuxMonitorConfig,
    #[serde(default)]
    pub workspace: Vec<WorkspaceMonitor>,
}

impl Default for MonitorConfig {
    fn default() -> Self {
        Self {
            poll_interval_secs: default_poll_interval(),
            github_token: None,
            github_api_base: default_github_api_base(),
            git: GitMonitorConfig::default(),
            tmux: TmuxMonitorConfig::default(),
            workspace: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct GitMonitorConfig {
    #[serde(default)]
    pub repos: Vec<GitRepoMonitor>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TmuxMonitorConfig {
    #[serde(default)]
    pub sessions: Vec<TmuxSessionMonitor>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitRepoMonitor {
    pub path: String,
    pub name: Option<String>,
    #[serde(default = "default_remote")]
    pub remote: String,
    pub github_repo: Option<String>,
    #[serde(default = "default_true")]
    pub emit_commits: bool,
    #[serde(default = "default_true")]
    pub emit_branch_changes: bool,
    #[serde(default = "default_true")]
    pub emit_issue_opened: bool,
    #[serde(default)]
    pub emit_pr_status: bool,
    pub channel: Option<String>,
    /// Human-readable channel name hint for binding verification.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel_name: Option<String>,
    pub mention: Option<String>,
    pub format: Option<MessageFormat>,
}

impl Default for GitRepoMonitor {
    fn default() -> Self {
        Self {
            path: String::new(),
            name: None,
            remote: default_remote(),
            github_repo: None,
            emit_commits: true,
            emit_branch_changes: true,
            emit_issue_opened: true,
            emit_pr_status: false,
            channel: None,
            channel_name: None,
            mention: None,
            format: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TmuxSessionMonitor {
    pub session: String,
    #[serde(default)]
    pub keywords: Vec<String>,
    #[serde(default = "default_keyword_window_secs")]
    pub keyword_window_secs: u64,
    #[serde(default = "default_stale_minutes")]
    pub stale_minutes: u64,
    pub channel: Option<String>,
    /// Human-readable channel name hint for binding verification.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel_name: Option<String>,
    pub mention: Option<String>,
    pub format: Option<MessageFormat>,
}

impl Default for TmuxSessionMonitor {
    fn default() -> Self {
        Self {
            session: String::new(),
            keywords: Vec::new(),
            keyword_window_secs: default_keyword_window_secs(),
            stale_minutes: default_stale_minutes(),
            channel: None,
            channel_name: None,
            mention: None,
            format: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceMonitor {
    pub path: String,
    #[serde(default = "default_workspace_watch_dirs")]
    pub watch_dirs: Vec<String>,
    #[serde(default)]
    pub discover_worktrees: bool,
    pub channel: Option<String>,
    pub mention: Option<String>,
    pub format: Option<MessageFormat>,
    #[serde(default)]
    pub events: Vec<String>,
    pub poll_interval_secs: Option<u64>,
    #[serde(default = "default_workspace_debounce_ms")]
    pub debounce_ms: u64,
}

impl Default for WorkspaceMonitor {
    fn default() -> Self {
        Self {
            path: String::new(),
            watch_dirs: default_workspace_watch_dirs(),
            discover_worktrees: false,
            channel: None,
            mention: None,
            format: None,
            events: Vec::new(),
            poll_interval_secs: None,
            debounce_ms: default_workspace_debounce_ms(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CronConfig {
    #[serde(default = "default_cron_poll_interval_secs")]
    pub poll_interval_secs: u64,
    #[serde(default)]
    pub jobs: Vec<CronJob>,
}

impl Default for CronConfig {
    fn default() -> Self {
        Self {
            poll_interval_secs: default_cron_poll_interval_secs(),
            jobs: Vec::new(),
        }
    }
}

impl CronConfig {
    fn is_empty(&self) -> bool {
        self.jobs.is_empty()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CronJob {
    pub id: String,
    pub schedule: String,
    #[serde(default = "default_cron_timezone")]
    pub timezone: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    pub channel: Option<String>,
    pub mention: Option<String>,
    pub format: Option<MessageFormat>,
    /// Optional path to a JSON state file that gates this job's emissions.
    ///
    /// When set, the cron scheduler reads the file before emitting. If the
    /// file parses as `{"open_issues": 0, "open_prs": 0, ...}` (zero backlog)
    /// **and** the canonical JSON fingerprint matches the one from the last
    /// emission for this job, the scheduler suppresses the emission. Any
    /// delta in the file (including fields beyond the backlog counters) or a
    /// non-zero backlog causes the job to fire again immediately. Missing or
    /// malformed state files fail open so existing jobs keep working.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state_file: Option<PathBuf>,
    #[serde(flatten)]
    pub kind: CronJobKind,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum CronJobKind {
    CustomMessage { message: String },
}

/// Returns the default config path using real environment and filesystem.
///
/// Resolution order:
/// 1. `HERMIP_CONFIG` env var (if set and non-empty)
/// 2. `hermip.toml` in the current working directory (if it exists)
/// 3. `~/.config/hermip/hermip.toml` (global fallback)
pub fn default_config_path() -> PathBuf {
    default_config_path_with(
        |name| env::var(name).ok(),
        || env::current_dir().ok(),
        |name| env::var(name).ok(),
    )
}

/// Returns the default config path using injectable environment and
/// filesystem readers, enabling unit testing without real env vars.
fn default_config_path_with<F, G, H>(mut get_env: F, get_cwd: G, mut get_env_home: H) -> PathBuf
where
    F: FnMut(&str) -> Option<String>,
    G: FnOnce() -> Option<PathBuf>,
    H: FnMut(&str) -> Option<String>,
{
    // 1. HERMIP_CONFIG override takes highest precedence.
    if let Some(override_path) = get_env("HERMIP_CONFIG")
        && !override_path.trim().is_empty()
    {
        return PathBuf::from(override_path);
    }
    // 2. Check for hermip.toml in current working directory.
    if let Some(cwd) = get_cwd() {
        let local = cwd.join("hermip.toml");
        if local.exists() {
            return local;
        }
    }
    // 3. Fall back to ~/.config/hermip/hermip.toml (global).
    let home = get_env_home("HOME").unwrap_or_else(|| ".".to_string());
    PathBuf::from(home)
        .join(".config")
        .join("hermip")
        .join("hermip.toml")
}

fn default_bind_host() -> String {
    "0.0.0.0".to_string()
}
fn default_port() -> u16 {
    25294
}
fn default_base_url() -> String {
    format!("http://127.0.0.1:{}", default_port())
}
fn default_poll_interval() -> u64 {
    5
}
fn default_github_api_base() -> String {
    "https://api.github.com".to_string()
}
fn default_remote() -> String {
    "origin".to_string()
}
fn default_stale_minutes() -> u64 {
    10
}
fn default_ci_batch_window_secs() -> u64 {
    30
}
fn default_routine_batch_window_secs() -> u64 {
    5
}
fn default_keyword_window_secs() -> u64 {
    30
}
fn default_cron_poll_interval_secs() -> u64 {
    30
}
fn default_cron_timezone() -> String {
    "UTC".to_string()
}
fn default_true() -> bool {
    true
}

pub fn default_sink_name() -> String {
    "discord".to_string()
}

const DISCORD_TOKEN_ENV_VARS: [&str; 2] = [
    "DISCORD_TOKEN",
    "HERMIP_DISCORD_BOT_TOKEN",
];
pub const CONFIG_EDITOR_MENU_ITEMS: [&str; 8] = [
    "Set Discord bot token",
    "Set daemon base URL",
    "Set default channel",
    "Set default format",
    "Set Discord webhook quickstart route",
    "Save and exit",
    "Exit without saving",
    "Print manual config template hint",
];

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SetupEdits {
    pub webhook: Option<String>,
    pub bot_token: Option<String>,
    pub default_channel: Option<String>,
    pub default_format: Option<MessageFormat>,
    pub daemon_base_url: Option<String>,
}

impl SetupEdits {
    pub fn is_empty(&self) -> bool {
        self.webhook.is_none()
            && self.bot_token.is_none()
            && self.default_channel.is_none()
            && self.default_format.is_none()
            && self.daemon_base_url.is_none()
    }
}

fn merge_legacy_discord_field(
    field: &str,
    legacy: Option<String>,
    provider: &mut Option<String>,
) -> Result<()> {
    let legacy = normalize_text(legacy);
    let provider_value = normalize_text(provider.clone());

    match (legacy, provider_value) {
        (Some(legacy), Some(provider_value)) if legacy != provider_value => Err(format!(
            "conflicting legacy [discord].{field} and [providers.discord].{field} values"
        )
        .into()),
        (Some(legacy), None) => {
            *provider = Some(legacy);
            Ok(())
        }
        (_, Some(provider_value)) => {
            *provider = Some(provider_value);
            Ok(())
        }
        (None, None) => {
            *provider = None;
            Ok(())
        }
    }
}

fn normalize_secret(value: Option<String>) -> Option<String> {
    value.and_then(|value| {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    })
}

fn non_empty_trimmed(value: Option<&str>) -> Option<&str> {
    value.and_then(|value| {
        let trimmed = value.trim();
        (!trimmed.is_empty()).then_some(trimmed)
    })
}

fn discord_token_from_env_with<F>(mut get_env: F) -> Option<String>
where
    F: FnMut(&str) -> Option<String>,
{
    DISCORD_TOKEN_ENV_VARS
        .iter()
        .find_map(|name| normalize_secret(get_env(name)))
}

impl AppConfig {
    pub fn load_or_default(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let raw = fs::read_to_string(path)
            .map_err(|e| format!("failed to read config file {}: {e}", path.display()))?;
        let raw_toml: toml::Value = toml::from_str(&raw)
            .map_err(|e| format!("failed to parse config file {}: {e}", path.display()))?;
        let mut config: Self = toml::from_str(&raw)
            .map_err(|e| format!("failed to deserialize config file {}: {e}", path.display()))?;
        config.merge_legacy_discord(&raw_toml)?;
        config.normalize();
        if config.defaults.channel.is_none() {
            config.defaults.channel = config.discord_default_channel();
        }
        // VAL-CONFIG-006: HERMIP_* env vars override TOML values (highest precedence).
        config.apply_hermip_env_overrides();
        Ok(config)
    }

    /// Apply HERMIP_* environment variable overrides.
    /// These take highest precedence over TOML values.
    fn apply_hermip_env_overrides(&mut self) {
        self.apply_hermip_env_overrides_with(|name| env::var(name).ok());
    }

    /// Apply HERMIP_* environment variable overrides using an injectable
    /// env-var reader. Uses the same pattern as `effective_token_with()` and
    /// `discord_token_source_with()` to enable unit testing without setting
    /// real environment variables.
    pub fn apply_hermip_env_overrides_with<F>(&mut self, mut get_env: F)
    where
        F: FnMut(&str) -> Option<String>,
    {
        // HERMIP_DAEMON_PORT overrides [daemon].port
        if let Some(port_str) = get_env("HERMIP_DAEMON_PORT")
            && let Ok(port) = port_str.parse::<u16>()
        {
            self.daemon.port = port;
        }
        // HERMIP_DAEMON_BASE_URL overrides [daemon].base_url
        if let Some(url) = get_env("HERMIP_DAEMON_BASE_URL")
            && !url.trim().is_empty()
        {
            self.daemon.base_url = url.trim().to_string();
        }
        // HERMIP_DEFAULTS_CHANNEL overrides [defaults].channel
        if let Some(ch) = get_env("HERMIP_DEFAULTS_CHANNEL")
            && !ch.trim().is_empty()
        {
            self.defaults.channel = Some(ch.trim().to_string());
        }
        // HERMIP_DEFAULTS_FORMAT overrides [defaults].format
        if let Some(fmt) = get_env("HERMIP_DEFAULTS_FORMAT")
            && !fmt.trim().is_empty()
            && let Ok(format) = crate::events::MessageFormat::from_label(fmt.trim())
        {
            self.defaults.format = format;
        }
        // HERMIP_PROVIDERS_DISCORD_TOKEN overrides [providers.discord].token
        if let Some(token) = get_env("HERMIP_PROVIDERS_DISCORD_TOKEN")
            && !token.trim().is_empty()
        {
            self.providers.discord.bot_token = Some(token.trim().to_string());
        }
        // HERMIP_DEFAULTS_CHANNEL_NAME overrides [defaults].channel_name
        if let Some(name) = get_env("HERMIP_DEFAULTS_CHANNEL_NAME")
            && !name.trim().is_empty()
        {
            self.defaults.channel_name = Some(name.trim().to_string());
        }
    }

    fn merge_legacy_discord(&mut self, raw_toml: &toml::Value) -> Result<()> {
        if raw_toml.get("discord").is_some() {
            merge_legacy_discord_field(
                "token",
                self.discord.bot_token.clone(),
                &mut self.providers.discord.bot_token,
            )?;
            merge_legacy_discord_field(
                "default_channel",
                self.discord.legacy_default_channel.clone(),
                &mut self.providers.discord.legacy_default_channel,
            )?;
        }

        self.discord = DiscordConfig::default();
        Ok(())
    }

    fn discord_default_channel(&self) -> Option<String> {
        normalize_text(self.providers.discord.legacy_default_channel.clone())
            .or_else(|| normalize_text(self.discord.legacy_default_channel.clone()))
    }

    pub fn to_pretty_toml(&self) -> Result<String> {
        Ok(toml::to_string_pretty(self)?)
    }

    /// Returns a masked copy of this config with all secret/credential fields
    /// replaced by `"***"` for safe display. Original values are preserved in
    /// `self` for daemon operation.
    pub fn masked(&self) -> Self {
        let mut config = self.clone();
        config.discord.bot_token = config.discord.bot_token.as_ref().map(|_| "***".to_string());
        config.providers.discord.bot_token = config
            .providers
            .discord
            .bot_token
            .as_ref()
            .map(|_| "***".to_string());
        config.monitors.github_token = config
            .monitors
            .github_token
            .as_ref()
            .map(|_| "***".to_string());
        for route in &mut config.routes {
            route.webhook = route.webhook.as_ref().map(|_| "***".to_string());
            route.slack_webhook = route.slack_webhook.as_ref().map(|_| "***".to_string());
        }
        config
    }

    /// Returns the config as a masked TOML string for safe display.
    /// All secret/credential fields are replaced with `"***"`.
    pub fn to_display_toml(&self) -> Result<String> {
        self.masked().to_pretty_toml()
    }

    /// Returns whether a dot-separated config key designates a secret field.
    /// Secret values should be masked in display output.
    pub fn is_secret_key(key: &str) -> bool {
        matches!(
            key,
            "discord.token" | "providers.discord.bot_token" | "monitors.github_token"
        ) || key.ends_with(".webhook")
            || key.ends_with(".slack_webhook")
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, self.to_pretty_toml()?)?;
        Ok(())
    }

    pub fn effective_token(&self) -> Option<String> {
        self.effective_token_with(|name| env::var(name).ok())
    }

    fn effective_token_with<F>(&self, get_env: F) -> Option<String>
    where
        F: FnMut(&str) -> Option<String>,
    {
        discord_token_from_env_with(get_env)
            .or_else(|| normalize_secret(self.providers.discord.bot_token.clone()))
            .or_else(|| normalize_secret(self.discord.bot_token.clone()))
    }

    pub fn discord_token_source(&self) -> &'static str {
        self.discord_token_source_with(|name| env::var(name).ok())
    }

    fn discord_token_source_with<F>(&self, get_env: F) -> &'static str
    where
        F: FnMut(&str) -> Option<String>,
    {
        if discord_token_from_env_with(get_env).is_some() {
            "env"
        } else if normalize_secret(self.providers.discord.bot_token.clone()).is_some()
            || normalize_secret(self.discord.bot_token.clone()).is_some()
        {
            "config"
        } else {
            "missing"
        }
    }

    pub fn webhook_route_count(&self) -> usize {
        self.routes
            .iter()
            .filter(|route| route.has_any_webhook_target())
            .count()
    }

    pub fn has_webhook_routes(&self) -> bool {
        self.webhook_route_count() > 0
    }

    pub fn validate(&self) -> Result<()> {
        if self.dispatch.ci_batch_window_secs == 0 {
            return Err("dispatch.ci_batch_window_secs must be at least 1".into());
        }
        if self.cron.poll_interval_secs == 0 {
            return Err("cron.poll_interval_secs must be at least 1".into());
        }

        for (index, route) in self.routes.iter().enumerate() {
            let sink = route.effective_sink();
            let has_channel = normalize_secret(route.channel.clone()).is_some();
            let has_discord_webhook = route.discord_webhook_target().is_some();
            let has_slack_webhook = route.slack_webhook_target().is_some();
            if route.sink.trim().is_empty() && !has_slack_webhook {
                return Err(
                    format!("route #{} ({}) must set a sink", index + 1, route.event).into(),
                );
            }
            if !matches!(sink, "discord" | "slack") {
                return Err(format!(
                    "route #{} ({}) uses unsupported sink '{}'",
                    index + 1,
                    route.event,
                    sink
                )
                .into());
            }

            match sink {
                "discord" => {
                    if has_channel && has_discord_webhook {
                        return Err(format!(
                            "route #{} ({}) cannot set both channel and webhook",
                            index + 1,
                            route.event
                        )
                        .into());
                    }
                }
                "slack" => {
                    if has_channel {
                        return Err(format!(
                            "route #{} ({}) cannot set channel when sink = \"slack\"",
                            index + 1,
                            route.event
                        )
                        .into());
                    }
                    if normalize_secret(route.webhook.clone()).is_some()
                        && normalize_secret(route.slack_webhook.clone()).is_some()
                    {
                        return Err(format!(
                            "route #{} ({}) cannot set both webhook and slack_webhook for Slack delivery",
                            index + 1,
                            route.event
                        )
                        .into());
                    }
                    if !has_slack_webhook {
                        return Err(format!(
                            "route #{} ({}) must set webhook or slack_webhook when sink = \"slack\"",
                            index + 1,
                            route.event
                        )
                        .into());
                    }
                }
                _ => unreachable!(),
            }
        }

        for (index, workspace) in self.monitors.workspace.iter().enumerate() {
            if workspace.path.trim().is_empty() {
                return Err(format!("workspace monitor #{} must set path", index + 1).into());
            }
            if workspace.watch_dirs.is_empty() {
                return Err(format!(
                    "workspace monitor #{} must set at least one watch_dirs entry",
                    index + 1
                )
                .into());
            }
            if workspace.channel.is_none()
                && self.defaults.channel.is_none()
                && !self.has_webhook_routes()
            {
                return Err(format!(
                    "workspace monitor #{} has no channel and no default Discord destination",
                    index + 1
                )
                .into());
            }
        }

        let mut cron_ids = std::collections::BTreeSet::new();
        for (index, job) in self.cron.jobs.iter().enumerate() {
            crate::cron::validate_job(job)
                .map_err(|error| format!("cron job #{}: {error}", index + 1))?;
            if !cron_ids.insert(job.id.as_str()) {
                return Err(format!("duplicate cron job id '{}'", job.id).into());
            }
        }

        if self.effective_token().is_none() && !self.has_webhook_routes() {
            return Err(
                "missing Discord delivery config: configure [providers.discord].token (or legacy [discord].token) or at least one route webhook"
                    .into(),
            );
        }

        Ok(())
    }

    pub fn apply_setup_edits(&mut self, edits: SetupEdits) -> Result<()> {
        let normalized = SetupEdits {
            webhook: normalize_text(edits.webhook),
            bot_token: normalize_secret(edits.bot_token),
            default_channel: normalize_text(edits.default_channel),
            default_format: edits.default_format,
            daemon_base_url: normalize_text(edits.daemon_base_url),
        };

        if normalized.is_empty() {
            return Err("setup requires at least one non-empty setup flag".into());
        }

        let SetupEdits {
            webhook,
            bot_token,
            default_channel,
            default_format,
            daemon_base_url,
        } = normalized;

        if let Some(webhook) = webhook {
            self.scaffold_webhook_quickstart(webhook)?;
        }
        if let Some(bot_token) = bot_token {
            self.providers.discord.bot_token = Some(bot_token);
        }
        if let Some(default_channel) = default_channel {
            self.defaults.channel = Some(default_channel);
        }
        if let Some(default_format) = default_format {
            self.defaults.format = default_format;
        }
        if let Some(daemon_base_url) = daemon_base_url {
            self.daemon.base_url = daemon_base_url;
        }

        Ok(())
    }

    /// Apply a single key=value update to the config.
    ///
    /// Keys use dot-notation to set nested fields, e.g. `daemon.port = 30999`.
    /// Values are parsed as TOML syntax. For bare integers or booleans, the value
    /// is parsed directly. For strings (including those representing integers like
    /// "25295"), wrap in quotes: `"25295"`.
    pub fn set_from_key_value(&mut self, key: &str, value: &str) -> Result<()> {
        // Try parsing as a bare TOML value first (integers, floats, booleans, strings in quotes).
        // If that fails, try wrapping in a TOML document fragment.
        let parsed: toml::Value = match value.parse::<toml::Value>() {
            Ok(v) => v,
            Err(_) => {
                let doc = format!("value = {}", value);
                let doc: toml::Value = doc
                    .parse()
                    .map_err(|e| format!("failed to parse '{}' as TOML: {}", value, e))?;
                doc.get("value").cloned().unwrap()
            }
        };

        let parts: Vec<&str> = key.split('.').collect();

        match parts.as_slice() {
            ["daemon", "port"] => {
                if let toml::Value::Integer(port) = parsed {
                    self.daemon.port = port as u16;
                } else {
                    return Err(format!("daemon.port must be an integer, got '{}'", value).into());
                }
            }
            ["daemon", "bind_host"] => {
                if let toml::Value::String(host) = parsed {
                    self.daemon.bind_host = host;
                } else {
                    return Err(
                        format!("daemon.bind_host must be a string, got '{}'", value).into(),
                    );
                }
            }
            ["daemon", "base_url"] => {
                if let toml::Value::String(url) = parsed {
                    self.daemon.base_url = url;
                } else {
                    return Err(format!("daemon.base_url must be a string, got '{}'", value).into());
                }
            }
            ["defaults", "channel"] => {
                if let toml::Value::String(ch) = parsed {
                    self.defaults.channel = Some(ch);
                } else {
                    return Err(
                        format!("defaults.channel must be a string, got '{}'", value).into(),
                    );
                }
            }
            ["defaults", "format"] => {
                if let toml::Value::String(fmt) = parsed {
                    self.defaults.format = crate::events::MessageFormat::from_label(&fmt)
                        .map_err(|e| format!("invalid defaults.format '{}': {}", fmt, e))?;
                } else {
                    return Err(format!("defaults.format must be a string, got '{}'", value).into());
                }
            }
            ["providers", "discord", "bot_token"] => {
                if let toml::Value::String(token) = parsed {
                    self.providers.discord.bot_token = Some(token);
                } else {
                    return Err(format!(
                        "providers.discord.bot_token must be a string, got '{}'",
                        value
                    )
                    .into());
                }
            }
            ["dispatch", "ci_batch_window_secs"] => {
                if let toml::Value::Integer(v) = parsed {
                    self.dispatch.ci_batch_window_secs = v as u64;
                } else {
                    return Err(format!(
                        "dispatch.ci_batch_window_secs must be an integer, got '{}'",
                        value
                    )
                    .into());
                }
            }
            ["dispatch", "routine_batch_window_secs"] => {
                if let toml::Value::Integer(v) = parsed {
                    self.dispatch.routine_batch_window_secs = v as u64;
                } else {
                    return Err(format!(
                        "dispatch.routine_batch_window_secs must be an integer, got '{}'",
                        value
                    )
                    .into());
                }
            }
            _ => {
                return Err(format!(
                    "unsupported config key '{}' (supported: daemon.port, daemon.bind_host, daemon.base_url, defaults.channel, defaults.format, providers.discord.bot_token, dispatch.ci_batch_window_secs, dispatch.routine_batch_window_secs)",
                    key
                )
                .into());
            }
        }

        Ok(())
    }

    pub fn scaffold_webhook_quickstart(&mut self, webhook: String) -> Result<()> {
        let webhook = normalize_text(Some(webhook)).ok_or_else(|| {
            "setup requires a non-empty webhook URL when --webhook is supplied".to_string()
        })?;

        let matches = self
            .routes
            .iter()
            .enumerate()
            .filter(|(_, route)| is_canonical_quickstart_route(route))
            .map(|(index, _)| index)
            .collect::<Vec<_>>();

        match matches.as_slice() {
            [] => {
                self.routes.push(RouteRule {
                    event: "*".to_string(),
                    filter: BTreeMap::new(),
                    sink: default_sink_name(),
                    channel: None,
                    channel_name: None,
                    webhook: Some(webhook),
                    slack_webhook: None,
                    mention: None,
                    allow_dynamic_tokens: false,
                    format: None,
                    template: None,
                });
                Ok(())
            }
            [index] => {
                self.routes[*index].webhook = Some(webhook);
                Ok(())
            }
            _ => Err(
                "multiple canonical quickstart routes found; clean up manual config before updating the webhook quickstart route"
                    .into(),
            ),
        }
    }

    /// Scaffold or update a repo→channel route with a binding-verify hint.
    ///
    /// Creates a `[[routes]]` entry shaped as:
    ///
    /// ```toml
    /// [[routes]]
    /// event = "*"
    /// filter = { repo = "<repo>" }
    /// sink = "discord"
    /// channel = "<channel_id>"
    /// channel_name = "<live_name>"  # hint, used by verify-bindings
    /// ```
    ///
    /// If an existing route matches the exact `(event="*", filter={repo=...},
    /// sink="discord")` shape, its channel and channel_name are updated in place
    /// instead of appending a duplicate.
    pub fn apply_repo_binding(
        &mut self,
        repo: &str,
        channel_id: &str,
        channel_name: Option<&str>,
    ) -> Result<()> {
        let repo = normalize_text(Some(repo.to_string()))
            .ok_or_else(|| "repo binding requires a non-empty repo name".to_string())?;
        let channel_id = normalize_text(Some(channel_id.to_string()))
            .ok_or_else(|| "repo binding requires a non-empty channel id".to_string())?;
        let channel_name = channel_name.and_then(|value| normalize_text(Some(value.to_string())));

        let existing = self
            .routes
            .iter_mut()
            .find(|route| is_repo_binding_route(route, &repo));

        match existing {
            Some(route) => {
                route.channel = Some(channel_id);
                route.channel_name = channel_name;
                route.webhook = None;
            }
            None => {
                let mut filter = BTreeMap::new();
                filter.insert("repo".to_string(), repo);
                self.routes.push(RouteRule {
                    event: "*".to_string(),
                    filter,
                    sink: default_sink_name(),
                    channel: Some(channel_id),
                    channel_name,
                    webhook: None,
                    slack_webhook: None,
                    mention: None,
                    allow_dynamic_tokens: false,
                    format: None,
                    template: None,
                });
            }
        }
        Ok(())
    }

    pub fn set_discord_bot_token(&mut self, bot_token: String) {
        self.providers.discord.bot_token = normalize_secret(Some(bot_token));
    }

    pub fn set_default_channel(&mut self, channel: String) {
        self.defaults.channel = normalize_text(Some(channel));
    }

    pub fn set_default_format(&mut self, format: MessageFormat) {
        self.defaults.format = format;
    }

    pub fn set_daemon_base_url(&mut self, base_url: String) {
        self.daemon.base_url = normalize_text(Some(base_url)).unwrap_or_else(default_base_url);
    }

    fn canonical_quickstart_webhook(&self) -> Option<&str> {
        self.routes
            .iter()
            .find(|route| is_canonical_quickstart_route(route))
            .and_then(|route| route.webhook.as_deref())
    }

    pub fn daemon_base_url(&self) -> String {
        env_var_or_fallback("HERMIP_DAEMON_URL", "CLAWHIP_DAEMON_URL")
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| self.daemon.base_url.clone())
    }

    pub fn monitor_github_token(&self) -> Option<String> {
        env_var_or_fallback("HERMIP_GITHUB_TOKEN", "CLAWHIP_GITHUB_TOKEN")
            .filter(|value| !value.trim().is_empty())
            .or_else(|| self.monitors.github_token.clone())
    }

    pub fn run_interactive_editor(&mut self, path: &Path) -> Result<()> {
        println!("hermip config editor");
        println!("Path: {}", path.display());
        println!();
        loop {
            self.print_summary();
            println!("Choose an action:");
            for (index, item) in CONFIG_EDITOR_MENU_ITEMS.iter().enumerate() {
                println!("  {}) {}", index + 1, item);
            }
            match prompt("Selection")?.trim() {
                "1" => self.set_discord_bot_token(prompt("Bot token")?),
                "2" => self.set_daemon_base_url(prompt_with_default(
                    "Daemon base URL",
                    Some(&self.daemon.base_url),
                )?),
                "3" => self.set_default_channel(prompt("Default channel")?),
                "4" => self.set_default_format(prompt_format(Some(self.defaults.format.clone()))?),
                "5" => {
                    let webhook = prompt_with_default(
                        "Discord webhook quickstart route",
                        self.canonical_quickstart_webhook(),
                    )?;
                    self.scaffold_webhook_quickstart(webhook)?;
                }
                "6" => {
                    self.save(path)?;
                    println!("Saved {}", path.display());
                    break;
                }
                "7" => {
                    println!("Discarded changes.");
                    break;
                }
                "8" => self.print_template_hint(),
                _ => println!("Unknown selection."),
            }
            println!();
        }
        Ok(())
    }

    fn print_summary(&self) {
        println!("Current config summary:");
        println!("  Discord token source: {}", self.discord_token_source());
        println!("  Daemon base URL: {}", self.daemon.base_url);
        println!(
            "  Bind host/port: {}:{}",
            self.daemon.bind_host, self.daemon.port
        );
        println!("  CI batch window: {}s", self.dispatch.ci_batch_window_secs);
        println!(
            "  Routine batch window: {}",
            self.dispatch
                .routine_batch_window()
                .map(|window| format!("{}s", window.as_secs()))
                .unwrap_or_else(|| "disabled".to_string())
        );
        println!(
            "  Default channel: {}",
            self.defaults.channel.as_deref().unwrap_or("<unset>")
        );
        println!("  Webhook routes: {}", self.routes_with_webhooks());
        println!("  Default format: {}", self.defaults.format.as_str());
        println!("  Routes: {}", self.routes.len());
        println!("  Git monitors: {}", self.monitors.git.repos.len());
        println!("  Tmux monitors: {}", self.monitors.tmux.sessions.len());
        println!("  Workspace monitors: {}", self.monitors.workspace.len());
        println!("  Cron jobs: {}", self.cron.jobs.len());
    }

    fn print_template_hint(&self) {
        println!("Advanced routes and monitors are still edited manually in the config file.");
        println!(
            "Sections: [providers.discord], [dispatch], [daemon], [cron], [[cron.jobs]], [[routes]], [[monitors.git.repos]], [[monitors.tmux.sessions]], [[monitors.workspace]]"
        );
        println!(
            "Routes may set either channel = \"...\" or webhook = \"https://discord.com/api/webhooks/...\"."
        );
        println!(
            r#"Webhook example: [[routes]] event = "tmux.keyword" webhook = "https://discord.com/api/webhooks/...""#
        );
    }

    fn normalize(&mut self) {
        self.discord.bot_token = normalize_secret(self.discord.bot_token.clone());
        self.discord.legacy_default_channel =
            normalize_text(self.discord.legacy_default_channel.clone());
        self.providers.discord.bot_token =
            normalize_secret(self.providers.discord.bot_token.clone());
        self.providers.discord.legacy_default_channel =
            normalize_text(self.providers.discord.legacy_default_channel.clone());
        self.defaults.channel = normalize_text(self.defaults.channel.clone());
        self.monitors.github_token = normalize_secret(self.monitors.github_token.clone());

        for route in &mut self.routes {
            route.sink = normalize_text(Some(route.sink.clone())).unwrap_or_else(default_sink_name);
            route.channel = normalize_text(route.channel.clone());
            route.channel_name = normalize_text(route.channel_name.clone());
            route.webhook = normalize_text(route.webhook.clone());
            route.slack_webhook = normalize_text(route.slack_webhook.clone());
            route.mention = normalize_text(route.mention.clone());
            route.template = normalize_text(route.template.clone());
        }

        for repo in &mut self.monitors.git.repos {
            repo.channel = normalize_text(repo.channel.clone());
            repo.channel_name = normalize_text(repo.channel_name.clone());
            repo.mention = normalize_text(repo.mention.clone());
            repo.name = normalize_text(repo.name.clone());
            repo.github_repo = normalize_text(repo.github_repo.clone());
        }

        for session in &mut self.monitors.tmux.sessions {
            session.channel = normalize_text(session.channel.clone());
            session.channel_name = normalize_text(session.channel_name.clone());
            session.mention = normalize_text(session.mention.clone());
        }

        for workspace in &mut self.monitors.workspace {
            workspace.path = normalize_text(Some(workspace.path.clone())).unwrap_or_default();
            workspace.channel = normalize_text(workspace.channel.clone());
            workspace.mention = normalize_text(workspace.mention.clone());
            workspace.watch_dirs = workspace
                .watch_dirs
                .iter()
                .filter_map(|dir| normalize_text(Some(dir.clone())))
                .collect();
            if workspace.watch_dirs.is_empty() {
                workspace.watch_dirs = default_workspace_watch_dirs();
            }
            workspace.events = workspace
                .events
                .iter()
                .filter_map(|event| normalize_text(Some(event.clone())))
                .collect();
            workspace.debounce_ms = workspace.debounce_ms.max(1);
            workspace.poll_interval_secs = workspace.poll_interval_secs.map(|secs| secs.max(1));
        }

        for job in &mut self.cron.jobs {
            job.id = normalize_text(Some(job.id.clone())).unwrap_or_default();
            job.schedule = normalize_text(Some(job.schedule.clone())).unwrap_or_default();
            job.timezone =
                normalize_text(Some(job.timezone.clone())).unwrap_or_else(default_cron_timezone);
            job.channel = normalize_text(job.channel.clone());
            job.mention = normalize_text(job.mention.clone());
            match &mut job.kind {
                CronJobKind::CustomMessage { message } => {
                    *message = normalize_text(Some(message.clone())).unwrap_or_default();
                }
            }
        }
    }

    fn routes_with_webhooks(&self) -> usize {
        self.routes
            .iter()
            .filter(|route| route.has_any_webhook_target())
            .count()
    }
}

fn is_repo_binding_route(route: &RouteRule, repo: &str) -> bool {
    route.event == "*"
        && route.sink.trim() == "discord"
        && route.slack_webhook.is_none()
        && route.filter.len() == 1
        && route
            .filter
            .get("repo")
            .map(|value| value == repo)
            .unwrap_or(false)
}

fn is_canonical_quickstart_route(route: &RouteRule) -> bool {
    route.event == "*"
        && route.filter.is_empty()
        && route.sink.trim() == "discord"
        && route.channel.is_none()
        && route.slack_webhook.is_none()
        && route.mention.is_none()
        && route.template.is_none()
        && !route.allow_dynamic_tokens
        && route.format.is_none()
}

fn prompt(label: &str) -> Result<String> {
    print!("{label}: ");
    io::stdout().flush()?;
    let mut value = String::new();
    io::stdin().read_line(&mut value)?;
    Ok(value.trim_end().to_string())
}

fn prompt_with_default(label: &str, default: Option<&str>) -> Result<String> {
    let value = match default {
        Some(default) => prompt(&format!("{label} [{default}]"))?,
        None => prompt(label)?,
    };

    if value.trim().is_empty() {
        Ok(default.unwrap_or_default().to_string())
    } else {
        Ok(value)
    }
}

fn prompt_format(default: Option<MessageFormat>) -> Result<MessageFormat> {
    let default_value = default.unwrap_or(MessageFormat::Compact);
    let input = prompt(&format!(
        "Format [{}] (compact/alert/inline/raw)",
        default_value.as_str()
    ))?;
    if input.trim().is_empty() {
        return Ok(default_value);
    }
    MessageFormat::from_label(input.trim())
}

fn normalize_text(value: Option<String>) -> Option<String> {
    value.and_then(|value| {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discord_token_source_prefers_env_over_config() {
        let mut config = AppConfig::default();
        config.providers.discord.bot_token = Some("config-token".into());

        assert_eq!(config.discord_token_source_with(|_| None), "config");
        assert_eq!(
            config.effective_token_with(|_| None).as_deref(),
            Some("config-token")
        );

        let token = config.effective_token_with(|name| {
            (name == "DISCORD_TOKEN").then(|| "env-token".to_string())
        });
        assert_eq!(token.as_deref(), Some("env-token"));
        assert_eq!(
            config.discord_token_source_with(|name| {
                (name == "DISCORD_TOKEN").then(|| "env-token".to_string())
            }),
            "env"
        );
    }

    #[test]
    fn discord_token_source_reports_missing_when_unset() {
        let config = AppConfig::default();

        assert_eq!(config.discord_token_source_with(|_| None), "missing");
        assert_eq!(config.effective_token_with(|_| None), None);
    }

    #[test]
    fn hermip_env_token_is_preferred_over_config() {
        let config = AppConfig::default();

        // When both HERMIP_ and config are set, HERMIP_ wins.
        let token = config.effective_token_with(|name| match name {
            "HERMIP_DISCORD_BOT_TOKEN" => Some("hermip-token".to_string()),
            _ => None,
        });
        assert_eq!(token.as_deref(), Some("hermip-token"));
    }

    #[test]
    fn provider_discord_token_is_used_when_present() {
        let mut config = AppConfig::default();
        config.providers.discord.bot_token = Some("config-token".into());

        assert_eq!(config.discord_token_source_with(|_| None), "config");
        assert_eq!(
            config.effective_token_with(|_| None).as_deref(),
            Some("config-token")
        );
    }

    #[test]
    fn load_or_default_migrates_legacy_discord_to_providers() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        fs::write(
            &path,
            "[discord]\ntoken = \"legacy-token\"\ndefault_channel = \"123\"\n",
        )
        .unwrap();

        let config = AppConfig::load_or_default(&path).unwrap();

        assert_eq!(
            config.providers.discord.bot_token.as_deref(),
            Some("legacy-token")
        );
        assert_eq!(
            config.providers.discord.legacy_default_channel.as_deref(),
            Some("123")
        );
        assert!(config.discord.is_empty());
        assert_eq!(config.defaults.channel.as_deref(), Some("123"));
    }

    #[test]
    fn load_or_default_rejects_conflicting_legacy_and_provider_discord() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        fs::write(
            &path,
            "[discord]\ntoken = \"legacy-token\"\n[providers.discord]\ntoken = \"provider-token\"\n",
        )
        .unwrap();

        let error = AppConfig::load_or_default(&path).unwrap_err().to_string();

        assert!(error.contains("conflicting legacy [discord].token"));
    }

    #[test]
    fn webhook_route_satisfies_delivery_validation_without_bot_token() {
        let config = AppConfig {
            routes: vec![RouteRule {
                event: "tmux.keyword".into(),
                webhook: Some("https://discord.com/api/webhooks/123/abc".into()),
                ..RouteRule::default()
            }],
            ..AppConfig::default()
        };

        assert!(config.validate().is_ok());
    }

    #[test]
    fn slack_webhook_route_satisfies_delivery_validation_without_bot_token() {
        let config = AppConfig {
            routes: vec![RouteRule {
                event: "tmux.keyword".into(),
                slack_webhook: Some("https://hooks.slack.com/services/T/B/abc".into()),
                ..RouteRule::default()
            }],
            ..AppConfig::default()
        };

        assert!(config.validate().is_ok());
        assert_eq!(config.webhook_route_count(), 1);
    }

    #[test]
    fn route_cannot_set_channel_and_webhook() {
        let config = AppConfig {
            providers: ProvidersConfig {
                discord: DiscordConfig {
                    bot_token: Some("token".into()),
                    legacy_default_channel: None,
                },
                slack: SlackConfig::default(),
            },
            routes: vec![RouteRule {
                event: "tmux.keyword".into(),
                sink: default_sink_name(),
                channel: Some("123".into()),
                webhook: Some("https://discord.com/api/webhooks/123/abc".into()),
                slack_webhook: None,
                ..RouteRule::default()
            }],
            ..AppConfig::default()
        };

        let error = config.validate().unwrap_err().to_string();
        assert!(error.contains("cannot set both channel and webhook"));
    }

    #[test]
    fn slack_route_cannot_set_channel() {
        let config = AppConfig {
            routes: vec![RouteRule {
                event: "tmux.keyword".into(),
                sink: "slack".into(),
                channel: Some("123".into()),
                webhook: Some("https://hooks.slack.com/services/T/B/abc".into()),
                ..RouteRule::default()
            }],
            ..AppConfig::default()
        };

        let error = config.validate().unwrap_err().to_string();
        assert!(error.contains("cannot set channel when sink = \"slack\""));
    }

    #[test]
    fn slack_route_can_use_generic_webhook_field() {
        let config = AppConfig {
            routes: vec![RouteRule {
                event: "tmux.keyword".into(),
                sink: "slack".into(),
                webhook: Some("https://hooks.slack.com/services/T/B/abc".into()),
                ..RouteRule::default()
            }],
            ..AppConfig::default()
        };

        assert!(config.validate().is_ok());
        assert_eq!(config.webhook_route_count(), 1);
    }

    #[test]
    fn setup_scaffold_adds_canonical_quickstart_route() {
        let mut config = AppConfig::default();
        config
            .scaffold_webhook_quickstart(" https://discord.com/api/webhooks/123/abc ".into())
            .unwrap();

        assert_eq!(config.routes.len(), 1);
        assert_eq!(config.routes[0].event, "*");
        assert_eq!(
            config.routes[0].webhook.as_deref(),
            Some("https://discord.com/api/webhooks/123/abc")
        );
        assert_eq!(config.routes[0].sink, "discord");
        assert_eq!(config.routes[0].channel, None);
    }

    #[test]
    fn setup_mixed_flag_edits_update_only_owned_nodes() {
        let mut config = AppConfig {
            providers: ProvidersConfig {
                discord: DiscordConfig {
                    bot_token: Some("old-token".into()),
                    legacy_default_channel: None,
                },
                slack: SlackConfig::default(),
            },
            daemon: DaemonConfig {
                base_url: "http://127.0.0.1:25294".into(),
                ..DaemonConfig::default()
            },
            defaults: DefaultsConfig {
                channel: Some("general".into()),
                channel_name: None,
                format: MessageFormat::Compact,
            },
            routes: vec![RouteRule {
                event: "git.commit".into(),
                channel: Some("eng".into()),
                ..RouteRule::default()
            }],
            monitors: MonitorConfig {
                github_token: Some("gh-token".into()),
                ..MonitorConfig::default()
            },
            ..AppConfig::default()
        };

        config
            .apply_setup_edits(SetupEdits {
                webhook: Some("https://discord.com/api/webhooks/123/new".into()),
                bot_token: Some("new-token".into()),
                default_channel: Some("alerts".into()),
                default_format: Some(MessageFormat::Alert),
                daemon_base_url: Some("http://127.0.0.1:9999".into()),
            })
            .unwrap();

        assert_eq!(
            config.providers.discord.bot_token.as_deref(),
            Some("new-token")
        );
        assert_eq!(config.defaults.channel.as_deref(), Some("alerts"));
        assert_eq!(config.defaults.format, MessageFormat::Alert);
        assert_eq!(config.daemon.base_url, "http://127.0.0.1:9999");
        assert_eq!(config.routes.len(), 2);
        assert_eq!(config.routes[0].event, "git.commit");
        assert_eq!(config.routes[0].channel.as_deref(), Some("eng"));
        assert_eq!(config.monitors.github_token.as_deref(), Some("gh-token"));
        assert_eq!(
            config.routes[1].webhook.as_deref(),
            Some("https://discord.com/api/webhooks/123/new")
        );
    }

    #[test]
    fn setup_non_webhook_edits_do_not_touch_routes() {
        let mut config = AppConfig {
            routes: vec![RouteRule {
                event: "tmux.keyword".into(),
                webhook: Some("https://discord.com/api/webhooks/123/original".into()),
                mention: Some("<@1>".into()),
                ..RouteRule::default()
            }],
            ..AppConfig::default()
        };

        config
            .apply_setup_edits(SetupEdits {
                bot_token: Some("discord-token".into()),
                default_channel: Some("alerts".into()),
                default_format: Some(MessageFormat::Raw),
                daemon_base_url: Some("http://127.0.0.1:4444".into()),
                ..SetupEdits::default()
            })
            .unwrap();

        assert_eq!(config.routes.len(), 1);
        assert_eq!(config.routes[0].event, "tmux.keyword");
        assert_eq!(
            config.routes[0].webhook.as_deref(),
            Some("https://discord.com/api/webhooks/123/original")
        );
        assert_eq!(config.routes[0].mention.as_deref(), Some("<@1>"));
    }

    #[test]
    fn setup_webhook_rerun_updates_only_canonical_quickstart_route() {
        let mut config = AppConfig {
            routes: vec![
                RouteRule {
                    event: "*".into(),
                    webhook: Some("https://discord.com/api/webhooks/123/old".into()),
                    ..RouteRule::default()
                },
                RouteRule {
                    event: "git.commit".into(),
                    webhook: Some("https://discord.com/api/webhooks/123/other".into()),
                    mention: Some("<@1>".into()),
                    ..RouteRule::default()
                },
            ],
            ..AppConfig::default()
        };

        config
            .scaffold_webhook_quickstart("https://discord.com/api/webhooks/123/new".into())
            .unwrap();

        assert_eq!(config.routes.len(), 2);
        assert_eq!(
            config.routes[0].webhook.as_deref(),
            Some("https://discord.com/api/webhooks/123/new")
        );
        assert_eq!(
            config.routes[1].webhook.as_deref(),
            Some("https://discord.com/api/webhooks/123/other")
        );
    }

    #[test]
    fn ambiguous_quickstart_routes_fail_without_mutating_config() {
        let mut config = AppConfig {
            routes: vec![
                RouteRule {
                    event: "*".into(),
                    webhook: Some("https://discord.com/api/webhooks/123/a".into()),
                    ..RouteRule::default()
                },
                RouteRule {
                    event: "*".into(),
                    webhook: Some("https://discord.com/api/webhooks/123/b".into()),
                    ..RouteRule::default()
                },
            ],
            ..AppConfig::default()
        };

        let error = config
            .scaffold_webhook_quickstart("https://discord.com/api/webhooks/123/new".into())
            .unwrap_err()
            .to_string();

        assert!(error.contains("multiple canonical quickstart routes"));
        assert_eq!(config.routes.len(), 2);
        assert_eq!(
            config.routes[0].webhook.as_deref(),
            Some("https://discord.com/api/webhooks/123/a")
        );
        assert_eq!(
            config.routes[1].webhook.as_deref(),
            Some("https://discord.com/api/webhooks/123/b")
        );
    }

    #[test]
    fn setup_edits_require_at_least_one_non_empty_value() {
        let mut config = AppConfig::default();

        let error = config
            .apply_setup_edits(SetupEdits {
                webhook: Some("   ".into()),
                bot_token: Some(" ".into()),
                default_channel: Some(" ".into()),
                daemon_base_url: Some(" ".into()),
                ..SetupEdits::default()
            })
            .unwrap_err()
            .to_string();

        assert!(error.contains("at least one non-empty setup flag"));
    }

    #[test]
    fn config_editor_menu_matches_bounded_preset_contract() {
        assert_eq!(
            CONFIG_EDITOR_MENU_ITEMS,
            [
                "Set Discord bot token",
                "Set daemon base URL",
                "Set default channel",
                "Set default format",
                "Set Discord webhook quickstart route",
                "Save and exit",
                "Exit without saving",
                "Print manual config template hint",
            ]
        );
    }

    #[test]
    fn tmux_session_monitor_defaults_keyword_window_to_thirty_seconds() {
        let session = TmuxSessionMonitor::default();
        assert_eq!(session.keyword_window_secs, 30);
    }

    #[test]
    fn dispatch_config_defaults_ci_batch_window_to_thirty_seconds() {
        let config = AppConfig::default();
        assert_eq!(config.dispatch.ci_batch_window_secs, 30);
    }

    #[test]
    fn dispatch_config_defaults_routine_batch_window_to_five_seconds() {
        let config = AppConfig::default();
        assert_eq!(config.dispatch.routine_batch_window_secs, 5);
        assert_eq!(
            config.dispatch.routine_batch_window(),
            Some(Duration::from_secs(5))
        );
    }

    #[test]
    fn cron_config_defaults_are_backward_compatible() {
        let config = AppConfig::default();
        assert_eq!(config.cron.poll_interval_secs, 30);
        assert!(config.cron.jobs.is_empty());
    }

    #[test]
    fn load_or_default_parses_dispatch_ci_batch_window_secs() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        fs::write(
            &path,
            "[providers.discord]\ntoken = \"abc\"\n[dispatch]\nci_batch_window_secs = 90\n",
        )
        .unwrap();

        let config = AppConfig::load_or_default(&path).unwrap();

        assert_eq!(config.dispatch.ci_batch_window_secs, 90);
        assert!(config.validate().is_ok());
    }

    #[test]
    fn load_or_default_parses_dispatch_routine_batch_window_secs() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        fs::write(
            &path,
            "[providers.discord]\ntoken = \"abc\"\n[dispatch]\nroutine_batch_window_secs = 9\n",
        )
        .unwrap();

        let config = AppConfig::load_or_default(&path).unwrap();

        assert_eq!(config.dispatch.routine_batch_window_secs, 9);
        assert_eq!(
            config.dispatch.routine_batch_window(),
            Some(Duration::from_secs(9))
        );
        assert!(config.validate().is_ok());
    }

    #[test]
    fn load_or_default_defaults_dispatch_ci_batch_window_when_omitted() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        fs::write(&path, "[providers.discord]\ntoken = \"abc\"\n").unwrap();

        let config = AppConfig::load_or_default(&path).unwrap();

        assert_eq!(config.dispatch.ci_batch_window_secs, 30);
        assert!(config.validate().is_ok());
    }

    #[test]
    fn load_or_default_defaults_routine_batch_window_when_omitted() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        fs::write(&path, "[providers.discord]\ntoken = \"abc\"\n").unwrap();

        let config = AppConfig::load_or_default(&path).unwrap();

        assert_eq!(config.dispatch.routine_batch_window_secs, 5);
        assert_eq!(
            config.dispatch.routine_batch_window(),
            Some(Duration::from_secs(5))
        );
        assert!(config.validate().is_ok());
    }

    #[test]
    fn load_or_default_preserves_zero_dispatch_ci_batch_window_secs_until_validation() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        fs::write(
            &path,
            "[providers.discord]\ntoken = \"abc\"\n[dispatch]\nci_batch_window_secs = 0\n",
        )
        .unwrap();

        let config = AppConfig::load_or_default(&path).unwrap();
        assert_eq!(config.dispatch.ci_batch_window_secs, 0);
        let error = config.validate().unwrap_err().to_string();
        assert!(error.contains("dispatch.ci_batch_window_secs must be at least 1"));
    }

    #[test]
    fn load_or_default_allows_zero_dispatch_routine_batch_window_secs_to_disable_batching() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        fs::write(
            &path,
            "[providers.discord]\ntoken = \"abc\"\n[dispatch]\nroutine_batch_window_secs = 0\n",
        )
        .unwrap();

        let config = AppConfig::load_or_default(&path).unwrap();
        assert_eq!(config.dispatch.routine_batch_window_secs, 0);
        assert_eq!(config.dispatch.routine_batch_window(), None);
        assert!(config.validate().is_ok());
    }

    #[test]
    fn load_or_default_parses_cron_jobs() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        fs::write(
            &path,
            r#"[providers.discord]
token = "abc"

[cron]
poll_interval_secs = 15

[[cron.jobs]]
id = "dev-followup"
schedule = "*/30 * * * *"
channel = "ops"
mention = " <@1> "
kind = "custom-message"
message = " ping "
"#,
        )
        .unwrap();

        let config = AppConfig::load_or_default(&path).unwrap();

        assert_eq!(config.cron.poll_interval_secs, 15);
        assert_eq!(config.cron.jobs.len(), 1);
        let job = &config.cron.jobs[0];
        assert_eq!(job.id, "dev-followup");
        assert_eq!(job.schedule, "*/30 * * * *");
        assert_eq!(job.channel.as_deref(), Some("ops"));
        assert_eq!(job.mention.as_deref(), Some("<@1>"));
        assert_eq!(job.timezone, "UTC");
        match &job.kind {
            CronJobKind::CustomMessage { message } => assert_eq!(message, "ping"),
        }
        assert!(config.validate().is_ok());
    }

    #[test]
    fn cron_validation_rejects_duplicate_ids() {
        let config = AppConfig {
            providers: ProvidersConfig {
                discord: DiscordConfig {
                    bot_token: Some("token".into()),
                    legacy_default_channel: None,
                },
                slack: SlackConfig::default(),
            },
            cron: CronConfig {
                poll_interval_secs: 30,
                jobs: vec![
                    CronJob {
                        id: "dup".into(),
                        schedule: "*/5 * * * *".into(),
                        timezone: "UTC".into(),
                        enabled: true,
                        channel: Some("ops".into()),
                        mention: None,
                        format: None,
                        state_file: None,
                        kind: CronJobKind::CustomMessage {
                            message: "first".into(),
                        },
                    },
                    CronJob {
                        id: "dup".into(),
                        schedule: "0 * * * *".into(),
                        timezone: "UTC".into(),
                        enabled: true,
                        channel: Some("ops".into()),
                        mention: None,
                        format: None,
                        state_file: None,
                        kind: CronJobKind::CustomMessage {
                            message: "second".into(),
                        },
                    },
                ],
            },
            ..AppConfig::default()
        };

        let error = config.validate().unwrap_err().to_string();
        assert!(error.contains("duplicate cron job id 'dup'"));
    }

    #[test]
    fn workspace_monitor_defaults_are_backward_compatible() {
        let config: AppConfig = toml::from_str(
            "
[providers.discord]
token = 'discord-token'

[[monitors.workspace]]
path = '/tmp/repo'
",
        )
        .unwrap();

        assert_eq!(config.monitors.workspace.len(), 1);
        let monitor = &config.monitors.workspace[0];
        assert_eq!(monitor.watch_dirs, default_workspace_watch_dirs());
        assert_eq!(monitor.debounce_ms, default_workspace_debounce_ms());
        assert_eq!(monitor.poll_interval_secs, None);
        assert!(!monitor.discover_worktrees);
    }

    #[test]
    fn normalize_trims_workspace_monitor_fields() {
        let mut config = AppConfig::default();
        config.monitors.workspace.push(WorkspaceMonitor {
            path: " /tmp/repo ".into(),
            watch_dirs: vec![" .omx/state ".into(), "".into(), " .omc/state ".into()],
            discover_worktrees: true,
            channel: Some(" 123 ".into()),
            mention: Some(" <@1> ".into()),
            format: Some(MessageFormat::Compact),
            events: vec!["workspace.*".into()],
            poll_interval_secs: Some(5),
            debounce_ms: 2000,
        });

        config.normalize();
        let monitor = &config.monitors.workspace[0];
        assert_eq!(monitor.path, "/tmp/repo");
        assert_eq!(monitor.watch_dirs, vec![".omx/state", ".omc/state"]);
        assert_eq!(monitor.channel.as_deref(), Some("123"));
        assert_eq!(monitor.mention.as_deref(), Some("<@1>"));
    }

    #[test]
    fn workspace_monitor_config_parses_and_normalizes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            format!(
                r#"[providers.discord]
token = "abc"

[[monitors.workspace]]
path = " {} "
watch_dirs = [" .omx/state ", " .omc/state "]
channel = " ops "
mention = " <@1> "
discover_worktrees = true
events = [" workspace.skill.* "]
debounce_ms = 1500
poll_interval_secs = 9
"#,
                dir.path().display()
            ),
        )
        .unwrap();

        let config = AppConfig::load_or_default(&path).unwrap();
        let monitor = &config.monitors.workspace[0];
        assert_eq!(monitor.path, dir.path().display().to_string());
        assert_eq!(monitor.watch_dirs, vec![".omx/state", ".omc/state"]);
        assert_eq!(monitor.channel.as_deref(), Some("ops"));
        assert_eq!(monitor.mention.as_deref(), Some("<@1>"));
        assert!(monitor.discover_worktrees);
        assert_eq!(monitor.events, vec!["workspace.skill.*"]);
        assert_eq!(monitor.debounce_ms, 1500);
        assert_eq!(monitor.poll_interval_secs, Some(9));
    }

    #[test]
    fn config_without_workspace_monitor_still_loads() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[providers.discord]\ntoken = \"abc\"\n").unwrap();

        let config = AppConfig::load_or_default(&path).unwrap();
        assert!(config.monitors.workspace.is_empty());
        assert!(config.validate().is_ok());
    }

    // ---------------------------------------------------------------------------
    // VAL-CONFIG-001: Config loads from hermip.toml in current directory
    // ---------------------------------------------------------------------------
    #[test]
    fn config_loads_from_hermip_toml_in_cwd() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("hermip.toml");

        std::fs::write(
            &path,
            r#"[daemon]
port = 25294
"#,
        )
        .unwrap();

        let config = AppConfig::load_or_default(&path).unwrap();
        assert_eq!(config.daemon.port, 25294);
    }

    // ---------------------------------------------------------------------------
    // VAL-CONFIG-004: Invalid TOML produces clear parse error with file path and line
    // ---------------------------------------------------------------------------
    #[test]
    fn invalid_toml_produces_error_with_file_path() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("hermip.toml");

        // TOML syntax error: unclosed table bracket
        std::fs::write(&path, "[daemon\nport = ").unwrap();

        let result = AppConfig::load_or_default(&path);
        let error = result.expect_err("should fail on malformed TOML");
        let error_msg = error.to_string();

        // Error should reference the file path.
        assert!(
            error_msg.contains("hermip.toml") || error_msg.contains(" hermip.toml"),
            "error should mention the file path, got: {error_msg}"
        );
    }

    // ---------------------------------------------------------------------------
    // VAL-CONFIG-005: Missing optional sections use sensible defaults
    // ---------------------------------------------------------------------------
    #[test]
    fn minimal_daemon_only_config_uses_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("hermip.toml");

        std::fs::write(&path, "[daemon]\nport = 25294\n").unwrap();

        let config = AppConfig::load_or_default(&path).unwrap();

        assert_eq!(config.daemon.port, 25294);
        assert_eq!(
            config.defaults.format,
            crate::events::MessageFormat::Compact
        );
        assert!(config.routes.is_empty());
        assert!(config.monitors.workspace.is_empty());
        assert!(config.cron.jobs.is_empty());
    }

    // ---------------------------------------------------------------------------
    // VAL-CONFIG-007: Legacy [discord] format still works
    // ---------------------------------------------------------------------------
    #[test]
    fn legacy_discord_format_still_works() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("hermip.toml");

        // Old ClawHip format.
        std::fs::write(
            &path,
            r#"[discord]
token = "legacy-discord-token"
default_channel = "legacy-default-channel"
"#,
        )
        .unwrap();

        let config = AppConfig::load_or_default(&path).unwrap();

        // Legacy values should be migrated to providers.discord.
        assert_eq!(
            config.providers.discord.bot_token.as_deref(),
            Some("legacy-discord-token")
        );
        assert_eq!(
            config.defaults.channel.as_deref(),
            Some("legacy-default-channel")
        );
        // The legacy [discord] struct should be cleared.
        assert!(config.discord.bot_token.is_none());
    }

    // ---------------------------------------------------------------------------
    // Secrets masking: to_display_toml masks all secret fields
    // ---------------------------------------------------------------------------

    fn config_with_all_secrets() -> AppConfig {
        AppConfig {
            discord: DiscordConfig {
                bot_token: Some("super-secret-legacy-token".into()),
                legacy_default_channel: None,
            },
            providers: ProvidersConfig {
                discord: DiscordConfig {
                    bot_token: Some("super-secret-bot-token".into()),
                    legacy_default_channel: Some("123456789".into()),
                },
                slack: SlackConfig::default(),
            },
            daemon: DaemonConfig {
                bind_host: "0.0.0.0".into(),
                port: 25294,
                base_url: "http://127.0.0.1:25294".into(),
            },
            dispatch: DispatchConfig::default(),
            defaults: DefaultsConfig {
                channel: Some("default-channel".into()),
                channel_name: Some("general".into()),
                format: MessageFormat::Compact,
            },
            routes: vec![
                RouteRule {
                    event: "git.commit".into(),
                    sink: "discord".into(),
                    channel: Some("123456".into()),
                    webhook: Some(
                        "https://discord.com/api/webhooks/111/secret-webhook-token".into(),
                    ),
                    slack_webhook: None,
                    mention: None,
                    allow_dynamic_tokens: false,
                    format: None,
                    template: None,
                    channel_name: None,
                    filter: BTreeMap::new(),
                },
                RouteRule {
                    event: "tmux.keyword".into(),
                    sink: "slack".into(),
                    channel: None,
                    webhook: None,
                    slack_webhook: Some(
                        "https://hooks.slack.com/services/T/B/secret-slack-hook".into(),
                    ),
                    mention: None,
                    allow_dynamic_tokens: false,
                    format: None,
                    template: None,
                    channel_name: None,
                    filter: BTreeMap::new(),
                },
                RouteRule {
                    event: "mission.started".into(),
                    sink: "discord".into(),
                    channel: Some("789012".into()),
                    webhook: None,
                    slack_webhook: None,
                    mention: None,
                    allow_dynamic_tokens: false,
                    format: None,
                    template: None,
                    channel_name: None,
                    filter: BTreeMap::new(),
                },
            ],
            monitors: MonitorConfig {
                github_token: Some("ghp_abcdef1234567890".into()),
                ..MonitorConfig::default()
            },
            cron: CronConfig::default(),
            update: crate::update::UpdateConfig::default(),
        }
    }

    #[test]
    fn masked_config_replaces_all_secret_fields_with_mask() {
        let config = config_with_all_secrets();
        let masked = config.masked();

        // discord.bot_token (legacy) should be masked
        assert_eq!(masked.discord.bot_token.as_deref(), Some("***"));
        // providers.discord.bot_token should be masked
        assert_eq!(masked.providers.discord.bot_token.as_deref(), Some("***"));
        // monitors.github_token should be masked
        assert_eq!(masked.monitors.github_token.as_deref(), Some("***"));
        // route.webhook should be masked
        assert_eq!(masked.routes[0].webhook.as_deref(), Some("***"));
        // route.slack_webhook should be masked
        assert_eq!(masked.routes[1].slack_webhook.as_deref(), Some("***"));
    }

    #[test]
    fn masked_config_preserves_non_secret_fields() {
        let config = config_with_all_secrets();
        let masked = config.masked();

        // Non-secret fields should be unchanged
        assert_eq!(masked.daemon.port, 25294);
        assert_eq!(masked.daemon.base_url, "http://127.0.0.1:25294");
        assert_eq!(
            masked.providers.discord.legacy_default_channel.as_deref(),
            Some("123456789")
        );
        assert_eq!(masked.defaults.channel.as_deref(), Some("default-channel"));
        assert_eq!(masked.defaults.channel_name.as_deref(), Some("general"));
        assert_eq!(masked.routes[0].event, "git.commit");
        assert_eq!(masked.routes[0].channel.as_deref(), Some("123456"));
        assert_eq!(masked.routes[0].sink, "discord");
        assert_eq!(masked.routes[2].channel.as_deref(), Some("789012"));
        assert_eq!(masked.routes.len(), 3);
    }

    #[test]
    fn masked_config_does_not_add_mask_to_none_secrets() {
        let config = AppConfig::default();
        let masked = config.masked();

        // None secret fields should remain None (not become Some("***"))
        assert!(masked.discord.bot_token.is_none());
        assert!(masked.providers.discord.bot_token.is_none());
        assert!(masked.monitors.github_token.is_none());
    }

    #[test]
    fn masked_config_does_not_modify_original() {
        let config = config_with_all_secrets();
        let original_bot_token = config.providers.discord.bot_token.clone();
        let original_webhook = config.routes[0].webhook.clone();
        let original_github_token = config.monitors.github_token.clone();

        let _masked = config.masked();

        // Original config should be unchanged
        assert_eq!(
            config.providers.discord.bot_token.as_deref(),
            original_bot_token.as_deref()
        );
        assert_eq!(
            config.routes[0].webhook.as_deref(),
            original_webhook.as_deref()
        );
        assert_eq!(
            config.monitors.github_token.as_deref(),
            original_github_token.as_deref()
        );
    }

    #[test]
    fn to_display_toml_produces_masked_output() {
        let config = config_with_all_secrets();
        let display = config.to_display_toml().unwrap();

        // All secret values must be masked
        assert!(
            !display.contains("super-secret-bot-token"),
            "display TOML should not contain plaintext bot token"
        );
        assert!(
            !display.contains("super-secret-legacy-token"),
            "display TOML should not contain plaintext legacy token"
        );
        assert!(
            !display.contains("secret-webhook-token"),
            "display TOML should not contain plaintext webhook URL"
        );
        assert!(
            !display.contains("secret-slack-hook"),
            "display TOML should not contain plaintext Slack webhook URL"
        );
        assert!(
            !display.contains("ghp_abcdef1234567890"),
            "display TOML should not contain plaintext GitHub token"
        );

        // The mask placeholder must appear for each secret field that has a value
        assert!(
            display.contains("\"***\""),
            "display TOML should contain masked values"
        );

        // Non-secret values should be present
        assert!(
            display.contains("25294"),
            "display TOML should contain port"
        );
        assert!(
            display.contains("default-channel"),
            "display TOML should contain channel name"
        );
    }

    #[test]
    fn to_pretty_toml_still_contains_plaintext_secrets() {
        // Ensure to_pretty_toml (used by save()) still contains real secrets
        let config = config_with_all_secrets();
        let raw = config.to_pretty_toml().unwrap();

        assert!(
            raw.contains("super-secret-bot-token"),
            "raw TOML should contain plaintext bot token for save()"
        );
        assert!(
            raw.contains("ghp_abcdef1234567890"),
            "raw TOML should contain plaintext GitHub token for save()"
        );
    }

    #[test]
    fn is_secret_key_identifies_all_secret_keys() {
        // Exact matches for known secret keys
        assert!(AppConfig::is_secret_key("discord.token"));
        assert!(AppConfig::is_secret_key("providers.discord.bot_token"));
        assert!(AppConfig::is_secret_key("monitors.github_token"));

        // Suffix matches for webhook fields (dots required before suffix)
        assert!(AppConfig::is_secret_key("routes[0].webhook"));
        assert!(AppConfig::is_secret_key("routes[0].slack_webhook"));
        assert!(AppConfig::is_secret_key("foo.webhook"));
        assert!(AppConfig::is_secret_key("foo.slack_webhook"));

        // Non-secret keys should not match
        assert!(!AppConfig::is_secret_key("daemon.port"));
        assert!(!AppConfig::is_secret_key("daemon.base_url"));
        assert!(!AppConfig::is_secret_key("defaults.channel"));
        assert!(!AppConfig::is_secret_key("defaults.format"));
        assert!(!AppConfig::is_secret_key(
            "providers.discord.default_channel"
        ));
        // Bare field names without dot prefix are not secret key paths
        assert!(!AppConfig::is_secret_key("webhook"));
        assert!(!AppConfig::is_secret_key("slack_webhook"));
        assert!(!AppConfig::is_secret_key("bot_token"));
    }

    // ---------------------------------------------------------------------------
    // VAL-CONFIG-006: HERMIP_* env var overrides (injectable _with pattern)
    // ---------------------------------------------------------------------------

    /// Helper: creates a minimal AppConfig with non-default TOML values so that
    /// env-var overrides can be verified as taking highest precedence.
    fn config_with_toml_values() -> AppConfig {
        AppConfig {
            daemon: DaemonConfig {
                bind_host: "0.0.0.0".into(),
                port: 25294,
                base_url: "http://127.0.0.1:25294".into(),
            },
            defaults: DefaultsConfig {
                channel: Some("toml-channel".into()),
                channel_name: Some("toml-channel-name".into()),
                format: MessageFormat::Compact,
            },
            providers: ProvidersConfig {
                discord: DiscordConfig {
                    bot_token: Some("toml-bot-token".into()),
                    legacy_default_channel: None,
                },
                slack: SlackConfig::default(),
            },
            ..AppConfig::default()
        }
    }

    // --- HERMIP_DAEMON_PORT ---

    #[test]
    fn hermip_daemon_port_overrides_toml_value() {
        let mut config = config_with_toml_values();
        assert_eq!(config.daemon.port, 25294);

        config.apply_hermip_env_overrides_with(|name| {
            (name == "HERMIP_DAEMON_PORT").then(|| "30999".to_string())
        });

        assert_eq!(config.daemon.port, 30999, "env var should override TOML");
    }

    #[test]
    fn hermip_daemon_port_is_ignored_when_invalid() {
        let mut config = config_with_toml_values();

        config.apply_hermip_env_overrides_with(|name| {
            (name == "HERMIP_DAEMON_PORT").then(|| "not-a-number".to_string())
        });

        assert_eq!(
            config.daemon.port, 25294,
            "invalid port should not override TOML"
        );
    }

    #[test]
    fn hermip_daemon_port_is_ignored_when_empty() {
        let mut config = config_with_toml_values();

        config.apply_hermip_env_overrides_with(|name| {
            (name == "HERMIP_DAEMON_PORT").then(|| "".to_string())
        });

        assert_eq!(
            config.daemon.port, 25294,
            "empty port string should not override TOML"
        );
    }

    #[test]
    fn hermip_daemon_port_is_ignored_when_unset() {
        let mut config = config_with_toml_values();

        config.apply_hermip_env_overrides_with(|_| None);

        assert_eq!(
            config.daemon.port, 25294,
            "unset env var should not override TOML"
        );
    }

    #[test]
    fn hermip_daemon_port_accepts_zero() {
        let mut config = config_with_toml_values();

        config.apply_hermip_env_overrides_with(|name| {
            (name == "HERMIP_DAEMON_PORT").then(|| "0".to_string())
        });

        assert_eq!(config.daemon.port, 0, "port 0 is a valid u16 value");
    }

    #[test]
    fn hermip_daemon_port_rejects_overflow() {
        let mut config = config_with_toml_values();

        config.apply_hermip_env_overrides_with(|name| {
            (name == "HERMIP_DAEMON_PORT").then(|| "99999".to_string())
        });

        assert_eq!(
            config.daemon.port, 25294,
            "u16 overflow should not override TOML"
        );
    }

    // --- HERMIP_DAEMON_BASE_URL ---

    #[test]
    fn hermip_daemon_base_url_overrides_toml_value() {
        let mut config = config_with_toml_values();

        config.apply_hermip_env_overrides_with(|name| {
            (name == "HERMIP_DAEMON_BASE_URL").then(|| "http://custom:8080".to_string())
        });

        assert_eq!(
            config.daemon.base_url, "http://custom:8080",
            "env var should override TOML"
        );
    }

    #[test]
    fn hermip_daemon_base_url_is_ignored_when_empty() {
        let mut config = config_with_toml_values();

        config.apply_hermip_env_overrides_with(|name| {
            (name == "HERMIP_DAEMON_BASE_URL").then(|| "".to_string())
        });

        assert_eq!(
            config.daemon.base_url, "http://127.0.0.1:25294",
            "empty value should not override TOML"
        );
    }

    #[test]
    fn hermip_daemon_base_url_is_ignored_when_whitespace_only() {
        let mut config = config_with_toml_values();

        config.apply_hermip_env_overrides_with(|name| {
            (name == "HERMIP_DAEMON_BASE_URL").then(|| "   ".to_string())
        });

        assert_eq!(
            config.daemon.base_url, "http://127.0.0.1:25294",
            "whitespace-only value should not override TOML"
        );
    }

    #[test]
    fn hermip_daemon_base_url_trims_whitespace() {
        let mut config = config_with_toml_values();

        config.apply_hermip_env_overrides_with(|name| {
            (name == "HERMIP_DAEMON_BASE_URL").then(|| "  http://custom:8080  ".to_string())
        });

        assert_eq!(
            config.daemon.base_url, "http://custom:8080",
            "value should be trimmed"
        );
    }

    // --- HERMIP_DEFAULTS_CHANNEL ---

    #[test]
    fn hermip_defaults_channel_overrides_toml_value() {
        let mut config = config_with_toml_values();

        config.apply_hermip_env_overrides_with(|name| {
            (name == "HERMIP_DEFAULTS_CHANNEL").then(|| "env-channel".to_string())
        });

        assert_eq!(
            config.defaults.channel.as_deref(),
            Some("env-channel"),
            "env var should override TOML"
        );
    }

    #[test]
    fn hermip_defaults_channel_is_ignored_when_empty() {
        let mut config = config_with_toml_values();

        config.apply_hermip_env_overrides_with(|name| {
            (name == "HERMIP_DEFAULTS_CHANNEL").then(|| "".to_string())
        });

        assert_eq!(
            config.defaults.channel.as_deref(),
            Some("toml-channel"),
            "empty value should not override TOML"
        );
    }

    #[test]
    fn hermip_defaults_channel_is_ignored_when_whitespace_only() {
        let mut config = config_with_toml_values();

        config.apply_hermip_env_overrides_with(|name| {
            (name == "HERMIP_DEFAULTS_CHANNEL").then(|| "   ".to_string())
        });

        assert_eq!(
            config.defaults.channel.as_deref(),
            Some("toml-channel"),
            "whitespace-only value should not override TOML"
        );
    }

    #[test]
    fn hermip_defaults_channel_trims_whitespace() {
        let mut config = config_with_toml_values();

        config.apply_hermip_env_overrides_with(|name| {
            (name == "HERMIP_DEFAULTS_CHANNEL").then(|| "  env-channel  ".to_string())
        });

        assert_eq!(
            config.defaults.channel.as_deref(),
            Some("env-channel"),
            "value should be trimmed"
        );
    }

    #[test]
    fn hermip_defaults_channel_sets_value_when_toml_is_none() {
        let mut config = AppConfig::default();
        assert!(config.defaults.channel.is_none());

        config.apply_hermip_env_overrides_with(|name| {
            (name == "HERMIP_DEFAULTS_CHANNEL").then(|| "123456".to_string())
        });

        assert_eq!(
            config.defaults.channel.as_deref(),
            Some("123456"),
            "env var should set value even when TOML has none"
        );
    }

    // --- HERMIP_DEFAULTS_FORMAT ---

    #[test]
    fn hermip_defaults_format_overrides_toml_value() {
        let mut config = config_with_toml_values();
        assert_eq!(config.defaults.format, MessageFormat::Compact);

        config.apply_hermip_env_overrides_with(|name| {
            (name == "HERMIP_DEFAULTS_FORMAT").then(|| "alert".to_string())
        });

        assert_eq!(
            config.defaults.format,
            MessageFormat::Alert,
            "env var should override TOML"
        );
    }

    #[test]
    fn hermip_defaults_format_accepts_all_valid_values() {
        let formats = [
            ("compact", MessageFormat::Compact),
            ("alert", MessageFormat::Alert),
            ("inline", MessageFormat::Inline),
            ("raw", MessageFormat::Raw),
        ];

        for (label, expected) in formats {
            let mut config = config_with_toml_values();
            let label_owned = label.to_string();
            config.apply_hermip_env_overrides_with(move |name| {
                (name == "HERMIP_DEFAULTS_FORMAT").then(|| label_owned.clone())
            });
            assert_eq!(
                config.defaults.format, expected,
                "format '{label}' should be accepted"
            );
        }
    }

    #[test]
    fn hermip_defaults_format_is_ignored_when_invalid() {
        let mut config = config_with_toml_values();

        config.apply_hermip_env_overrides_with(|name| {
            (name == "HERMIP_DEFAULTS_FORMAT").then(|| "invalid-format".to_string())
        });

        assert_eq!(
            config.defaults.format,
            MessageFormat::Compact,
            "invalid format should not override TOML"
        );
    }

    #[test]
    fn hermip_defaults_format_is_ignored_when_empty() {
        let mut config = config_with_toml_values();

        config.apply_hermip_env_overrides_with(|name| {
            (name == "HERMIP_DEFAULTS_FORMAT").then(|| "".to_string())
        });

        assert_eq!(
            config.defaults.format,
            MessageFormat::Compact,
            "empty value should not override TOML"
        );
    }

    #[test]
    fn hermip_defaults_format_trims_whitespace() {
        let mut config = config_with_toml_values();

        config.apply_hermip_env_overrides_with(|name| {
            (name == "HERMIP_DEFAULTS_FORMAT").then(|| "  alert  ".to_string())
        });

        assert_eq!(
            config.defaults.format,
            MessageFormat::Alert,
            "value should be trimmed before parsing"
        );
    }

    // --- HERMIP_PROVIDERS_DISCORD_TOKEN ---

    #[test]
    fn hermip_providers_discord_token_overrides_toml_value() {
        let mut config = config_with_toml_values();

        config.apply_hermip_env_overrides_with(|name| {
            (name == "HERMIP_PROVIDERS_DISCORD_TOKEN").then(|| "env-token".to_string())
        });

        assert_eq!(
            config.providers.discord.bot_token.as_deref(),
            Some("env-token"),
            "env var should override TOML"
        );
    }

    #[test]
    fn hermip_providers_discord_token_is_ignored_when_empty() {
        let mut config = config_with_toml_values();

        config.apply_hermip_env_overrides_with(|name| {
            (name == "HERMIP_PROVIDERS_DISCORD_TOKEN").then(|| "".to_string())
        });

        assert_eq!(
            config.providers.discord.bot_token.as_deref(),
            Some("toml-bot-token"),
            "empty value should not override TOML"
        );
    }

    #[test]
    fn hermip_providers_discord_token_is_ignored_when_whitespace_only() {
        let mut config = config_with_toml_values();

        config.apply_hermip_env_overrides_with(|name| {
            (name == "HERMIP_PROVIDERS_DISCORD_TOKEN").then(|| "   ".to_string())
        });

        assert_eq!(
            config.providers.discord.bot_token.as_deref(),
            Some("toml-bot-token"),
            "whitespace-only value should not override TOML"
        );
    }

    #[test]
    fn hermip_providers_discord_token_trims_whitespace() {
        let mut config = config_with_toml_values();

        config.apply_hermip_env_overrides_with(|name| {
            (name == "HERMIP_PROVIDERS_DISCORD_TOKEN").then(|| "  env-token  ".to_string())
        });

        assert_eq!(
            config.providers.discord.bot_token.as_deref(),
            Some("env-token"),
            "value should be trimmed"
        );
    }

    #[test]
    fn hermip_providers_discord_token_sets_value_when_toml_is_none() {
        let mut config = AppConfig::default();
        assert!(config.providers.discord.bot_token.is_none());

        config.apply_hermip_env_overrides_with(|name| {
            (name == "HERMIP_PROVIDERS_DISCORD_TOKEN").then(|| "bot-token-from-env".to_string())
        });

        assert_eq!(
            config.providers.discord.bot_token.as_deref(),
            Some("bot-token-from-env"),
            "env var should set value even when TOML has none"
        );
    }

    // --- HERMIP_DEFAULTS_CHANNEL_NAME ---

    #[test]
    fn hermip_defaults_channel_name_overrides_toml_value() {
        let mut config = config_with_toml_values();

        config.apply_hermip_env_overrides_with(|name| {
            (name == "HERMIP_DEFAULTS_CHANNEL_NAME").then(|| "env-channel-name".to_string())
        });

        assert_eq!(
            config.defaults.channel_name.as_deref(),
            Some("env-channel-name"),
            "env var should override TOML"
        );
    }

    #[test]
    fn hermip_defaults_channel_name_is_ignored_when_empty() {
        let mut config = config_with_toml_values();

        config.apply_hermip_env_overrides_with(|name| {
            (name == "HERMIP_DEFAULTS_CHANNEL_NAME").then(|| "".to_string())
        });

        assert_eq!(
            config.defaults.channel_name.as_deref(),
            Some("toml-channel-name"),
            "empty value should not override TOML"
        );
    }

    #[test]
    fn hermip_defaults_channel_name_is_ignored_when_whitespace_only() {
        let mut config = config_with_toml_values();

        config.apply_hermip_env_overrides_with(|name| {
            (name == "HERMIP_DEFAULTS_CHANNEL_NAME").then(|| "   ".to_string())
        });

        assert_eq!(
            config.defaults.channel_name.as_deref(),
            Some("toml-channel-name"),
            "whitespace-only value should not override TOML"
        );
    }

    #[test]
    fn hermip_defaults_channel_name_trims_whitespace() {
        let mut config = config_with_toml_values();

        config.apply_hermip_env_overrides_with(|name| {
            (name == "HERMIP_DEFAULTS_CHANNEL_NAME").then(|| "  env-channel-name  ".to_string())
        });

        assert_eq!(
            config.defaults.channel_name.as_deref(),
            Some("env-channel-name"),
            "value should be trimmed"
        );
    }

    #[test]
    fn hermip_defaults_channel_name_sets_value_when_toml_is_none() {
        let mut config = AppConfig::default();
        assert!(config.defaults.channel_name.is_none());

        config.apply_hermip_env_overrides_with(|name| {
            (name == "HERMIP_DEFAULTS_CHANNEL_NAME").then(|| "general".to_string())
        });

        assert_eq!(
            config.defaults.channel_name.as_deref(),
            Some("general"),
            "env var should set value even when TOML has none"
        );
    }

    // --- HERMIP_CONFIG (default_config_path_with) ---

    #[test]
    fn hermip_config_env_var_overrides_default_path() {
        let path = default_config_path_with(
            |name| (name == "HERMIP_CONFIG").then(|| "/custom/hermip.toml".to_string()),
            || None,
            |_| None,
        );

        assert_eq!(
            path,
            PathBuf::from("/custom/hermip.toml"),
            "HERMIP_CONFIG should override default path"
        );
    }

    #[test]
    fn hermip_config_env_var_trims_whitespace() {
        let path = default_config_path_with(
            |name| (name == "HERMIP_CONFIG").then(|| "  /custom/hermip.toml  ".to_string()),
            || None,
            |_| None,
        );

        assert_eq!(
            path,
            PathBuf::from("  /custom/hermip.toml  "),
            "HERMIP_CONFIG value is used as-is (caller trims if needed)"
        );
    }

    #[test]
    fn hermip_config_empty_value_falls_through_to_cwd() {
        let dir = tempfile::tempdir().unwrap();
        let local = dir.path().join("hermip.toml");
        fs::write(&local, "[daemon]\nport = 1\n").unwrap();

        let path = default_config_path_with(
            |name| (name == "HERMIP_CONFIG").then(|| "".to_string()),
            || Some(dir.path().to_path_buf()),
            |_| None,
        );

        assert_eq!(
            path, local,
            "empty HERMIP_CONFIG should fall through to CWD"
        );
    }

    #[test]
    fn hermip_config_unset_falls_through_to_global() {
        let path = default_config_path_with(
            |_| None,
            || None,
            |name| (name == "HOME").then(|| "/home/testuser".to_string()),
        );

        assert_eq!(
            path,
            PathBuf::from("/home/testuser/.config/hermip/hermip.toml"),
            "unset HERMIP_CONFIG should fall through to global path"
        );
    }

    #[test]
    fn hermip_config_unset_cwd_local_file_takes_precedence_over_global() {
        let dir = tempfile::tempdir().unwrap();
        let local = dir.path().join("hermip.toml");
        fs::write(&local, "[daemon]\nport = 1\n").unwrap();

        let path = default_config_path_with(
            |_| None,
            || Some(dir.path().to_path_buf()),
            |name| (name == "HOME").then(|| "/home/testuser".to_string()),
        );

        assert_eq!(
            path, local,
            "local hermip.toml should take precedence over global"
        );
    }

    // --- Multiple overrides simultaneously ---

    #[test]
    fn multiple_hermip_env_overrides_all_take_effect() {
        let mut config = config_with_toml_values();

        config.apply_hermip_env_overrides_with(|name| match name {
            "HERMIP_DAEMON_PORT" => Some("30999".to_string()),
            "HERMIP_DAEMON_BASE_URL" => Some("http://custom:30999".to_string()),
            "HERMIP_DEFAULTS_CHANNEL" => Some("env-channel".to_string()),
            "HERMIP_DEFAULTS_FORMAT" => Some("raw".to_string()),
            "HERMIP_PROVIDERS_DISCORD_TOKEN" => Some("env-token".to_string()),
            "HERMIP_DEFAULTS_CHANNEL_NAME" => Some("env-channel-name".to_string()),
            _ => None,
        });

        assert_eq!(config.daemon.port, 30999);
        assert_eq!(config.daemon.base_url, "http://custom:30999");
        assert_eq!(config.defaults.channel.as_deref(), Some("env-channel"));
        assert_eq!(config.defaults.format, MessageFormat::Raw);
        assert_eq!(
            config.providers.discord.bot_token.as_deref(),
            Some("env-token")
        );
        assert_eq!(
            config.defaults.channel_name.as_deref(),
            Some("env-channel-name")
        );
    }

    #[test]
    fn no_hermip_env_overrides_preserves_toml_values() {
        let mut config = config_with_toml_values();

        config.apply_hermip_env_overrides_with(|_| None);

        assert_eq!(config.daemon.port, 25294);
        assert_eq!(config.daemon.base_url, "http://127.0.0.1:25294");
        assert_eq!(config.defaults.channel.as_deref(), Some("toml-channel"));
        assert_eq!(config.defaults.format, MessageFormat::Compact);
        assert_eq!(
            config.providers.discord.bot_token.as_deref(),
            Some("toml-bot-token")
        );
        assert_eq!(
            config.defaults.channel_name.as_deref(),
            Some("toml-channel-name")
        );
    }

    #[test]
    fn partial_hermip_env_overrides_only_affect_specified_fields() {
        let mut config = config_with_toml_values();

        // Only override port and format, leave others from TOML.
        config.apply_hermip_env_overrides_with(|name| match name {
            "HERMIP_DAEMON_PORT" => Some("40999".to_string()),
            "HERMIP_DEFAULTS_FORMAT" => Some("alert".to_string()),
            _ => None,
        });

        assert_eq!(config.daemon.port, 40999, "port should be overridden");
        assert_eq!(
            config.daemon.base_url, "http://127.0.0.1:25294",
            "base_url should remain from TOML"
        );
        assert_eq!(
            config.defaults.channel.as_deref(),
            Some("toml-channel"),
            "channel should remain from TOML"
        );
        assert_eq!(
            config.defaults.format,
            MessageFormat::Alert,
            "format should be overridden"
        );
        assert_eq!(
            config.providers.discord.bot_token.as_deref(),
            Some("toml-bot-token"),
            "bot_token should remain from TOML"
        );
        assert_eq!(
            config.defaults.channel_name.as_deref(),
            Some("toml-channel-name"),
            "channel_name should remain from TOML"
        );
    }

    // --- Env vars have highest precedence (override even after TOML merge) ---

    #[test]
    fn hermip_env_overrides_have_highest_precedence_after_full_load() {
        // Simulate a full config load path: parse TOML → merge legacy → normalize → apply env overrides
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        fs::write(
            &path,
            r#"[daemon]
port = 25294
base_url = "http://127.0.0.1:25294"

[defaults]
channel = "toml-channel"
format = "compact"

[providers.discord]
token = "toml-bot-token"
"#,
        )
        .unwrap();

        let mut config = AppConfig::load_or_default(&path).unwrap();

        // Now apply env overrides using the _with pattern
        config.apply_hermip_env_overrides_with(|name| match name {
            "HERMIP_DAEMON_PORT" => Some("30999".to_string()),
            "HERMIP_DEFAULTS_CHANNEL" => Some("env-channel".to_string()),
            "HERMIP_DEFAULTS_FORMAT" => Some("raw".to_string()),
            "HERMIP_PROVIDERS_DISCORD_TOKEN" => Some("env-token".to_string()),
            _ => None,
        });

        assert_eq!(
            config.daemon.port, 30999,
            "env should override loaded TOML port"
        );
        assert_eq!(
            config.defaults.channel.as_deref(),
            Some("env-channel"),
            "env should override loaded TOML channel"
        );
        assert_eq!(
            config.defaults.format,
            MessageFormat::Raw,
            "env should override loaded TOML format"
        );
        assert_eq!(
            config.providers.discord.bot_token.as_deref(),
            Some("env-token"),
            "env should override loaded TOML token"
        );
    }

    // --- Individual HERMIP_* env var tests ---

    #[test]
    fn hermip_daemon_port_overrides_toml() {
        let mut config = config_with_toml_values();
        config.daemon.port = 25294;

        config.apply_hermip_env_overrides_with(|name| {
            (name == "HERMIP_DAEMON_PORT").then(|| "40999".to_string())
        });

        assert_eq!(
            config.daemon.port, 40999,
            "HERMIP_DAEMON_PORT should override TOML"
        );
    }

    #[test]
    fn hermip_daemon_port_rejects_invalid_values() {
        let mut config = config_with_toml_values();

        config.apply_hermip_env_overrides_with(|name| {
            (name == "HERMIP_DAEMON_PORT").then(|| "invalid".to_string())
        });

        assert_eq!(
            config.daemon.port, 25294,
            "Invalid port should not override"
        );
    }
}
