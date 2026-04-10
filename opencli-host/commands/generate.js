/**
 * Generate a new opencli adapter for a URL.
 * Runs `opencli generate <url>` and saves the resulting adapter.
 */

import { spawn } from 'node:child_process';
import { resolve } from 'node:path';
import { homedir } from 'node:os';

export async function opencliGenerate({ url, name, force = false, timeout = 120 } = {}) {
  const args = ['generate', url];
  if (name) args.push('--name', name);
  if (force) args.push('--force');

  return new Promise((resolve, reject) => {
    const proc = spawn('opencli', args, {
      stdio: ['ignore', 'pipe', 'pipe'],
      timeout: timeout * 1000,
    });

    let stdout = '';
    let stderr = '';
    let killed = false;

    const timer = setTimeout(() => {
      killed = true;
      proc.kill('SIGKILL');
    }, timeout * 1000);

    proc.stdout.on('data', (d) => (stdout += d.toString()));
    proc.stderr.on('data', (d) => (stderr += d.toString()));

    proc.on('close', (code) => {
      clearTimeout(timer);
      if (killed) {
        resolve({ success: false, error: `Generate timed out after ${timeout}s`, url });
        return;
      }

      // Parse generated file path from output
      const pathMatch = stdout.match(/Saved to[:\s]+(.+\.(?:yaml|ts|js))/i);
      const adapterPath = pathMatch ? pathMatch[1].trim() : null;

      // Parse suggested commands
      const commands = [];
      const cmdMatches = stdout.matchAll(/opencli\s+(\S+)\s+(\S+)/g);
      for (const m of cmdMatches) {
        commands.push({ site: m[1], command: m[2] });
      }

      resolve({
        success: code === 0,
        exitCode: code,
        output: stdout.trim(),
        error: stderr.trim(),
        adapterPath,
        suggestedCommands: commands,
        url,
        adapterName: name ?? extractSiteName(url),
      });
    });

    proc.on('error', (err) => {
      clearTimeout(timer);
      resolve({ success: false, error: err.message, url });
    });
  });
}

function extractSiteName(url) {
  try {
    const host = new URL(url).hostname;
    const parts = host.split('.').filter((p) => !['www', 'com', 'org', 'net'].includes(p));
    return parts[parts.length - 1] ?? host;
  } catch {
    return 'site';
  }
}
