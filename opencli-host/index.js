#!/usr/bin/env node
/**
 * opencli-host — Node.js bridge between agent-browser (Rust) and opencli.
 *
 * Communication: JSON-RPC 2.0 over stdin/stdout.
 * Rust daemon spawns this as a long-lived subprocess and sends commands over stdin.
 *
 * Protocol:
 *   Request:  { jsonrpc: "2.0", id: <number>, method: <string>, params: <object> }
 *   Response: { jsonrpc: "2.0", id: <number>, result: <any> }
 *   Error:   { jsonrpc: "2.0", id: <number>, error: { code: <number>, message: <string> } }
 *   Notify:   { jsonrpc: "2.0", method: <string>, params: <object> }  (no id)
 */

import { spawn } from 'node:child_process';
import { createRequire } from 'node:module';
import { resolve, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';
import { opencliList } from './commands/list.js';
import { opencliRun } from './commands/run.js';
import { opencliExplore } from './commands/explore.js';
import { opencliGenerate } from './commands/generate.js';
import { opencliDetect } from './commands/detect.js';
import { getCapabilityForUrl } from './capability.js';

const __dirname = dirname(fileURLToPath(import.meta.url));

// ── Methods registry ──────────────────────────────────────────────────────────

const methods = {
  /** List all available opencli commands */
  'opencli.list': async (params) => {
    const format = params?.format ?? 'json';
    return await opencliList({ format });
  },

  /** Run an opencli command: opencli <site> <command> [args] */
  'opencli.run': async (params) => {
    const { site, command, args = {}, format = 'json', timeout = 60 } = params;
    if (!site || !command) {
      throw new Error('opencli.run requires site and command');
    }
    return await opencliRun({ site, command, args, format, timeout });
  },

  /** Auto-detect site and suggest commands for a URL */
  'opencli.detect': async (params) => {
    const { url } = params;
    if (!url) throw new Error('opencli.detect requires url');
    return await opencliDetect({ url });
  },

  /** Explore a URL and discover its API/capabilities */
  'opencli.explore': async (params) => {
    const { url, timeout = 120 } = params;
    if (!url) throw new Error('opencli.explore requires url');
    return await opencliExplore({ url, timeout });
  },

  /** Generate a new opencli adapter for a URL */
  'opencli.generate': async (params) => {
    const { url, name, force = false } = params;
    if (!url) throw new Error('opencli.generate requires url');
    return await opencliGenerate({ url, name, force });
  },

  /** Check if opencli and its dependencies are installed */
  'opencli.health': async () => {
    return { ok: true, version: '1.7.0' };
  },
};

// ── JSON-RPC dispatch ────────────────────────────────────────────────────────

let requestId = 1;
const pending = new Map();

function sendResponse(id, result) {
  const msg = JSON.stringify({ jsonrpc: '2.0', id, result });
  process.stdout.write(msg + '\n');
}

function sendError(id, code, message) {
  const msg = JSON.stringify({ jsonrpc: '2.0', id, error: { code, message } });
  process.stdout.write(msg + '\n');
}

function sendNotification(method, params) {
  const msg = JSON.stringify({ jsonrpc: '2.0', method, params });
  process.stdout.write(msg + '\n');
}

async function handleMessage(raw) {
  let msg;
  try {
    msg = JSON.parse(raw);
  } catch {
    return;
  }

  // Notification (no id)
  if (!msg.id) {
    if (msg.method === 'shutdown') {
      process.exit(0);
    }
    return;
  }

  const { jsonrpc, id, method, params } = msg;
  if (jsonrpc !== '2.0') {
    sendError(id, -32600, 'Invalid Request');
    return;
  }

  const handler = methods[method];
  if (!handler) {
    sendError(id, -32601, `Method not found: ${method}`);
    return;
  }

  try {
    const result = await handler(params ?? {});
    sendResponse(id, result);
  } catch (err) {
    sendError(id, -32000, err.message ?? String(err));
  }
}

// ── Startup ──────────────────────────────────────────────────────────────────

console.error('[opencli-host] Starting...');

// Read lines from stdin and dispatch
let buffer = '';
process.stdin.setEncoding('utf8');
process.stdin.on('data', (chunk) => {
  buffer += chunk;
  const lines = buffer.split('\n');
  buffer = lines.pop() ?? '';
  for (const line of lines) {
    if (line.trim()) {
      handleMessage(line);
    }
  }
});

process.stdin.on('end', () => {
  if (buffer.trim()) handleMessage(buffer);
});

process.on('SIGTERM', () => process.exit(0));
process.on('SIGINT', () => process.exit(0));

// Signal ready
sendNotification('ready', { pid: process.pid });
console.error('[opencli-host] Ready, waiting for commands...');
