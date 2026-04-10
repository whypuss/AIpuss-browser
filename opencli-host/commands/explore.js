/**
 * Explore a URL: run `opencli explore <url>`.
 * Discovers APIs, network activity, and infers CLI capabilities.
 */

import { spawn } from 'node:child_process';

export async function opencliExplore({ url, timeout = 120 } = {}) {
  return new Promise((resolve, reject) => {
    const proc = spawn('opencli', ['explore', url], {
      stdio: ['ignore', 'pipe', 'pipe'],
      timeout: timeout * 1000,
      env: { ...process.env, OPENCLI_BROWSER_EXPLORE_TIMEOUT: String(timeout) },
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
        resolve({ success: false, error: `Explore timed out after ${timeout}s`, url });
        return;
      }

      resolve({
        success: code === 0,
        exitCode: code,
        output: stdout,
        error: stderr,
        url,
        suggestedCommands: parseSuggestedCommands(stdout),
      });
    });

    proc.on('error', (err) => {
      clearTimeout(timer);
      resolve({ success: false, error: err.message, url });
    });
  });
}

/** Heuristic: extract command suggestions from opencli explore output */
function parseSuggestedCommands(output) {
  const suggestions = [];
  const lines = output.split('\n');
  for (const line of lines) {
    // Match patterns like "opencli site command" or "Try: opencli ..."
    const match = line.match(/opencli\s+(\S+)\s+(\S+)/);
    if (match) {
      suggestions.push({ site: match[1], command: match[2] });
    }
  }
  return suggestions;
}
