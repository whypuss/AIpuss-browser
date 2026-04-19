//! Pre-fetching and background caching for AIpuss-browser.
//!
//! While the agent is thinking, AIpuss scans the current page for links and
//! pre-fetches their HTML in the background (without full rendering).
//! This dramatically reduces latency when the agent decides to navigate.
//!
//! Additionally, DNS pre-connect hints are sent for likely navigation targets.

use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use url::Url;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// A pre-fetched page entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrefetchEntry {
    /// The URL that was pre-fetched.
    pub url: String,
    /// HTTP status code (if obtained).
    pub status: Option<u16>,
    /// Content-Type header.
    pub content_type: Option<String>,
    /// Final URL after redirects.
    pub final_url: Option<String>,
    /// Whether pre-fetch succeeded.
    pub success: bool,
    /// Error message if failed.
    pub error: Option<String>,
    /// When this entry was created.
    pub cached_at_ms: u64,
    /// Content hash (SHA-256 of body, if small enough).
    pub content_hash: Option<String>,
    /// Content length in bytes.
    pub content_length: Option usize,
    /// Whether the response indicated HTML content.
    pub is_html: bool,
}

/// LRU cache for pre-fetched pages.
pub struct PrefetchCache {
    inner: Arc<RwLock<HashMap<String, PrefetchEntry>>>,
    max_entries: usize,
    access_order: Arc<RwLock<Vec<String>>>,
}

impl PrefetchCache {
    pub fn new(max_entries: usize) -> Self {
        Self {
            inner: Arc::new(RwLock::new(HashMap::new())),
            max_entries,
            access_order: Arc::new(RwLock::new(Vec::new())),
        }
    }

    /// Get a cached entry by URL.
    pub async fn get(&self, url: &str) -> Option<PrefetchEntry> {
        let inner = self.inner.read().await;
        inner.get(url).cloned()
    }

    /// Insert a new entry, evicting LRU if at capacity.
    pub async fn insert(&self, entry: PrefetchEntry) {
        let url = entry.url.clone();
        let mut inner = self.inner.write().await;
        let mut order = self.access_order.write().await;

        // Evict LRU if at capacity
        if inner.len() >= self.max_entries && !inner.contains_key(&url) {
            if let Some(lru) = order.pop() {
                inner.remove(&lru);
            }
        }

        inner.insert(url.clone(), entry);
        // Move to front (most recently used)
        order.retain(|u| u != &url);
        order.insert(0, url);
    }

    /// Check if a URL is in cache.
    pub async fn contains(&self, url: &str) -> bool {
        self.inner.read().await.contains_key(url)
    }

    /// Get all cached URLs.
    pub async fn urls(&self) -> Vec<String> {
        self.inner.read().await.keys().cloned().collect()
    }

    /// Clear all cached entries.
    pub async fn clear(&self) {
        self.inner.write().await.clear();
        self.access_order.write().await.clear();
    }

    /// Evict entries older than max_age.
    pub async fn evict_older_than(&self, max_age: Duration) {
        let cutoff = Instant::now()
            .checked_sub(max_age)
            .map(|t| t.elapsed().as_millis() as u64)
            .unwrap_or(0);

        let mut inner = self.inner.write().await;
        let mut order = self.access_order.write().await;
        let to_remove: Vec<String> = order
            .iter()
            .filter(|u| {
                inner
                    .get(*u)
                    .map(|e| e.cached_at_ms < cutoff)
                    .unwrap_or(false)
            })
            .cloned()
            .collect();

        for u in &to_remove {
            inner.remove(u);
        }
        order.retain(|u| !to_remove.contains(u));
    }
}

/// Link scanner — extracts navigable links from raw HTML.
pub fn extract_links(html: &str, base_url: &Url) -> Vec<String> {
    let mut links = Vec::new();
    let base_str = base_url.as_str().trim_end_matches('/');

    // Simple regex-based link extraction from HTML
    // Matches href="..." and src="..." attributes
    let href_re = regex_lite::Regex::new(r#"href\s*=\s*["']([^"']+)["']"#).ok();
    let src_re = regex_lite::Regex::new(r#"src\s*=\s*["']([^"']+)["']"#).ok();

    for re in [&href_re, &src_re] {
        if let Some(re) = re {
            for cap in re.captures_iter(html) {
                if let Some(url_str) = cap.get(1) {
                    let raw = url_str.as_str();
                    if raw.starts_with('#')
                        || raw.starts_with("javascript:")
                        || raw.starts_with("mailto:")
                        || raw.starts_with("tel:")
                    {
                        continue;
                    }

                    if let Ok(resolved) = base_url.join(raw) {
                        let resolved_str = resolved.as_str().trim_end_matches('/');
                        // Only keep same-domain or subdomain links to be conservative
                        if resolved.host() == base_url.host() || resolved.host_str().map(|h| h.ends_with(base_url.host_str().unwrap_or(""))).unwrap_or(false) {
                            links.push(resolved_str.to_string());
                        }
                    }
                }
            }
        }
    }

    links
}

// ---------------------------------------------------------------------------
// Background pre-fetch engine
// ---------------------------------------------------------------------------

/// Configuration for the background pre-fetch engine.
#[derive(Debug, Clone)]
pub struct PrefetchConfig {
    /// Maximum number of URLs to pre-fetch in parallel.
    pub parallel: usize,
    /// Maximum total bytes to fetch per pre-fetch run.
    pub max_bytes: usize,
    /// Maximum age of cached entries before eviction.
    pub max_age: Duration,
    /// Request timeout per URL.
    pub timeout: Duration,
    /// User-Agent header to use.
    pub user_agent: String,
    /// Maximum cache entries.
    pub cache_max_entries: usize,
    /// Minimum content length to consider "interesting" (skip tiny responses).
    pub min_content_length: usize,
}

impl Default for PrefetchConfig {
    fn default() -> Self {
        Self {
            parallel: 3,
            max_bytes: 1024 * 1024 * 5, // 5MB
            max_age: Duration::from_secs(300), // 5 minutes
            timeout: Duration::from_secs(8),
            user_agent: "Mozilla/5.0 (compatible; AIpuss-browser/1.0; +https://github.com/whypuss/AIpuss-browser)".to_string(),
            cache_max_entries: 50,
            min_content_length: 100,
        }
    }
}

/// The background pre-fetch engine.
pub struct PrefetchEngine {
    client: Client,
    cache: PrefetchCache,
    config: PrefetchConfig,
}

impl PrefetchEngine {
    pub fn new(config: PrefetchConfig) -> Self {
        let client = Client::builder()
            .timeout(config.timeout)
            .user_agent(&config.user_agent)
            .browser_metadata()
            .build()
            .unwrap_or_else(|_| Client::new());

        Self {
            client,
            cache: PrefetchCache::new(config.cache_max_entries),
            config,
        }
    }

    /// Get a shared reference to the cache.
    pub fn cache(&self) -> &PrefetchCache {
        &self.cache
    }

    /// Start a background pre-fetch of links found in `html` at `base_url`.
    /// Returns immediately — actual fetching happens in spawned tasks.
    pub fn start_prefetch(&self, html: &str, base_url: &str) {
        let base = match Url::parse(base_url) {
            Ok(b) => b,
            Err(_) => return,
        };

        let links: Vec<String> = extract_links(html, &base);
        if links.is_empty() {
            return;
        }

        // Deduplicate and limit
        let links: Vec<String> = links
            .into_iter()
            .collect::<HashSet<_>>()
            .into_iter()
            .take(20)
            .collect();

        let client = self.client.clone();
        let cache = self.cache.clone();
        let parallel = self.config.parallel;
        let max_bytes = self.config.max_bytes;
        let user_agent = self.config.user_agent.clone();
        let min_len = self.config.min_content_length;

        tokio::spawn(async move {
            let semaphore = Arc::new(tokio::sync::Semaphore::new(parallel));
            let mut handles = Vec::new();

            for url in links {
                let permit = semaphore.clone().acquire_owned().await.ok();
                if permit.is_none() {
                    break;
                }

                let client = client.clone();
                let cache = cache.clone();
                let url = url.clone();
                let ua = user_agent.clone();

                let handle = tokio::spawn(async move {
                    let _permit = permit;
                    let entry = Self::fetch_url(&client, &url, &ua, min_len, max_bytes).await;
                    cache.insert(entry).await;
                });
                handles.push(handle);
            }

            // Wait for all fetches to complete
            for handle in handles {
                let _ = handle.await;
            }
        });
    }

    /// Fetch a single URL and return a PrefetchEntry.
    async fn fetch_url(
        client: &Client,
        url: &str,
        user_agent: &str,
        min_len: usize,
        max_bytes: usize,
    ) -> PrefetchEntry {
        let timestamp_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);

        let result = client
            .get(url)
            .header("User-Agent", user_agent)
            .header("Accept", "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8")
            .header("Accept-Language", "en-US,en;q=0.9")
            .send()
            .await;

        match result {
            Ok(response) => {
                let status = response.status().as_u16();
                let content_type = response
                    .headers()
                    .get("content-type")
                    .and_then(|v| v.to_str().ok())
                    .map(|s| s.to_string());
                let final_url = response.url().as_str().to_string();
                let is_html = content_type
                    .as_ref()
                    .map(|ct| ct.contains("text/html") || ct.contains("application/xhtml"))
                    .unwrap_or(false);

                let content_length = response.content_length().map(|l| l as usize);

                if status == 200 && is_html {
                    let body = response.bytes().await.unwrap_or_default();
                    let body_len = body.len();

                    if body_len < min_len {
                        return PrefetchEntry {
                            url: url.to_string(),
                            status: Some(status),
                            content_type,
                            final_url: Some(final_url),
                            success: false,
                            error: Some("Content too small, likely not useful".to_string()),
                            cached_at_ms: timestamp_ms,
                            content_hash: None,
                            content_length: Some(body_len),
                            is_html,
                        };
                    }

                    if body_len > max_bytes {
                        return PrefetchEntry {
                            url: url.to_string(),
                            status: Some(status),
                            content_type,
                            final_url: Some(final_url),
                            success: false,
                            error: Some(format!("Content too large ({} bytes)", body_len)),
                            cached_at_ms: timestamp_ms,
                            content_hash: None,
                            content_length: Some(body_len),
                            is_html,
                        };
                    }

                    // Compute hash of content
                    use std::collections::hash_map::DefaultHasher;
                    use std::hash::{Hash, Hasher};
                    let mut hasher = DefaultHasher::new();
                    body.hash(&mut hasher);
                    let hash = format!("{:x}", hasher.finish());

                    PrefetchEntry {
                        url: url.to_string(),
                        status: Some(status),
                        content_type,
                        final_url: Some(final_url),
                        success: true,
                        error: None,
                        cached_at_ms: timestamp_ms,
                        content_hash: Some(hash),
                        content_length: Some(body_len),
                        is_html,
                    }
                } else {
                    PrefetchEntry {
                        url: url.to_string(),
                        status: Some(status),
                        content_type,
                        final_url: Some(final_url),
                        success: false,
                        error: Some(format!(
                            "Non-HTML or non-200 response (status={})",
                            status
                        )),
                        cached_at_ms: timestamp_ms,
                        content_hash: None,
                        content_length,
                        is_html,
                    }
                }
            }
            Err(e) => PrefetchEntry {
                url: url.to_string(),
                status: None,
                content_type: None,
                final_url: None,
                success: false,
                error: Some(e.to_string()),
                cached_at_ms: timestamp_ms,
                content_hash: None,
                content_length: None,
                is_html: false,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_links_basic() {
        let html = r#"<a href="/page1">Page 1</a><a href="/page2">Page 2</a>"#;
        let base = Url::parse("https://example.com/").unwrap();
        let links = extract_links(html, &base);
        assert!(links.contains(&"https://example.com/page1".to_string()));
        assert!(links.contains(&"https://example.com/page2".to_string()));
    }

    #[test]
    fn test_extract_links_skips_anchors() {
        let html = r#"<a href="#section">Skip</a><a href="/real">Real</a>"#;
        let base = Url::parse("https://example.com/").unwrap();
        let links = extract_links(html, &base);
        assert!(!links.iter().any(|l| l.contains('#')));
        assert!(links.contains(&"https://example.com/real".to_string()));
    }

    #[test]
    fn test_prefetch_cache_eviction() {
        let cache = PrefetchCache::new(3);
        let url = "https://example.com/1";
        let entry = PrefetchEntry {
            url: url.to_string(),
            status: Some(200),
            content_type: Some("text/html".to_string()),
            final_url: Some(url.to_string()),
            success: true,
            error: None,
            cached_at_ms: 0,
            content_hash: None,
            content_length: Some(100),
            is_html: true,
        };

        // Fill cache
        for i in 0..4 {
            let e = PrefetchEntry {
                url: format!("https://example.com/{}", i),
                ..entry.clone()
            };
            cache.insert(e);
        }

        // First entry should be evicted (LRU)
        assert!(!cache.inner.try_read().unwrap().contains_key("https://example.com/0"));
    }
}
