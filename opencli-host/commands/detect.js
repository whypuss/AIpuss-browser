/**
 * Detect: given a URL, find matching opencli site adapters.
 * Uses opencli's KNOWN_SITE_ALIASES and registry lookup.
 */

import { spawn } from 'node:child_process';
import { opencliList } from './list.js';
import { getCapabilityForUrl } from '../capability.js';

export async function opencliDetect({ url } = {}) {
  const capability = await getCapabilityForUrl(url);

  // Try to run the site's default command to verify adapter works
  let adapterTest = null;
  if (capability.site && capability.strategy !== 'unknown') {
    try {
      adapterTest = await testAdapter(capability.site, url);
    } catch {
      // ignore
    }
  }

  return {
    url,
    matched: capability.strategy !== 'unknown',
    site: capability.site,
    strategy: capability.strategy,
    suggestedCommands: capability.commands,
    adapterTest,
    isLoggedInRequired: capability.strategy === 'cookie' || capability.strategy === 'header',
  };
}

async function testAdapter(site, url) {
  return new Promise((resolve) => {
    // Try a read-only command to verify the adapter works
    const proc = spawn('opencli', [site, 'search', '--limit', '1', url.split('/').pop() ?? ''], {
      stdio: ['ignore', 'pipe', 'pipe'],
      timeout: 15000,
    });

    let stderr = '';
    proc.stderr.on('data', (d) => (stderr += d.toString()));
    proc.on('close', (code) => {
      resolve({ tested: site, success: code === 0, exitCode: code });
    });
    proc.on('error', () => {
      resolve({ tested: site, success: false, error: 'process error' });
    });
  });
}
