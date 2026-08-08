//! The document-mode binding: one signed HTTP exchange for one contract envelope.
//!
//! This module knows HTTP and no instructions. A successful response is returned as opaque bytes
//! in [`sipx_app_protocol::Response::Body`]'s vocabulary; only the contract interpreter parses it.

use std::fmt::Write as _;
use std::sync::Arc;
use std::time::Duration;

use hmac::{Hmac, Mac};
use reqwest::{Client, redirect};
use sha2::Sha256;
use sipx_app_protocol::{Envelope, Failure, Response};
use tokio::time::{Instant, timeout_at};

const BACKOFF_MS: [u64; 2] = [100, 200];
const MAX_RESPONSE_BYTES: usize = 1024 * 1024;

/// A completed document delivery, in the interpreter's own response vocabulary.
pub type Delivery = Response;

/// The host-wide HTTP client whose connection pool document bindings share.
///
/// Construct one of these for a process and pass it to [`Webhook::with_client`] for every app.
/// Cloning it retains the same pool; it does not construct another client.
#[derive(Clone)]
pub struct WebhookClient {
    inner: Arc<Client>,
}

impl std::fmt::Debug for WebhookClient {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("WebhookClient")
            .finish_non_exhaustive()
    }
}

impl WebhookClient {
    /// Construct a pooled client with redirects disabled for every binding that shares it.
    pub fn new() -> Result<Self, WebhookError> {
        let client = Client::builder()
            .redirect(redirect::Policy::none())
            .build()
            .map_err(|error| WebhookError::Client(error.to_string()))?;
        Ok(Self {
            inner: Arc::new(client),
        })
    }

    #[cfg(test)]
    fn shares_pool_with(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.inner, &other.inner)
    }
}

/// A pooled HTTP client and one app's immutable webhook declaration.
#[derive(Clone)]
pub struct Webhook {
    client: WebhookClient,
    url: reqwest::Url,
    secrets: Vec<Vec<u8>>,
}

impl std::fmt::Debug for Webhook {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Webhook")
            .field("url", &self.url)
            .field("signing_keys", &self.secrets.len())
            .finish_non_exhaustive()
    }
}

/// Why a webhook declaration could not become a usable binding.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum WebhookError {
    /// The URL is not an absolute HTTP(S) URL.
    Url,
    /// At least one signing key is required, and none may be empty.
    Secret,
    /// The HTTP client could not be constructed.
    Client(String),
}

impl std::fmt::Display for WebhookError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Url => formatter.write_str("the webhook URL must be absolute HTTP or HTTPS"),
            Self::Secret => {
                formatter.write_str("one or two non-empty signing secrets are required")
            }
            Self::Client(error) => write!(formatter, "the HTTP client could not start: {error}"),
        }
    }
}

impl std::error::Error for WebhookError {}

impl Webhook {
    /// Construct one standalone binding with its own pool.
    ///
    /// A multi-app host should construct one [`WebhookClient`] and use [`Self::with_client`] so
    /// every binding shares it.
    pub fn new(url: &str, secrets: Vec<Vec<u8>>) -> Result<Self, WebhookError> {
        let client = WebhookClient::new()?;
        Self::with_client(&client, url, secrets)
    }

    /// Construct a binding over a host-owned client and its shared connection pool.
    pub fn with_client(
        client: &WebhookClient,
        url: &str,
        secrets: Vec<Vec<u8>>,
    ) -> Result<Self, WebhookError> {
        let url = reqwest::Url::parse(url).map_err(|_| WebhookError::Url)?;
        if !matches!(url.scheme(), "http" | "https") || url.host().is_none() {
            return Err(WebhookError::Url);
        }
        if secrets.is_empty() || secrets.len() > 2 || secrets.iter().any(Vec::is_empty) {
            return Err(WebhookError::Secret);
        }
        Ok(Self {
            client: client.clone(),
            url,
            secrets,
        })
    }

    /// Deliver one envelope within the app's whole callback budget.
    ///
    /// `unix_seconds` is supplied by the host's clock alongside the envelope timestamp. It is a
    /// parameter so fixed vectors need no clock, and it is computed once so every retry is signed
    /// identically.
    pub async fn deliver(
        &self,
        envelope: &Envelope,
        budget: Duration,
        unix_seconds: i64,
    ) -> Delivery {
        let body = envelope.to_text();
        let signature = signature(unix_seconds, body.as_bytes(), &self.secrets);
        let deadline = Instant::now() + budget;

        for delay_after in BACKOFF_MS.into_iter().map(Some).chain([None]) {
            let exchange = self.exchange(&body, &signature);
            let Ok(result) = timeout_at(deadline, exchange).await else {
                return Response::Failed(Failure::Timeout);
            };
            match result {
                Attempt::Body(body) => return Response::Body(body),
                Attempt::ClientError => return Response::Failed(Failure::ClientError),
                Attempt::BindingError => return Response::Failed(Failure::ServerError),
                Attempt::ServerError | Attempt::Unreachable => {
                    let Some(delay_ms) = delay_after else {
                        return result.failure();
                    };
                    let delay = Duration::from_millis(delay_ms);
                    let Some(wake) = Instant::now().checked_add(delay) else {
                        return result.failure();
                    };
                    if wake >= deadline {
                        return result.failure();
                    }
                    tokio::time::sleep_until(wake).await;
                }
            }
        }
        Response::Failed(Failure::ServerError)
    }

    async fn exchange(&self, body: &str, signature: &str) -> Attempt {
        let Ok(response) = self
            .client
            .inner
            .post(self.url.clone())
            .header("Content-Type", "application/json")
            .header("Sipx-Signature", signature)
            .body(body.to_owned())
            .send()
            .await
        else {
            return Attempt::Unreachable;
        };
        let status = response.status();
        if status.is_client_error() {
            return Attempt::ClientError;
        }
        if status.is_server_error() {
            return Attempt::ServerError;
        }
        if !status.is_success() {
            // Includes 3xx. Redirect following is disabled above; an unclassified status is a
            // binding failure, not an instruction the adapter gets to interpret.
            return Attempt::BindingError;
        }
        read_body(response).await
    }
}

#[derive(Debug)]
enum Attempt {
    Body(String),
    ClientError,
    ServerError,
    Unreachable,
    BindingError,
}

impl Attempt {
    fn failure(self) -> Delivery {
        match self {
            Self::Unreachable => Response::Failed(Failure::Unreachable),
            Self::ServerError | Self::BindingError => Response::Failed(Failure::ServerError),
            Self::ClientError => Response::Failed(Failure::ClientError),
            Self::Body(body) => Response::Body(body),
        }
    }
}

async fn read_body(mut response: reqwest::Response) -> Attempt {
    if response
        .content_length()
        .is_some_and(|length| length > MAX_RESPONSE_BYTES as u64)
    {
        return Attempt::BindingError;
    }
    let mut bytes = Vec::new();
    loop {
        match response.chunk().await {
            Ok(Some(chunk)) => {
                if bytes.len().saturating_add(chunk.len()) > MAX_RESPONSE_BYTES {
                    return Attempt::BindingError;
                }
                bytes.extend_from_slice(&chunk);
            }
            Ok(None) => break,
            Err(_) => return Attempt::ServerError,
        }
    }
    String::from_utf8(bytes).map_or(Attempt::BindingError, Attempt::Body)
}

/// Build the contract's HMAC-SHA-256 signature header for one logical delivery.
#[must_use]
pub fn signature(unix_seconds: i64, body: &[u8], secrets: &[Vec<u8>]) -> String {
    let mut header = format!("t={unix_seconds}");
    let timestamp = unix_seconds.to_string();
    for secret in secrets {
        let Ok(mut mac) = Hmac::<Sha256>::new_from_slice(secret) else {
            continue;
        };
        mac.update(timestamp.as_bytes());
        mac.update(b".");
        mac.update(body);
        let bytes = mac.finalize().into_bytes();
        header.push_str(", v1=");
        for byte in bytes {
            // Writing to a `String` cannot fail.
            let _ = write!(header, "{byte:02x}");
        }
    }
    header
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;
    use reqwest::StatusCode;

    #[test]
    fn declarations_are_checked_before_a_call() {
        assert!(matches!(
            Webhook::new("ftp://example.test", vec![vec![1]]),
            Err(WebhookError::Url)
        ));
        assert!(matches!(
            Webhook::new("http://", vec![vec![1]]),
            Err(WebhookError::Url)
        ));
        assert!(matches!(
            Webhook::new("https://:443/missing-host", vec![vec![1]]),
            Err(WebhookError::Url)
        ));
        assert!(matches!(
            Webhook::new("not a URL", vec![vec![1]]),
            Err(WebhookError::Url)
        ));
        assert!(matches!(
            Webhook::new("https://example.test", Vec::new()),
            Err(WebhookError::Secret)
        ));
    }

    #[test]
    fn the_response_bound_is_one_mebibyte() {
        assert_eq!(MAX_RESPONSE_BYTES, 1_048_576);
    }

    #[test]
    fn bindings_built_for_two_apps_share_the_host_clients_pool() {
        let client = WebhookClient::new().expect("client");
        let first = Webhook::with_client(&client, "https://one.example.test/hook", vec![vec![1]])
            .expect("first binding");
        let second = Webhook::with_client(&client, "https://two.example.test/hook", vec![vec![2]])
            .expect("second binding");

        assert!(first.client.shares_pool_with(&second.client));
    }

    #[test]
    fn statuses_keep_the_contracts_four_failure_classes() {
        assert!(StatusCode::BAD_REQUEST.is_client_error());
        assert!(StatusCode::SERVICE_UNAVAILABLE.is_server_error());
        assert!(StatusCode::FOUND.is_redirection());
    }
}
