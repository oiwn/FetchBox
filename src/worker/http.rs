//! HTTP client for downloading resources

use bytes::Bytes;
use reqwest::{Client, Proxy};
use std::time::Duration;
use thiserror::Error;
use tracing::{debug, warn};

#[derive(Debug, Error)]
pub enum DownloadError {
    #[error("HTTP request failed: {0}")]
    RequestFailed(String),

    #[error("Connection timeout")]
    Timeout,

    #[error("Invalid URL: {0}")]
    InvalidUrl(String),

    #[error("Too many redirects")]
    TooManyRedirects,
}

pub type Result<T> = std::result::Result<T, DownloadError>;

/// HTTP client configuration
#[derive(Debug, Clone)]
pub struct HttpConfig {
    pub connect_timeout: Duration,
    pub request_timeout: Duration,
    pub max_retries: u32,
    pub user_agent: String,
}

impl Default for HttpConfig {
    fn default() -> Self {
        Self {
            connect_timeout: Duration::from_secs(10),
            request_timeout: Duration::from_secs(60),
            max_retries: 3,
            user_agent: "FetchBox/0.1.0".to_string(),
        }
    }
}

/// HTTP downloader
pub struct HttpClient {
    client: Client,
    config: HttpConfig,
}

impl HttpClient {
    /// Create a new HTTP client
    pub fn new(config: HttpConfig, proxy_url: Option<&str>) -> Result<Self> {
        let mut builder = Client::builder()
            .connect_timeout(config.connect_timeout)
            .timeout(config.request_timeout)
            .user_agent(&config.user_agent)
            .redirect(reqwest::redirect::Policy::limited(10));

        // Configure proxy if provided
        if let Some(url) = proxy_url {
            let proxy = Proxy::all(url)
                .map_err(|e| DownloadError::InvalidUrl(format!("Invalid proxy: {}", e)))?;
            builder = builder.proxy(proxy);
        }

        let client = builder
            .build()
            .map_err(|e| DownloadError::RequestFailed(e.to_string()))?;

        Ok(Self { client, config })
    }

    /// Download a resource with retry
    pub async fn download(
        &self,
        url: &str,
        headers: Vec<(String, String)>,
    ) -> Result<Bytes> {
        let mut attempts = 0;
        let mut last_error = String::new();

        loop {
            attempts += 1;

            match self.download_once(url, &headers).await {
                Ok(bytes) => {
                    if attempts > 1 {
                        debug!(url, attempts, "Download succeeded after retry");
                    }
                    return Ok(bytes);
                }
                Err(e) => {
                    last_error = e.to_string();

                    if attempts >= self.config.max_retries {
                        warn!(url, attempts, error = %last_error, "Download failed after retries");
                        return Err(DownloadError::RequestFailed(format!(
                            "Failed after {} attempts: {}",
                            attempts, last_error
                        )));
                    }

                    warn!(url, attempts, error = %last_error, "Download failed, retrying");

                    // Exponential backoff: 1s, 2s, 4s
                    let backoff = Duration::from_secs(2u64.pow(attempts - 1));
                    tokio::time::sleep(backoff).await;
                }
            }
        }
    }

    /// Download once (no retry)
    async fn download_once(
        &self,
        url: &str,
        headers: &[(String, String)],
    ) -> Result<Bytes> {
        debug!(url, "Starting download");

        let mut request = self.client.get(url);

        // Add custom headers
        for (name, value) in headers {
            request = request.header(name, value);
        }

        let response = request
            .send()
            .await
            .map_err(|e| {
                if e.is_timeout() {
                    DownloadError::Timeout
                } else if e.is_redirect() {
                    DownloadError::TooManyRedirects
                } else {
                    DownloadError::RequestFailed(e.to_string())
                }
            })?;

        // Check HTTP status
        let status = response.status();
        if !status.is_success() {
            return Err(DownloadError::RequestFailed(format!(
                "HTTP {}: {}",
                status.as_u16(),
                status.canonical_reason().unwrap_or("Unknown")
            )));
        }

        // Read response body
        let bytes = response
            .bytes()
            .await
            .map_err(|e| DownloadError::RequestFailed(format!("Failed to read body: {}", e)))?;

        debug!(url, size = bytes.len(), "Download completed");

        Ok(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_http_config_defaults() {
        let config = HttpConfig::default();
        assert_eq!(config.connect_timeout, Duration::from_secs(10));
        assert_eq!(config.request_timeout, Duration::from_secs(60));
        assert_eq!(config.max_retries, 3);
        assert_eq!(config.user_agent, "FetchBox/0.1.0");
    }
}
