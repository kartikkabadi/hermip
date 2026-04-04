#!/usr/bin/env node
// OMX Session Stop Hook — clawhip integration
// Emits session.finished on clean exit, session.failed on error.
// Follows the Claude Code hook contract (stdin JSON, stdout JSON).

import { existsSync, readFileSync } from 'fs';
import { join, dirname } from 'path';
import { execFileSync } from 'child_process';
import { fileURLToPath, pathToFileURL } from 'url';

const __filename = fileURLToPath(import.meta.url);
const __dirname = dirname(__filename);

// ---------------------------------------------------------------------------
// stdin reader (timeout-protected, matches hook convention)
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
// clawhip SDK — lazy import from integrations/omx/clawhip-sdk.mjs
// ---------------------------------------------------------------------------

let clawhipClient = null;

async function getClawhipClient() {
  if (clawhipClient) return clawhipClient;

  const sdkCandidates = [
    join(__dirname, 'lib', 'clawhip-sdk.mjs'),
    join(__dirname, '..', 'omc', 'lib', 'clawhip-sdk.mjs'),
    join(__dirname, '..', '..', 'integrations', 'omx', 'clawhip-sdk.mjs'),
  ];

  for (const candidate of sdkCandidates) {
    if (existsSync(candidate)) {
      try {
        const mod = await import(pathToFileURL(candidate).href);
        clawhipClient = await mod.createClawhipOmxClient();
        return clawhipClient;
      } catch { /* try next */ }
    }
  }
  return null;
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/**
 * Detect session name from environment or worktree/branch heuristics.
 */
function inferSessionName(data) {
  if (data.session_name) return data.session_name;
  if (process.env.CLAWHIP_SESSION) return process.env.CLAWHIP_SESSION;

  if (process.env.TMUX) {
    try {
      const name = execFileSync('tmux', ['display-message', '-p', '#S'], {
        timeout: 2000, encoding: 'utf-8',
      }).trim();
      if (name) return name;
    } catch { /* ignore */ }
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
    const reason = data.reason || data.stop_reason || '';
    const hasError = data.error || data.error_message ||
      reason === 'error' || reason === 'failed' || reason === 'crash';

    const status = hasError ? 'failed' : 'completed';

    // Build context
    const sessionName = inferSessionName(data);
    const repoName = inferRepoName(directory);
    const branch = inferBranch(directory);
    const issueNumber = inferIssueNumber(sessionName, branch);

    const context = {
      agent_name: 'omx',
      status,
      ...(sessionName && { session_name: sessionName }),
      ...(sessionId && { session_id: sessionId }),
      ...(repoName && { project: repoName, repo_name: repoName }),
      ...(directory && { repo_path: directory, worktree_path: directory }),
      ...(branch && { branch }),
      ...(issueNumber && { issue_number: issueNumber }),
    };

    if (hasError) {
      const errorMsg = data.error_message || data.error || `Session stopped: ${reason}`;
      context.error_summary = typeof errorMsg === 'string' ? errorMsg : String(errorMsg);
      context.summary = `OMX session failed: ${context.error_summary}`;
    } else {
      context.summary = sessionName
        ? `OMX session "${sessionName}" finished`
        : 'OMX session finished';
    }

    // Emit event via SDK
    const client = await getClawhipClient();
    if (client) {
      try {
        const emitter = hasError ? client.emitSessionFailed : client.emitSessionFinished;
        await Promise.race([
          emitter({ sessionId, context }),
          new Promise((r) => setTimeout(r, 5000)),
        ]);
      } catch {
        // clawhip may not be running — that's okay
      }
    }

    // Always allow the stop to proceed
    console.log(JSON.stringify({ continue: true, suppressOutput: true }));
  } catch {
    // Never block session stop on hook errors
    console.log(JSON.stringify({ continue: true, suppressOutput: true }));
  }
}

main();
