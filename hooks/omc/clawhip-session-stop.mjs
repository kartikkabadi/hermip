#!/usr/bin/env node
// clawhip OMC Session Stop Hook
// Emits session.finished on clean exit, session.failed on error.
// Cleans up OMC state files for the session.

import { existsSync, readFileSync, unlinkSync, readdirSync } from 'fs';
import { join, dirname } from 'path';
import { execFileSync } from 'child_process';
import { fileURLToPath } from 'url';

const __filename = fileURLToPath(import.meta.url);
const __dirname = dirname(__filename);

// ---------------------------------------------------------------------------
// Stdin reader (timeout-protected, same pattern as existing OMC hooks)
// ---------------------------------------------------------------------------

function readStdin(timeoutMs = 5000) {
  return new Promise((resolve) => {
    const chunks = [];
    let settled = false;

    const timeout = setTimeout(() => {
      if (!settled) {
        settled = true;
        process.stdin.removeAllListeners();
        process.stdin.destroy();
        resolve(Buffer.concat(chunks).toString('utf-8'));
      }
    }, timeoutMs);

    process.stdin.on('data', (chunk) => { chunks.push(chunk); });

    process.stdin.on('end', () => {
      if (!settled) {
        settled = true;
        clearTimeout(timeout);
        resolve(Buffer.concat(chunks).toString('utf-8'));
      }
    });

    process.stdin.on('error', () => {
      if (!settled) {
        settled = true;
        clearTimeout(timeout);
        resolve('');
      }
    });

    if (process.stdin.readableEnded) {
      if (!settled) {
        settled = true;
        clearTimeout(timeout);
        resolve(Buffer.concat(chunks).toString('utf-8'));
      }
    }
  });
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

function readJsonFile(path) {
  try {
    if (!existsSync(path)) return null;
    return JSON.parse(readFileSync(path, 'utf-8'));
  } catch {
    return null;
  }
}

function tryUnlink(path) {
  try {
    if (existsSync(path)) unlinkSync(path);
  } catch {
    // best-effort cleanup
  }
}

/**
 * Detect session name from environment or worktree/branch heuristics.
 */
function inferSessionName(data) {
  if (data.session_name) return data.session_name;

  // Try CLAWHIP_SESSION env
  if (process.env.CLAWHIP_SESSION) return process.env.CLAWHIP_SESSION;

  // Try tmux session name
  if (process.env.TMUX) {
    try {
      const name = execFileSync('tmux', ['display-message', '-p', '#S'], {
        timeout: 2000, encoding: 'utf-8',
      }).trim();
      if (name) return name;
    } catch {
      // ignore
    }
  }

  return undefined;
}

/**
 * Infer repo name from directory path.
 */
function inferRepoName(directory) {
  if (!directory) return undefined;
  try {
    const name = execFileSync('git', ['-C', directory, 'rev-parse', '--show-toplevel'], {
      timeout: 2000, encoding: 'utf-8',
    }).trim().split('/').pop();
    return name || undefined;
  } catch {
    return directory.split('/').pop() || undefined;
  }
}

/**
 * Infer current branch.
 */
function inferBranch(directory) {
  if (!directory) return undefined;
  try {
    return execFileSync('git', ['-C', directory, 'rev-parse', '--abbrev-ref', 'HEAD'], {
      timeout: 2000, encoding: 'utf-8',
    }).trim() || undefined;
  } catch {
    return undefined;
  }
}

/**
 * Infer issue number from session name or branch name.
 */
function inferIssueNumber(sessionName, branch) {
  for (const str of [sessionName, branch]) {
    if (!str) continue;
    const m = str.match(/(?:issue|feat|fix|bug)[\/\-](\d+)/i);
    if (m) return parseInt(m[1], 10);
  }
  return undefined;
}

// ---------------------------------------------------------------------------
// Event emission via clawhip CLI
// ---------------------------------------------------------------------------

/**
 * Emit a v1 event envelope to clawhip.
 */
function emitEvent(normalizedEvent, context) {
  const envelope = {
    schema_version: '1',
    event: 'notify',
    timestamp: new Date().toISOString(),
    context: {
      normalized_event: normalizedEvent,
      agent_name: 'omc',
      ...context,
    },
  };

  try {
    execFileSync('clawhip', ['omx', 'hook'], {
      input: JSON.stringify(envelope),
      timeout: 5000,
      encoding: 'utf-8',
      stdio: ['pipe', 'pipe', 'pipe'],
    });
    return true;
  } catch {
    // clawhip may not be running or installed - that's okay
    return false;
  }
}

// ---------------------------------------------------------------------------
// State cleanup
// ---------------------------------------------------------------------------

const EPHEMERAL_STATE_FILES = [
  'ultrawork-state.json',
  'autopilot-state.json',
  'ralph-state.json',
  'ultraqa-state.json',
  'team-state.json',
];

/**
 * Clean up ephemeral OMC state files for the finished session.
 * Only removes state files that belong to this session (by session_id match)
 * or that are clearly stale.
 */
function cleanupStateFiles(directory, sessionId) {
  const stateDir = join(directory, '.omc', 'state');
  if (!existsSync(stateDir)) return;

  for (const file of EPHEMERAL_STATE_FILES) {
    const filePath = join(stateDir, file);
    const state = readJsonFile(filePath);
    if (!state) continue;

    // Only clean up if session matches or no session is recorded
    if (state.session_id && sessionId && state.session_id !== sessionId) {
      continue;
    }

    // Only clean up if it was active (don't remove already-completed states)
    if (state.active) {
      tryUnlink(filePath);
    }
  }

  // Clean up session-specific state directory
  const sessionDir = join(stateDir, 'sessions', sessionId);
  if (sessionId && existsSync(sessionDir)) {
    try {
      const files = readdirSync(sessionDir);
      for (const f of files) {
        tryUnlink(join(sessionDir, f));
      }
      // Remove empty session dir (best-effort)
      try { unlinkSync(sessionDir); } catch { /* may need rmdir */ }
    } catch {
      // best-effort
    }
  }
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

async function main() {
  try {
    const input = await readStdin();
    let data = {};
    try { data = JSON.parse(input); } catch {}

    const directory = data.cwd || data.directory || process.cwd();
    const sessionId = data.sessionId || data.session_id || data.sessionid || '';

    // Determine if this is a clean exit or an error.
    // Claude Code stop hooks receive a `reason` field or error context.
    const reason = data.reason || data.stop_reason || '';
    const hasError = data.error || data.error_message ||
      reason === 'error' || reason === 'failed' || reason === 'crash';

    const normalizedEvent = hasError ? 'failed' : 'finished';
    const status = hasError ? 'failed' : 'completed';

    // Build context
    const sessionName = inferSessionName(data);
    const repoName = inferRepoName(directory);
    const branch = inferBranch(directory);
    const issueNumber = inferIssueNumber(sessionName, branch);

    const context = {
      status,
      ...(sessionName && { session_name: sessionName }),
      ...(sessionId && { session_id: sessionId }),
      ...(repoName && { project: repoName, repo_name: repoName }),
      ...(directory && { repo_path: directory, worktree_path: directory }),
      ...(branch && { branch }),
      ...(issueNumber && { issue_number: issueNumber }),
    };

    // Add error details for failed sessions
    if (hasError) {
      const errorMsg = data.error_message || data.error || `Session stopped: ${reason}`;
      context.error_summary = typeof errorMsg === 'string' ? errorMsg : String(errorMsg);
      context.summary = `OMC session failed: ${context.error_summary}`;
    } else {
      context.summary = sessionName
        ? `OMC session "${sessionName}" finished`
        : 'OMC session finished';
    }

    // Emit event to clawhip
    emitEvent(normalizedEvent, context);

    // Clean up ephemeral state files
    cleanupStateFiles(directory, sessionId);

    // Always allow the stop to proceed
    console.log(JSON.stringify({ continue: true, suppressOutput: true }));
  } catch {
    // Never block session stop on hook errors
    console.log(JSON.stringify({ continue: true, suppressOutput: true }));
  }
}

main();
