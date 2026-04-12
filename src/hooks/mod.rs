#[cfg(any(feature = "codex-hook", feature = "claude-hook"))]
#[allow(dead_code)]
pub mod prompt_deliver;

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::anyhow;
use serde_json::json;
#[cfg(any(feature = "codex-hook", feature = "claude-hook"))]
use serde_json::{Map, Value};

use crate::Result;
use crate::cli::{HookInstallScope, HookProvider, HooksInstallArgs, HooksUninstallArgs};
#[cfg(feature = "claude-hook")]
use crate::native_hooks::CLAUDE_SETTINGS_FILE;
#[cfg(feature = "codex-hook")]
use crate::native_hooks::CODEX_HOOKS_FILE;
use crate::native_hooks::{
    HERMES_PLUGIN_DIR, HERMIP_PROJECT_FILE, HOOK_SCRIPT, SHARED_HOOK_EVENTS, generated_hook_script,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallReport {
    pub generated_files: Vec<PathBuf>,
}

pub fn install(args: HooksInstallArgs) -> Result<()> {
    let report = run_install(&args)?;

    println!("Installed provider-native hook forwarding:");
    for path in &report.generated_files {
        println!("  {}", path.display());
    }
    println!("Supported shared events: {}", SHARED_HOOK_EVENTS.join(", "));
    println!("Ingress: hermip native hook --provider <codex|claude-code|hermes>");

    Ok(())
}

pub fn uninstall(args: HooksUninstallArgs) -> Result<()> {
    let report = run_uninstall(&args)?;

    if report.removed_files.is_empty() && report.removed_dirs.is_empty() {
        println!("No hermip hook files found to remove.");
    } else {
        println!("Uninstalled provider-native hook forwarding:");
        for path in &report.removed_files {
            println!("  removed file: {}", path.display());
        }
        for path in &report.removed_dirs {
            println!("  removed dir: {}", path.display());
        }
    }

    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UninstallReport {
    pub removed_files: Vec<PathBuf>,
    pub removed_dirs: Vec<PathBuf>,
}

fn run_uninstall(args: &HooksUninstallArgs) -> Result<UninstallReport> {
    let root = resolve_uninstall_root(args)?;
    let providers = selected_uninstall_providers(args);
    let mut removed_files = Vec::new();
    let mut removed_dirs = Vec::new();

    for provider in providers {
        match provider {
            #[cfg(feature = "codex-hook")]
            HookProvider::Codex => {
                let path = root.join(CODEX_HOOKS_FILE);
                if path.exists() {
                    remove_hermip_hooks_from_file(&path)?;
                    removed_files.push(path);
                }
            }
            #[cfg(feature = "claude-hook")]
            HookProvider::ClaudeCode => {
                let path = root.join(CLAUDE_SETTINGS_FILE);
                if path.exists() {
                    remove_hermip_hooks_from_file(&path)?;
                    removed_files.push(path);
                }
            }
            HookProvider::Hermes => {
                let plugin_dir = root.join(HERMES_PLUGIN_DIR);
                if plugin_dir.exists() {
                    // Remove hermip-managed files in the plugin directory
                    let files_to_check = [
                        plugin_dir.join("plugin.yaml"),
                        plugin_dir.join("__init__.py"),
                        plugin_dir.join("hooks.py"),
                        plugin_dir.join("tools.py"),
                        plugin_dir.join("hooks").join("native-hook.mjs"),
                    ];
                    for path in files_to_check {
                        if path.exists() {
                            fs::remove_file(&path)?;
                            removed_files.push(path);
                        }
                    }
                    // Remove hooks dir if empty
                    let hooks_dir = plugin_dir.join("hooks");
                    if hooks_dir.exists()
                        && let Ok(entries) = fs::read_dir(&hooks_dir)
                        && entries.count() == 0
                    {
                        fs::remove_dir(&hooks_dir)?;
                        removed_dirs.push(hooks_dir);
                    }
                    // Remove plugin dir if empty
                    if let Ok(entries) = fs::read_dir(&plugin_dir)
                        && entries.count() == 0
                    {
                        fs::remove_dir(&plugin_dir)?;
                        removed_dirs.push(plugin_dir);
                    }
                }
            }
        }
    }

    // If --clean, also remove the hook script and project metadata
    if args.clean {
        let hook_script = root.join(HOOK_SCRIPT);
        if hook_script.exists() {
            fs::remove_file(&hook_script)?;
            removed_files.push(hook_script);
        }
        let metadata = root.join(HERMIP_PROJECT_FILE);
        if metadata.exists() {
            fs::remove_file(&metadata)?;
            removed_files.push(metadata);
        }
    }

    Ok(UninstallReport {
        removed_files,
        removed_dirs,
    })
}

fn resolve_uninstall_root(args: &HooksUninstallArgs) -> Result<PathBuf> {
    match args.scope {
        HookInstallScope::Project => Ok(args
            .root
            .clone()
            .unwrap_or(std::env::current_dir()?)
            .canonicalize()?),
        HookInstallScope::Global => home_dir(),
    }
}

fn selected_uninstall_providers(args: &HooksUninstallArgs) -> Vec<HookProvider> {
    if args.all || args.provider.is_empty() {
        default_uninstall_providers()
    } else {
        args.provider.clone()
    }
}

#[cfg(all(feature = "codex-hook", not(feature = "claude-hook")))]
fn default_uninstall_providers() -> Vec<HookProvider> {
    vec![HookProvider::Codex, HookProvider::Hermes]
}

#[cfg(all(not(feature = "codex-hook"), feature = "claude-hook"))]
fn default_uninstall_providers() -> Vec<HookProvider> {
    vec![HookProvider::ClaudeCode, HookProvider::Hermes]
}

#[cfg(all(feature = "codex-hook", feature = "claude-hook"))]
fn default_uninstall_providers() -> Vec<HookProvider> {
    vec![
        HookProvider::Codex,
        HookProvider::ClaudeCode,
        HookProvider::Hermes,
    ]
}

#[cfg(all(not(feature = "codex-hook"), not(feature = "claude-hook")))]
fn default_uninstall_providers() -> Vec<HookProvider> {
    vec![HookProvider::Hermes]
}

#[cfg(any(feature = "codex-hook", feature = "claude-hook"))]
fn remove_hermip_hooks_from_file(path: &Path) -> Result<()> {
    let content = fs::read_to_string(path)?;
    let mut document: Map<String, Value> =
        serde_json::from_str(&content).unwrap_or_else(|_| Map::new());

    if let Some(hooks) = document.get_mut("hooks").and_then(Value::as_object_mut) {
        // Remove hermip entries from each hook event
        for event in SHARED_HOOK_EVENTS {
            if let Some(event_hooks) = hooks.get_mut(event).and_then(Value::as_array_mut) {
                // Remove groups that contain only hermip hook commands
                event_hooks.retain(|group| {
                    if let Some(obj) = group.as_object() {
                        if let Some(hooks_arr) = obj.get("hooks").and_then(Value::as_array) {
                            // Keep group if it has non-hermip hooks
                            hooks_arr.iter().any(|h| {
                                if let Some(cmd) = h.get("command").and_then(Value::as_str) {
                                    !cmd.contains("native-hook.mjs") && !cmd.contains("hermip")
                                } else {
                                    true
                                }
                            })
                        } else {
                            true
                        }
                    } else {
                        true
                    }
                });
            }
        }
    }

    fs::write(
        path,
        serde_json::to_string_pretty(&Value::Object(document))? + "\n",
    )?;
    Ok(())
}

fn run_install(args: &HooksInstallArgs) -> Result<InstallReport> {
    let root = resolve_install_root(args)?;
    let providers = selected_providers(args);
    let mut generated_files = Vec::new();

    let hook_script_path = root.join(HOOK_SCRIPT);
    write_generated_file(&hook_script_path, generated_hook_script(), args.force)?;
    generated_files.push(hook_script_path.clone());

    if args.scope == HookInstallScope::Project {
        let metadata_path = ensure_project_metadata(&root, args.force)?;
        generated_files.push(metadata_path);
    }

    for provider in providers {
        let path = match provider {
            #[cfg(feature = "codex-hook")]
            HookProvider::Codex => write_codex_hooks(&root, &hook_script_path)?,
            #[cfg(feature = "claude-hook")]
            HookProvider::ClaudeCode => write_claude_settings(&root, &hook_script_path)?,
            HookProvider::Hermes => write_hermes_plugin(&root, &hook_script_path)?,
        };
        generated_files.push(path);
    }

    Ok(InstallReport { generated_files })
}

fn write_hermes_plugin(root: &Path, hook_script_path: &Path) -> Result<PathBuf> {
    let plugin_dir = root.join(HERMES_PLUGIN_DIR);
    let mut generated_files = Vec::new();

    // Write plugin.yaml
    let yaml_path = plugin_dir.join("plugin.yaml");
    let yaml_content = hermes_plugin_yaml();
    write_generated_file(&yaml_path, yaml_content, true)?;
    generated_files.push(yaml_path);

    // Write __init__.py
    let init_path = plugin_dir.join("__init__.py");
    write_generated_file(&init_path, HERMES_INIT_PY_CONTENT, true)?;
    generated_files.push(init_path);

    // Write hooks.py
    let hooks_path = plugin_dir.join("hooks.py");
    write_generated_file(&hooks_path, HERMES_HOOKS_PY_CONTENT, true)?;
    generated_files.push(hooks_path);

    // Write tools.py
    let tools_path = plugin_dir.join("tools.py");
    write_generated_file(&tools_path, HERMES_TOOLS_PY_CONTENT, true)?;
    generated_files.push(tools_path);

    // Write native-hook.mjs in the hermip plugin dir for forwarding
    let hermip_hook_dir = plugin_dir.join("hooks");
    let hook_dest = hermip_hook_dir.join("native-hook.mjs");
    write_generated_file(&hook_dest, generated_hook_script(), true)?;
    generated_files.push(hook_dest);

    // Register hook script as pre_tool_call and post_tool_call in hooks.py
    // The hooks.py content already references the hook script via a fixed path.
    // For the Hermes hook integration to work, the hooks.py must call the hermip daemon.
    let _ = hook_script_path; // Used via hook_dest path
    let _ = root;

    Ok(plugin_dir)
}

fn hermes_plugin_yaml() -> &'static str {
    r#"name: hermip
version: "0.1.0"
description: "Hermip event gateway for Discord, Slack, and Droid Mission notifications"
provides_hooks:
  - pre_tool_call
  - post_tool_call
  - on_session_start
  - on_session_end
  - pre_llm_call
  - post_llm_call
provides_tools:
  - name: send-event
    description: "Send an event to the Hermip daemon"
  - name: config-verify
    description: "Verify Hermip webhook bindings"
"#
}

const HERMES_INIT_PY_CONTENT: &str = r#""""Hermip plugin for Hermes Agent."""

import json
import subprocess
import sys


def register(ctx):
    """Register hook handlers and tools with Hermes Agent."""
    return {
        "hooks": {
            "pre_tool_call": "hooks.pre_tool_call",
            "post_tool_call": "hooks.post_tool_call",
            "on_session_start": "hooks.on_session_start",
            "on_session_end": "hooks.on_session_end",
            "pre_llm_call": "hooks.pre_llm_call",
            "post_llm_call": "hooks.post_llm_call",
        },
        "tools": {
            "send-event": "tools.send_event",
            "config-verify": "tools.config_verify",
        },
    }


def forward_event(event_name: str, payload: dict) -> None:
    """Forward an event to the Hermip daemon."""
    try:
        daemon_url = "http://localhost:25294"
        full_payload = {
            "provider": "hermes",
            "event_name": event_name,
            **payload,
        }
        result = subprocess.run(
            [
                sys.executable,
                "-c",
                f"""
import urllib.request
import json
payload = json.dumps({full_payload}).encode()
req = urllib.request.Request('{daemon_url}/api/native/hook', data=payload, headers={{'Content-Type': 'application/json'}})
urllib.request.urlopen(req)
""",
            ],
            capture_output=True,
            timeout=5,
        )
    except Exception:
        pass  # Best-effort forwarding
"#;

const HERMES_HOOKS_PY_CONTENT: &str = r#""""Hermip hook handlers for Hermes Agent."""

import json
import os
import subprocess
import sys
import time
from pathlib import Path

DAEMON_URL = os.environ.get("HERMIP_DAEMON_URL", "http://localhost:25294")


def forward_to_hermip(event_name: str, context: dict) -> None:
    """Send a hook event to the Hermip daemon."""
    try:
        import urllib.request

        payload = {
            "provider": "hermes",
            "event_name": event_name,
            "directory": os.getcwd(),
            "context": context,
        }
        data = json.dumps(payload).encode("utf-8")
        req = urllib.request.Request(
            f"{DAEMON_URL}/api/native/hook",
            data=data,
            headers={"Content-Type": "application/json"},
        )
        with urllib.request.urlopen(req, timeout=5) as response:
            return response.status == 202
    except Exception:
        return False


def pre_tool_call(tool_name: str, tool_input: dict, context: dict) -> dict:
    """Called before a tool is invoked."""
    forward_to_hermip("PreToolUse", {
        "tool_name": tool_name,
        "tool_input": tool_input,
        "session_id": context.get("session_id"),
    })
    return {}


def post_tool_call(tool_name: str, tool_input: dict, tool_output: dict, context: dict) -> dict:
    """Called after a tool completes."""
    forward_to_hermip("PostToolUse", {
        "tool_name": tool_name,
        "tool_input": tool_input,
        "tool_output": tool_output,
        "session_id": context.get("session_id"),
    })
    return {}


def on_session_start(session_id: str, context: dict) -> None:
    """Called when a new session starts."""
    forward_to_hermip("SessionStart", {
        "session_id": session_id,
        "cwd": context.get("cwd", os.getcwd()),
    })


def on_session_end(session_id: str, context: dict) -> None:
    """Called when a session ends."""
    forward_to_hermip("Stop", {
        "session_id": session_id,
    })


def pre_llm_call(model: str, prompt: str, context: dict) -> dict:
    """Called before an LLM call."""
    forward_to_hermip("PreToolUse", {
        "tool_name": "llm",
        "tool_input": {"model": model, "prompt": prompt[:200]},
        "session_id": context.get("session_id"),
    })
    return {}


def post_llm_call(model: str, prompt: str, response: str, context: dict) -> dict:
    """Called after an LLM call completes."""
    forward_to_hermip("PostToolUse", {
        "tool_name": "llm",
        "tool_input": {"model": model, "prompt": prompt[:200]},
        "tool_output": {"response": response[:200]},
        "session_id": context.get("session_id"),
    })
    return {}
"#;

const HERMES_TOOLS_PY_CONTENT: &str = r#""""Hermip tools for Hermes Agent."""

import json
import subprocess
import sys


def send_event(event_type: str, source: str = "hermes", channel: str = None, message: str = "") -> dict:
    """Send an event to the Hermip daemon.

    Args:
        event_type: The event type (e.g. mission.started, feature.completed)
        source: The event source (default: hermes)
        channel: Optional channel override
        message: Optional message text

    Returns:
        A dict with ok=true and the event_id on success.
    """
    import urllib.request

    daemon_url = "http://localhost:25294"
    payload = {
        "provider": "hermes",
        "event_name": event_type,
        "source": source,
        "channel": channel,
        "message": message,
        "directory": "",
    }
    data = json.dumps(payload).encode("utf-8")
    req = urllib.request.Request(
        f"{daemon_url}/api/native/hook",
        data=data,
        headers={"Content-Type": "application/json"},
    )
    try:
        with urllib.request.urlopen(req, timeout=10) as response:
            result = json.loads(response.read())
            return result
    except urllib.error.URLError as e:
        return {"ok": False, "error": str(e)}


def config_verify() -> dict:
    """Verify Hermip webhook bindings against live Discord API.

    Runs `hermip config verify-bindings` and returns the result.

    Returns:
        A dict with ok=true and verification results on success.
    """
    try:
        result = subprocess.run(
            ["hermip", "config", "verify-bindings"],
            capture_output=True,
            text=True,
            timeout=30,
        )
        return {
            "ok": result.returncode == 0,
            "stdout": result.stdout,
            "stderr": result.stderr,
            "exit_code": result.returncode,
        }
    except Exception as e:
        return {"ok": False, "error": str(e)}
"#;

fn resolve_install_root(args: &HooksInstallArgs) -> Result<PathBuf> {
    match args.scope {
        HookInstallScope::Project => Ok(args
            .root
            .clone()
            .unwrap_or(std::env::current_dir()?)
            .canonicalize()?),
        HookInstallScope::Global => home_dir(),
    }
}

fn selected_providers(args: &HooksInstallArgs) -> Vec<HookProvider> {
    if args.all || args.provider.is_empty() {
        default_install_providers()
    } else {
        args.provider.clone()
    }
}

#[cfg(all(feature = "codex-hook", not(feature = "claude-hook")))]
fn default_install_providers() -> Vec<HookProvider> {
    vec![HookProvider::Codex]
}

#[cfg(all(not(feature = "codex-hook"), feature = "claude-hook"))]
fn default_install_providers() -> Vec<HookProvider> {
    vec![HookProvider::ClaudeCode]
}

#[cfg(all(feature = "codex-hook", feature = "claude-hook"))]
fn default_install_providers() -> Vec<HookProvider> {
    vec![HookProvider::Codex, HookProvider::ClaudeCode]
}

#[cfg(all(not(feature = "codex-hook"), not(feature = "claude-hook")))]
fn default_install_providers() -> Vec<HookProvider> {
    vec![]
}

#[cfg(feature = "codex-hook")]
fn write_codex_hooks(root: &Path, hook_script_path: &Path) -> Result<PathBuf> {
    let path = root.join(CODEX_HOOKS_FILE);
    let mut document = read_json_object(&path)?;
    let hooks = ensure_child_object(&mut document, "hooks")?;
    let command = hook_command(hook_script_path, HookProvider::Codex);

    for event in SHARED_HOOK_EVENTS {
        upsert_hook_event(hooks, event, &command, codex_matcher_for(event));
    }

    write_json(&path, Value::Object(document))?;
    Ok(path)
}

#[cfg(feature = "claude-hook")]
fn write_claude_settings(root: &Path, hook_script_path: &Path) -> Result<PathBuf> {
    let path = root.join(CLAUDE_SETTINGS_FILE);
    let mut document = read_json_object(&path)?;
    let hooks = ensure_child_object(&mut document, "hooks")?;
    let command = hook_command(hook_script_path, HookProvider::ClaudeCode);

    for event in SHARED_HOOK_EVENTS {
        upsert_hook_event(hooks, event, &command, claude_matcher_for(event));
    }

    write_json(&path, Value::Object(document))?;
    Ok(path)
}

#[cfg(feature = "codex-hook")]
fn codex_matcher_for(event: &str) -> Option<&'static str> {
    match event {
        "PreToolUse" | "PostToolUse" => Some(".*"),
        _ => None,
    }
}

#[cfg(feature = "claude-hook")]
fn claude_matcher_for(event: &str) -> Option<&'static str> {
    match event {
        "PreToolUse" | "PostToolUse" => Some(".*"),
        _ => None,
    }
}

#[cfg(any(feature = "codex-hook", feature = "claude-hook"))]
fn hook_command(hook_script_path: &Path, provider: HookProvider) -> String {
    format!(
        "node {} --provider {}",
        shell_escape(&hook_script_path.display().to_string()),
        provider.as_str()
    )
}

#[cfg(any(feature = "codex-hook", feature = "claude-hook"))]
fn upsert_hook_event(
    hooks: &mut Map<String, Value>,
    event: &str,
    command: &str,
    matcher: Option<&str>,
) {
    let entry = hooks
        .entry(event.to_string())
        .or_insert_with(|| Value::Array(Vec::new()));
    let groups = entry.as_array_mut().expect("hook event groups array");

    if let Some(existing_group) = groups
        .iter_mut()
        .find(|group| matcher_matches(group, matcher))
    {
        let hooks = ensure_group_hooks(existing_group);
        if !hooks.iter().any(|hook| hook_command_matches(hook, command)) {
            hooks.push(json!({
                "type": "command",
                "command": command,
            }));
        }
        return;
    }

    let mut group = Map::new();
    if let Some(matcher) = matcher {
        group.insert("matcher".into(), json!(matcher));
    }
    group.insert(
        "hooks".into(),
        json!([
            {
                "type": "command",
                "command": command,
            }
        ]),
    );
    groups.push(Value::Object(group));
}

#[cfg(any(feature = "codex-hook", feature = "claude-hook"))]
fn matcher_matches(group: &Value, matcher: Option<&str>) -> bool {
    match (group.get("matcher").and_then(Value::as_str), matcher) {
        (None, None) => true,
        (Some(existing), Some(expected)) => existing == expected,
        _ => false,
    }
}

#[cfg(any(feature = "codex-hook", feature = "claude-hook"))]
fn ensure_group_hooks(group: &mut Value) -> &mut Vec<Value> {
    let object = group.as_object_mut().expect("hook event group object");
    object
        .entry("hooks")
        .or_insert_with(|| Value::Array(Vec::new()))
        .as_array_mut()
        .expect("hooks array")
}

#[cfg(any(feature = "codex-hook", feature = "claude-hook"))]
fn hook_command_matches(hook: &Value, command: &str) -> bool {
    hook.get("type").and_then(Value::as_str) == Some("command")
        && hook.get("command").and_then(Value::as_str) == Some(command)
}

fn ensure_project_metadata(root: &Path, force: bool) -> Result<PathBuf> {
    let path = root.join(HERMIP_PROJECT_FILE);
    let project_name = root
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("unknown")
        .to_string();
    let content = serde_json::to_string_pretty(&json!({
        "project": project_name,
        "repo_name": project_name,
    }))? + "\n";
    write_generated_file(&path, &content, force)?;
    Ok(path)
}

fn write_generated_file(path: &Path, content: &str, force: bool) -> Result<()> {
    if path.exists() && !force {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, content)?;
    #[cfg(unix)]
    if path
        .extension()
        .and_then(|value| value.to_str())
        .is_some_and(|ext| ext == "mjs")
    {
        set_executable(path)?;
    }
    Ok(())
}

#[cfg(any(feature = "codex-hook", feature = "claude-hook"))]
fn read_json_object(path: &Path) -> Result<Map<String, Value>> {
    if !path.exists() {
        return Ok(Map::new());
    }

    let content = fs::read_to_string(path)?;
    let value: Value = serde_json::from_str(&content)?;
    value
        .as_object()
        .cloned()
        .ok_or_else(|| anyhow!("{} must contain a JSON object", path.display()).into())
}

#[cfg(any(feature = "codex-hook", feature = "claude-hook"))]
fn ensure_child_object<'a>(
    object: &'a mut Map<String, Value>,
    key: &str,
) -> Result<&'a mut Map<String, Value>> {
    let entry = object
        .entry(key.to_string())
        .or_insert_with(|| Value::Object(Map::new()));
    entry
        .as_object_mut()
        .ok_or_else(|| anyhow!("{key} must be a JSON object").into())
}

#[cfg(any(feature = "codex-hook", feature = "claude-hook"))]
fn write_json(path: &Path, value: Value) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, serde_json::to_string_pretty(&value)? + "\n")?;
    Ok(())
}

#[cfg(any(feature = "codex-hook", feature = "claude-hook"))]
fn shell_escape(value: &str) -> String {
    if value.is_empty() {
        return "''".to_string();
    }
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn home_dir() -> Result<PathBuf> {
    std::env::var("HOME")
        .map(PathBuf::from)
        .map_err(|_| anyhow!("HOME environment variable not set").into())
}

#[cfg(unix)]
fn set_executable(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = fs::metadata(path)?.permissions();
    perms.set_mode(0o755);
    fs::set_permissions(path, perms)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    #![allow(unused_imports)]
    use super::*;
    use tempfile::tempdir;

    #[cfg(all(feature = "codex-hook", feature = "claude-hook"))]
    #[test]
    fn install_project_scope_writes_generic_provider_files() {
        let dir = tempdir().expect("tempdir");
        // Canonicalize to handle macOS /private/var/folders symlink situation
        let canonical_dir = dir.path().canonicalize().expect("canonicalize");
        let report = run_install(&HooksInstallArgs {
            all: true,
            provider: Vec::new(),
            scope: HookInstallScope::Project,
            root: Some(canonical_dir.clone()),
            force: false,
        })
        .expect("install");

        assert!(
            report
                .generated_files
                .contains(&canonical_dir.join(HOOK_SCRIPT))
        );
        assert!(
            report
                .generated_files
                .contains(&canonical_dir.join(HERMIP_PROJECT_FILE))
        );
        assert!(
            report
                .generated_files
                .contains(&canonical_dir.join(CODEX_HOOKS_FILE))
        );
        assert!(
            report
                .generated_files
                .contains(&canonical_dir.join(CLAUDE_SETTINGS_FILE))
        );
    }

    #[cfg(feature = "codex-hook")]
    #[test]
    fn codex_install_writes_shared_events() {
        let dir = tempdir().expect("tempdir");
        let path =
            write_codex_hooks(dir.path(), &dir.path().join(HOOK_SCRIPT)).expect("codex hooks");
        let document: Value = serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap();
        for event in SHARED_HOOK_EVENTS {
            assert!(document["hooks"][event].is_array(), "missing {event}");
        }
    }

    #[cfg(feature = "claude-hook")]
    #[test]
    fn claude_install_writes_shared_events() {
        let dir = tempdir().expect("tempdir");
        let path = write_claude_settings(dir.path(), &dir.path().join(HOOK_SCRIPT))
            .expect("claude settings");
        let document: Value = serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap();
        for event in SHARED_HOOK_EVENTS {
            assert!(document["hooks"][event].is_array(), "missing {event}");
        }
    }
}
