/**
 * Run an opencli command: <site> <command> [args].
 * Example: opencli hackernews top --limit 5
 */

import { spawn } from 'node:child_process';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { createRequire } from 'node:module';

const __dirname = fileURLToPath(new URL('.', import.meta.url));

export async function opencliRun({ site, command, args = {}, format = 'json', timeout = 60 } = {}) {
  return new Promise((resolve, reject) => {
    const flags = [`--format=${format}`];

    for (const [key, value] of Object.entries(args)) {
      if (value === true) {
        flags.push(`--${key}`);
      } else if (value !== false && value !== null && value !== undefined) {
        flags.push(`--${key}`, String(value));
      }
    }

    // Build the opencli args: <site> <command> [...flags]
    const opencliArgs = [site, command, ...flags];

    const proc = spawn('opencli', opencliArgs, {
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
        reject(new Error(`Command timed out after ${timeout}s`));
        return;
      }

      if (code === 0) {
        try {
          const result = format === 'json' ? JSON.parse(stdout) : stdout;
          resolve({ success: true, output: result, site, command, format });
        } catch {
          resolve({ success: true, output: stdout.trim(), site, command, format, parseError: true });
        }
      } else {
        resolve({
          success: false,
          error: stderr || stdout,
          site,
          command,
          exitCode: code,
        });
      }
    });

    proc.on('error', (err) => {
      clearTimeout(timer);
      reject(new Error(`Failed to spawn opencli: ${err.message}`));
    });
  });
}
