#!/usr/bin/env node
// OMX Session Init Hook — clawhip integration
// Emits session.started to clawhip via the OMX SDK.
// Follows the Claude Code hook contract (stdin JSON, stdout JSON).

import { existsSync, readFileSync } from 'fs';
import { join, dirname } from 'path';
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
// Emit session.started to clawhip
// ---------------------------------------------------------------------------

async function emitSessionStarted(client, { directory, sessionId, branch }) {
  if (!client) return null;

  try {
    return await client.emitSessionStarted({
      sessionId,
      context: {
        agent_name: 'omx',
        session_id: sessionId || undefined,
        repo_path: directory,
        worktree_path: directory,
        branch: branch || undefined,
        status: 'started',
      },
    });
  } catch {
    // Silent fail — clawhip daemon may not be running
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

    // Emit session.started to clawhip (non-blocking, best-effort)
    const branch = detectBranch(directory);
    const client = await getClawhipClient();
    const emitPromise = emitSessionStarted(client, {
      directory,
      sessionId,
      branch,
    });

    // Don't block hook output on clawhip delivery
    emitPromise?.catch?.(() => {});

    // OMX session init has no additional context to inject
    console.log(JSON.stringify({ continue: true, suppressOutput: true }));

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
