#!/usr/bin/env node
// OMC Session Init Hook — clawhip integration
// Emits session.started to clawhip, restores ultrawork state, loads notepad priority context.
// Follows the OMC session-start hook contract (stdin JSON, stdout JSON with hookSpecificOutput).

import { existsSync, readFileSync, mkdirSync } from 'fs';
import { join, resolve, normalize } from 'path';
import { homedir } from 'os';
import { dirname, sep } from 'path';
import { fileURLToPath, pathToFileURL } from 'url';

const __filename = fileURLToPath(import.meta.url);
const __dirname = dirname(__filename);

// ---------------------------------------------------------------------------
// stdin reader (timeout-protected, matches OMC hook convention)
// ---------------------------------------------------------------------------

let readStdin;
try {
  // Prefer shared OMC lib if co-located (e.g. ~/.claude/hooks/lib/stdin.mjs)
  const candidates = [
    join(__dirname, 'lib', 'stdin.mjs'),
    join(__dirname, '..', 'lib', 'stdin.mjs'),
  ];
  for (const candidate of candidates) {
    if (existsSync(candidate)) {
      const mod = await import(pathToFileURL(candidate).href);
      readStdin = mod.readStdin;
      break;
    }
  }
} catch { /* fall through to inline fallback */ }

if (!readStdin) {
  readStdin = (timeoutMs = 5000) => new Promise((res) => {
    const chunks = [];
    let settled = false;
    const timeout = setTimeout(() => {
      if (!settled) { settled = true; process.stdin.removeAllListeners(); process.stdin.destroy(); res(Buffer.concat(chunks).toString('utf-8')); }
    }, timeoutMs);
    process.stdin.on('data', (c) => chunks.push(c));
    process.stdin.on('end', () => { if (!settled) { settled = true; clearTimeout(timeout); res(Buffer.concat(chunks).toString('utf-8')); } });
    process.stdin.on('error', () => { if (!settled) { settled = true; clearTimeout(timeout); res(''); } });
    if (process.stdin.readableEnded) { if (!settled) { settled = true; clearTimeout(timeout); res(Buffer.concat(chunks).toString('utf-8')); } }
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
    join(__dirname, '..', 'omx', 'lib', 'clawhip-sdk.mjs'),
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
// JSON file helpers
// ---------------------------------------------------------------------------

function readJsonFile(path) {
  try {
    if (!existsSync(path)) return null;
    return JSON.parse(readFileSync(path, 'utf-8'));
  } catch {
    return null;
  }
}

// ---------------------------------------------------------------------------
// Ultrawork state restore (mirrors OMC session-start.mjs logic)
// ---------------------------------------------------------------------------

const STALE_STATE_THRESHOLD_MS = 2 * 60 * 60 * 1000; // 2 hours

function normalizePath(p) {
  if (!p || typeof p !== 'string') return '';
  let normalized = resolve(p);
  normalized = normalize(normalized).replace(/[\/\\]+$/, '');
  if (process.platform === 'win32') normalized = normalized.toLowerCase();
  return normalized;
}

function isFreshActiveState(state) {
  if (!state?.active) return false;
  const startedAt = state.started_at ? new Date(state.started_at).getTime() : 0;
  const lastCheckedAt = state.last_checked_at ? new Date(state.last_checked_at).getTime() : 0;
  const recencyMs = Math.max(startedAt || 0, lastCheckedAt || 0);
  if (!Number.isFinite(recencyMs) || recencyMs <= 0) return false;
  return (Date.now() - recencyMs) <= STALE_STATE_THRESHOLD_MS;
}

function getUltraworkState(directory, sessionId) {
  const localPath = join(directory, '.omc', 'state', 'ultrawork-state.json');
  const globalPath = join(homedir(), '.omc', 'state', 'ultrawork-state.json');

  for (const { path, source } of [
    { path: localPath, source: 'local' },
    { path: globalPath, source: 'global' },
  ]) {
    const state = readJsonFile(path);
    if (!state?.active) continue;

    // Skip if owned by a different session
    if (sessionId && typeof state.session_id === 'string' && state.session_id && state.session_id !== sessionId) {
      if (source === 'global' && typeof state.project_path === 'string' && state.project_path) {
        if (normalizePath(state.project_path) !== normalizePath(directory)) continue;
      }
      return { state, collision: true, source };
    }

    if (!state.session_id || state.session_id === sessionId) {
      return { state, collision: false, source };
    }
  }
  return { state: null, collision: false, source: null };
}

// ---------------------------------------------------------------------------
// Notepad priority context (mirrors OMC session-start.mjs logic)
// ---------------------------------------------------------------------------

function getPriorityContext(directory) {
  const notepadPath = join(directory, '.omc', 'notepad.md');
  if (!existsSync(notepadPath)) return null;

  try {
    const content = readFileSync(notepadPath, 'utf-8');
    const regex = /## Priority Context\n([\s\S]*?)(?=\n## [^#]|$)/;
    const match = content.match(regex);
    if (!match) return null;
    const section = match[1].replace(/<!--[\s\S]*?-->/g, '').trim();
    return section || null;
  } catch {
    return null;
  }
}

// ---------------------------------------------------------------------------
// Emit session.started to clawhip
// ---------------------------------------------------------------------------

async function emitSessionStarted(client, { directory, sessionId, branch, ultraworkActive }) {
  if (!client) return null;

  try {
    return await client.emitSessionStarted({
      sessionId,
      context: {
        agent_name: 'omc',
        session_id: sessionId || undefined,
        repo_path: directory,
        branch: branch || undefined,
        status: 'started',
        ultrawork_active: ultraworkActive ? 'true' : undefined,
      },
    });
  } catch {
    // Silent fail — clawhip daemon may not be running
    return null;
  }
}

// ---------------------------------------------------------------------------
// Detect current git branch (best-effort, no git dependency required)
// ---------------------------------------------------------------------------

function detectBranch(directory) {
  try {
    const headPath = join(directory, '.git', 'HEAD');
    if (!existsSync(headPath)) {
      // Worktree: .git is a file with "gitdir: ..." content
      const gitFile = join(directory, '.git');
      if (existsSync(gitFile)) {
        const content = readFileSync(gitFile, 'utf-8').trim();
        const match = content.match(/^gitdir:\s*(.+)$/m);
        if (match) {
          const realHead = join(match[1].trim(), 'HEAD');
          if (existsSync(realHead)) {
            const ref = readFileSync(realHead, 'utf-8').trim();
            const branchMatch = ref.match(/^ref:\s*refs\/heads\/(.+)$/);
            return branchMatch ? branchMatch[1] : null;
          }
        }
      }
      return null;
    }
    const ref = readFileSync(headPath, 'utf-8').trim();
    const branchMatch = ref.match(/^ref:\s*refs\/heads\/(.+)$/);
    return branchMatch ? branchMatch[1] : null;
  } catch {
    return null;
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
    const messages = [];

    // 1. Restore ultrawork state if active
    const ultrawork = getUltraworkState(directory, sessionId);
    let ultraworkActive = false;

    if (ultrawork.collision) {
      messages.push(`<session-restore>

[PARALLEL SESSION WARNING]

Detected an active ultrawork session for ${ultrawork.source === 'global' ? 'matching project path in the shared global fallback state' : 'this repo root'}.
Owner session: ${ultrawork.state?.session_id || 'another session'}
Started: ${ultrawork.state?.started_at || 'unknown'}

OMC suppressed the restore for this session to avoid state bleed.

</session-restore>

---
`);
    } else if (ultrawork.state && isFreshActiveState(ultrawork.state)) {
      ultraworkActive = true;
      messages.push(`<session-restore>

[ULTRAWORK MODE RESTORED]

You have an active ultrawork session from ${ultrawork.state.started_at}.
Original task: ${ultrawork.state.original_prompt || ultrawork.state.prompt || '(unknown)'}

Continue working in ultrawork mode until all tasks are complete.

</session-restore>

---
`);
    }

    // 2. Load notepad priority context
    const priorityContext = getPriorityContext(directory);
    if (priorityContext) {
      messages.push(`<session-restore>

[NOTEPAD PRIORITY CONTEXT LOADED]

<notepad-priority>

## Priority Context

${priorityContext}

</notepad-priority>

</session-restore>

---
`);
    }

    // 3. Emit session.started to clawhip (non-blocking, best-effort)
    const branch = detectBranch(directory);
    const client = await getClawhipClient();
    const emitPromise = emitSessionStarted(client, {
      directory,
      sessionId,
      branch,
      ultraworkActive,
    });

    // Don't block hook output on clawhip delivery
    emitPromise?.catch?.(() => {});

    if (messages.length > 0) {
      console.log(JSON.stringify({
        continue: true,
        hookSpecificOutput: {
          hookEventName: 'SessionStart',
          additionalContext: messages.join('\n'),
        },
      }));
    } else {
      console.log(JSON.stringify({ continue: true, suppressOutput: true }));
    }

    // Wait briefly for the emit to complete (best-effort, don't hang)
    await Promise.race([
      emitPromise,
      new Promise((r) => setTimeout(r, 3000)),
    ]).catch(() => {});
  } catch {
    console.log(JSON.stringify({ continue: true, suppressOutput: true }));
  }
}

main();
