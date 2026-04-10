/**
 * Capability detection: map a URL → opencli site adapter + available commands.
 * This is the core intelligence that makes agent-browser "know" when it can
 * delegate to opencli instead of doing raw CDP scraping.
 *
 * Strategy hierarchy (from most reliable to least):
 *   1. PUBLIC   — No auth needed, use direct JSON API
 *   2. COOKIE   — Reuse browser's logged-in session via CDP cookies
 *   3. HEADER   — Needs API key injected as header
 *   4. INTERCEPT — Man-in-the-middle API calls
 *   5. UI       — Must use browser interaction (no API)
 */

import { spawn } from 'node:child_process';

// Known site → opencli adapter mapping
const SITE_MAP = {
  'news.ycombinator.com': { site: 'hackernews', strategy: 'public', commands: ['top', 'best', 'new', 'ask', 'show', 'jobs', 'user'] },
  'hackernews': { site: 'hackernews', strategy: 'public', commands: ['top', 'best', 'new', 'ask', 'show', 'jobs', 'user'] },

  'bilibili.com': { site: 'bilibili', strategy: 'cookie', commands: ['hot', 'search', 'history', 'feed', 'ranking', 'download', 'comments', 'dynamic', 'user-videos'] },
  'xiaohongshu.com': { site: 'xiaohongshu', strategy: 'cookie', commands: ['search', 'note', 'comments', 'feed', 'user', 'download', 'publish', 'notifications'] },
  'reddit.com': { site: 'reddit', strategy: 'public', commands: ['hot', 'new', 'top', 'rising', 'search'] },
  'twitter.com': { site: 'twitter', strategy: 'cookie', commands: ['search', 'user', 'tweet'] },
  'x.com': { site: 'twitter', strategy: 'cookie', commands: ['search', 'user', 'tweet'] },
  'github.com': { site: 'github', strategy: 'public', commands: ['search', 'user', 'repo'] },

  // Chinese platforms
  'weibo.com': { site: 'weibo', strategy: 'cookie', commands: ['hot', 'search', 'user', 'timeline'] },
  'zhihu.com': { site: 'zhihu', strategy: 'cookie', commands: ['hot', 'search', 'answer', 'article'] },
  'douban.com': { site: 'douban', strategy: 'cookie', commands: ['hot', 'search', 'review'] },
  'tieba.baidu.com': { site: 'tieba', strategy: 'public', commands: ['hot', 'posts', 'search', 'read'] },
  'hupu.com': { site: 'hupu', strategy: 'public', commands: ['hot', 'search', 'detail', 'reply'] },

  // Tech news
  'techcrunch.com': { site: 'techcrunch', strategy: 'public', commands: ['top', 'search'] },
  'theverge.com': { site: 'theverge', strategy: 'public', commands: ['top', 'search'] },
  'arstechnica.com': { site: 'arstechnica', strategy: 'public', commands: ['top', 'search'] },
  'bbc.com': { site: 'bbc', strategy: 'public', commands: ['top', 'search'] },
  'reuters.com': { site: 'reuters', strategy: 'public', commands: ['top', 'search'] },

  // Commerce
  'amazon.com': { site: 'amazon', strategy: 'cookie', commands: ['search', 'product', 'bestsellers', 'deals'] },
  'jd.com': { site: 'jd', strategy: 'cookie', commands: ['search', 'product', 'price'] },
  'taobao.com': { site: 'taobao', strategy: 'cookie', commands: ['search', 'product'] },
  '1688.com': { site: '1688', strategy: 'cookie', commands: ['search', 'item', 'store'] },

  // Video/Streaming
  'youtube.com': { site: 'youtube', strategy: 'public', commands: ['search', 'trending', 'video'] },
  'tiktok.com': { site: 'tiktok', strategy: 'cookie', commands: ['search', 'user', 'video'] },
  'instagram.com': { site: 'instagram', strategy: 'cookie', commands: ['search', 'user', 'post'] },

  // Academic
  'arxiv.org': { site: 'arxiv', strategy: 'public', commands: ['search', 'recent', 'author'] },
  'scholar.google.com': { site: 'google-scholar', strategy: 'public', commands: ['search', 'author', 'citations'] },

  // Code
  'stackoverflow.com': { site: 'stackoverflow', strategy: 'public', commands: ['search', 'question', 'answers'] },
  'dev.to': { site: 'devto', strategy: 'public', commands: ['top', 'search', 'article'] },
  'producthunt.com': { site: 'producthunt', strategy: 'public', commands: ['top', 'search', 'launches'] },

  // AI
  'chatgpt.com': { site: 'chatgpt', strategy: 'ui', commands: ['chat'] },
  'claude.ai': { site: 'claude', strategy: 'ui', commands: ['chat'] },
  'gemini.google.com': { site: 'gemini', strategy: 'ui', commands: ['chat'] },
  'deepseek.com': { site: 'deepseek', strategy: 'ui', commands: ['chat'] },
  'kimi.moonshot.cn': { site: 'kimi', strategy: 'ui', commands: ['chat'] },
};

// Site name aliases (common URL variations → canonical site name)
const SITE_ALIASES = {
  'x.com': 'twitter',
  'www.x.com': 'twitter',
  'www.twitter.com': 'twitter',
  'mobile.twitter.com': 'twitter',
  'www.hackernews.com': 'hackernews',
  'news.ycombinator.com': 'hackernews',
  'www.bilibili.com': 'bilibili',
  'search.bilibili.com': 'bilibili',
  'www.xiaohongshu.com': 'xiaohongshu',
  'www.zhihu.com': 'zhihu',
  'www.douban.com': 'douban',
  'www.weibo.com': 'weibo',
  'www.reddit.com': 'reddit',
  'www.amazon.com': 'amazon',
  'www.youtube.com': 'youtube',
  'www.instagram.com': 'instagram',
  'www.tiktok.com': 'tiktok',
};

export function getCapabilityForUrl(url) {
  try {
    const parsed = new URL(url);
    const hostname = parsed.hostname.toLowerCase();
    const pathname = parsed.pathname;

    // Direct hostname match
    if (SITE_MAP[hostname]) {
      return { ...SITE_MAP[hostname], url };
    }

    // Alias lookup
    const alias = SITE_ALIASES[hostname];
    if (alias && SITE_MAP[alias]) {
      return { ...SITE_MAP[alias], url };
    }

    // Partial hostname match (e.g., "detail.1688.com" → "1688")
    for (const [pattern, info] of Object.entries(SITE_MAP)) {
      if (hostname.includes(pattern) || pattern.includes(hostname)) {
        return { ...info, url };
      }
    }

    // Try extracting site name from hostname
    const parts = hostname.replace('www.', '').split('.');
    if (parts.length >= 2) {
      const candidate = parts[parts.length - 2];
      // Check if we have a known opencli adapter
      if (SITE_MAP[candidate]) {
        return { ...SITE_MAP[candidate], url };
      }
    }

    return {
      site: null,
      strategy: 'unknown',
      commands: [],
      url,
      recommendation: 'No opencli adapter found. Consider running: agent-browser opencli explore <url>',
    };
  } catch {
    return { site: null, strategy: 'unknown', commands: [], url, error: 'Invalid URL' };
  }
}

/**
 * Get all available commands for a known site.
 * Returns cached registry on first call.
 */
let _cachedList = null;
let _cacheTime = 0;
const CACHE_TTL = 60_000; // 1 minute

export async function getSiteCommands(site) {
  const now = Date.now();
  if (!_cachedList || now - _cacheTime > CACHE_TTL) {
    try {
      const proc = spawn('opencli', ['list', '--format=json'], {
        stdio: ['ignore', 'pipe', 'pipe'],
        timeout: 15000,
      });

      let stdout = '';
      proc.stdout.on('data', (d) => (stdout += d.toString()));
      await new Promise((r) => proc.on('close', r));

      try {
        const list = JSON.parse(stdout);
        _cachedList = list;
        _cacheTime = now;
      } catch {
        // ignore parse errors
      }
    } catch {
      // ignore spawn errors
    }
  }

  if (!_cachedList || !Array.isArray(_cachedList)) return [];

  return _cachedList
    .filter((cmd) => {
      const key = cmd.site ?? cmd.command ?? '';
      return key.includes(site);
    })
    .map((cmd) => ({
      site: cmd.site,
      command: cmd.name ?? cmd.command ?? '',
      description: cmd.description ?? '',
      strategy: cmd.strategy ?? 'unknown',
      browser: cmd.browser ?? false,
    }));
}
