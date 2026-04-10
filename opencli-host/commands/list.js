/**
 * List all available opencli commands.
 * Bridges to `opencli list --format json`.
 */

import { spawn } from 'node:child_process';

export async function opencliList({ format = 'json' } = {}) {
  return new Promise((resolve, reject) => {
    const proc = spawn('opencli', ['list', '--format', format], {
      stdio: ['ignore', 'pipe', 'pipe'],
      timeout: 10000,
    });

    let stdout = '';
    let stderr = '';

    proc.stdout.on('data', (d) => (stdout += d.toString()));
    proc.stderr.on('data', (d) => (stderr += d.toString()));

    proc.on('close', (code) => {
      if (code === 0) {
        try {
          const result = format === 'json' ? JSON.parse(stdout) : stdout;
          resolve(result);
        } catch {
          resolve({ raw: stdout });
        }
      } else {
        reject(new Error(`opencli list failed: ${stderr || stdout}`));
      }
    });

    proc.on('error', (err) => reject(err));
  });
}
