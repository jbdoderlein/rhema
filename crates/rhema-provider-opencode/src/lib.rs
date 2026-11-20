//! `rhema-provider-opencode`, the OpenCode Console (pay-as-you-go)
//! and Go adapter for rhema-core's [`ModelProvider`] seam.
//!
//! Console serves every paid and Go text model through an
//! OpenAI-compatible chat-completions API, translated to the model's
//! native API server-side. This adapter speaks that route directly: one
//! `POST {base}/chat/completions` with `stream: true` per exchange, the
//! API key in the `Authorization: Bearer` header, and the SSE stream
//! decoded by the shared OpenAI-compatible core. `GET {base}/models`
//! lists the plan's catalog with context windows, modalities, and the
//! per-MTok prices Console publishes.
//!
//! Two base URLs matter:
//! - [`DEFAULT_BASE_URL`]: pay-as-you-go from the Console balance.
//! - [`GO_BASE_URL`]: the Go, Go Plus, and Go Teams subscription.
//!
//! Paste either into the login dialog; the stored instance keeps it.
//!
//! Every request also carries the stable conversation id Go requires in
//! [`SESSION_HEADER`], plus a client-identifying user agent instead of
//! the HTTP library's default. Console uses both for routing and prompt
//! caching. The header follows the request's own conversation id
//! ([`ChatParams::session_id`]) when one rides, the durable identity a
//! frontend stamps per conversation, so a resumed conversation keeps
//! its bucket across a restart; [`with_session_id`] seeds the id a
//! request naming none carries, and keeps it across provider rebuilds.
//!
//! [`with_session_id`]: OpenCodeProvider::with_session_id
//! [`ChatParams::session_id`]: rhema_core::request::ChatParams::session_id

#![forbid(unsafe_code)]

pub mod wire;

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use rhema_core::event::EventSink;
use rhema_core::output::ModelOutput;
use rhema_core::provider::{ModelProvider, ProviderError};
use rhema_core::request::ModelRequest;
use rhema_provider::{ModelCard, ModelListing};
use rhema_provider_openai_compatible::provider::{
    Api, OpenAiCompatibleProvider, network_error as core_network_error,
};
use rhema_provider_openai_compatible::wire::WireErrorBody;

use crate::wire::{WireModelsPage, is_free, is_text};

/// The pay-as-you-go Console base, versioned path included.
pub const DEFAULT_BASE_URL: &str = "https://opencode.ai/inference/openai/v1";

/// The Go/Go Plus/Go Teams base, versioned path included. Go requires a
/// stable session id on every request.
pub const GO_BASE_URL: &str = "https://opencode.ai/inference/go/openai/v1";

/// The header Console reads a conversation's stable id from.
pub const SESSION_HEADER: &str = "x-opencode-session";

/// The client identification Console asks harnesses to send instead of
/// a generic SDK or HTTP-library name.
pub const USER_AGENT: &str = concat!("rhema/", env!("CARGO_PKG_VERSION"));

/// The HTTP client every instance uses: bounded connects and name
/// lookups, stream reads stall-guarded, never total-time capped, and
/// named as rhema.
pub fn http_client() -> reqwest::Client {
    rhema_provider::http::client_builder()
        .user_agent(USER_AGENT)
        .build()
        .expect("the shared client builder is valid")
}

/// OpenCode Console or Go, speaking the shared OpenAI-compatible
/// chat-completions core.
pub struct OpenCodeProvider {
    base_url: String,
    api_key: String,
    /// The conversation id every request carries.
    session_id: String,
    http: reqwest::Client,
    /// Which request shape completions speak; the default is chat
    /// completions, and `/responses` switches the gateway's Responses
    /// route on.
    api: Api,
    /// The dialect-core transport carrying the session header; every
    /// completion goes through it. Behind a lock so a completion can
    /// re-aim the header at the request's conversation: the lock
    /// covers only the swap, the generation runs on an owned clone,
    /// so concurrent completions never serialize.
    transport: std::sync::Mutex<AimedTransport>,
}

/// The transport aimed at one conversation: the id its session header
/// carries and the transport carrying it, swapped together.
struct AimedTransport {
    provider: std::sync::Arc<OpenAiCompatibleProvider>,
    session_id: String,
}

/// Builds the transport aimed at `session_id`.
fn aimed_transport(base_url: &str, api_key: &str, session_id: &str, api: Api) -> AimedTransport {
    AimedTransport {
        provider: std::sync::Arc::new(transport_for(base_url, api_key, session_id, api)),
        session_id: session_id.to_owned(),
    }
}

impl std::fmt::Debug for OpenCodeProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The key never prints; the plain data does.
        f.debug_struct("OpenCodeProvider")
            .field("base_url", &self.base_url)
            .field("api_key", &"<redacted>")
            .field("session_id", &self.session_id)
            .field("http", &self.http)
            .finish()
    }
}

impl OpenCodeProvider {
    /// A provider authenticating with `api_key` against
    /// [`DEFAULT_BASE_URL`] (pay-as-you-go), carrying a fresh session
    /// id.
    ///
    /// An empty key is accepted but useless against Console: the header
    /// is omitted and the API answers 401. The model catalog works
    /// without a key.
    pub fn new(api_key: impl Into<String>) -> Self {
        let api_key = api_key.into();
        let session_id = new_session_id();
        Self {
            base_url: DEFAULT_BASE_URL.to_owned(),
            transport: std::sync::Mutex::new(aimed_transport(
                DEFAULT_BASE_URL,
                &api_key,
                &session_id,
                Api::Chat,
            )),
            api_key,
            session_id,
            http: http_client(),
            api: Api::Chat,
        }
    }

    /// A provider against [`GO_BASE_URL`], the subscription route.
    pub fn go(api_key: impl Into<String>) -> Self {
        Self::new(api_key).with_base_url(GO_BASE_URL)
    }

    /// Targets `base_url`, the full versioned base used verbatim with
    /// `/chat/completions` and `/models` appended; trailing slashes are
    /// trimmed. This is how a stored instance chooses pay-as-you-go or
    /// Go, and how tests reach a mock server.
    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = base_url.into().trim_end_matches('/').to_owned();
        self.transport = std::sync::Mutex::new(aimed_transport(
            &self.base_url,
            &self.api_key,
            &self.session_id,
            self.api,
        ));
        self
    }

    /// Speaks the gateway's Responses route (`POST {base}/responses`)
    /// instead of chat completions. A tool result's images ride its
    /// `function_call_output` as content parts there, so `see`'s staged
    /// images reach the model with no follow-up turn. The choice lives
    /// here, not on the transport, so the transport rebuilds of
    /// [`Self::with_base_url`] and [`Self::with_session_id`] keep it.
    pub fn with_api(mut self, api: Api) -> Self {
        self.api = api;
        self.transport = std::sync::Mutex::new(aimed_transport(
            &self.base_url,
            &self.api_key,
            &self.session_id,
            self.api,
        ));
        self
    }

    /// Keeps this conversation's id stable across provider rebuilds and
    /// resumes. A blank id is ignored: an empty header is worse than a
    /// fresh one. A value no HTTP header can carry is ignored the same
    /// way: Go requires the id on every request and [`Self::session_id`]
    /// reports the id actually carried, so a control character in a
    /// hand-edited profile keeps the current id instead of panicking
    /// construction.
    pub fn with_session_id(mut self, session_id: impl Into<String>) -> Self {
        let session_id = session_id.into();
        // The id arrives from a stored profile the way the key arrives
        // from config: runtime input. An id reqwest would reject keeps
        // the current one; dropping the header instead would silently
        // cost Go its routing and caching.
        if !session_id.trim().is_empty()
            && reqwest::header::HeaderValue::from_str(&session_id).is_ok()
        {
            self.session_id = session_id.clone();
            self.transport = std::sync::Mutex::new(aimed_transport(
                &self.base_url,
                &self.api_key,
                &session_id,
                self.api,
            ));
        }
        self
    }

    /// The conversation id every request carries.
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// The catalog rows this plan can serve through chat completions.
    /// Console's free-tier models are only available inside OpenCode
    /// itself, so they are dropped from the pay-as-you-go listing; Go
    /// includes its own free models and keeps them.
    async fn fetch_models(&self) -> Result<Vec<ModelCard>, ProviderError> {
        let response = self
            .http
            .get(format!("{}/models", self.base_url))
            .headers(self.headers())
            .timeout(Duration::from_secs(20))
            .send()
            .await
            .map_err(network_error)?;
        let status = response.status();
        if !status.is_success() {
            let text = response.text().await.unwrap_or_default();
            return Err(api_error(status, &text));
        }
        let page: WireModelsPage = response.json().await.map_err(|err| ProviderError::Api {
            message: format!("malformed models page: {err}"),
            status: None,
        })?;
        let go = self.base_url.contains("/inference/go/");
        Ok(page
            .data
            .into_iter()
            .filter(is_text)
            .map(ModelCard::from)
            .filter(|model| go || !is_free(model))
            .collect())
    }

    /// The headers every request carries: the bearer key when one is
    /// configured, the conversation id, and the client name.
    fn headers(&self) -> reqwest::header::HeaderMap {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(SESSION_HEADER, header_value(&self.session_id));
        headers.insert(reqwest::header::USER_AGENT, header_value(USER_AGENT));
        if !self.api_key.is_empty() {
            // The key comes from config or the environment: runtime
            // input, unlike the constants around it. A control
            // character pasted into it must skip the header, not
            // panic the constructor.
            let value = format!("Bearer {}", self.api_key);
            if let Ok(parsed) = value.parse() {
                headers.insert(reqwest::header::AUTHORIZATION, parsed);
            }
        }
        headers
    }
}

/// A header value from the adapter's own constants, or a session id
/// [`OpenCodeProvider::with_session_id`] already validated as one; a
/// malformed one is a programmer error caught in tests.
fn header_value(value: &str) -> reqwest::header::HeaderValue {
    value
        .parse()
        .expect("adapter headers are valid header values")
}

/// The dialect-core transport for one base: the session header, the
/// client name, the bearer key when present.
fn transport_for(
    base_url: &str,
    api_key: &str,
    session_id: &str,
    api: Api,
) -> OpenAiCompatibleProvider {
    let mut transport = OpenAiCompatibleProvider::new(api_key)
        .with_base_url(base_url)
        .with_api(api);
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(SESSION_HEADER, header_value(session_id));
    headers.insert(reqwest::header::USER_AGENT, header_value(USER_AGENT));
    transport = transport.with_extra_headers(headers);
    transport
}

/// Generates one process-unique conversation id in Console's `ses_…`
/// spelling. Wall-clock nanos make it unique across restarts in
/// practice; the counter separates providers created in the same tick.
/// Frontends keeping an id across provider rebuilds mint it here and
/// hand it back through [`OpenCodeProvider::with_session_id`].
pub fn new_session_id() -> String {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or_default();
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    format!("ses_{nanos:032x}{seq:04x}")
}

/// Maps a non-2xx body onto [`ProviderError::Api`], keeping the
/// OpenAI-shaped `error.message` when the body has it, and carrying the
/// HTTP status structurally for [`ProviderError::is_retryable`].
pub fn api_error(status: reqwest::StatusCode, raw: &str) -> ProviderError {
    let parsed: WireErrorBody = serde_json::from_str(raw).unwrap_or_default();
    ProviderError::Api {
        message: format!(
            "opencode api error (HTTP {}): {}",
            status.as_u16(),
            parsed.message(raw)
        ),
        status: Some(status.as_u16()),
    }
}

/// Maps a transport failure onto [`ProviderError`]; reqwest errors cannot
/// be carried in [`ProviderError`] directly (it is `Clone + Eq`).
pub fn network_error(err: reqwest::Error) -> ProviderError {
    core_network_error(err)
}

#[async_trait]
impl ModelListing for OpenCodeProvider {
    /// The plan's text chat models (`GET {base}/models`), normalized to
    /// [`ModelCard`] rows with their published context windows,
    /// modalities, and per-MTok prices.
    async fn list_models(&self) -> Result<Vec<ModelCard>, ProviderError> {
        self.fetch_models().await
    }
}

#[async_trait]
impl ModelProvider for OpenCodeProvider {
    /// # Errors
    ///
    /// Fails with [`ProviderError::Network`]/[`ProviderError::Timeout`]
    /// when the transport fails, [`ProviderError::Api`] with the HTTP
    /// status on a non-2xx response, and [`ProviderError::Api`] without
    /// one on a malformed stream chunk or a mid-stream error chunk.
    async fn complete(
        &self,
        req: ModelRequest,
        sink: &mut dyn EventSink,
    ) -> Result<ModelOutput, ProviderError> {
        // The gateway keys its routing and its cache bucket on the
        // session header. The request's conversation id (rhema-code
        // stamps the checkpoint stem, stable across a restart) is the
        // durable identity, so it aims the header and a resumed
        // conversation keeps its bucket; a request naming none keeps
        // the constructed id. The lock covers only the swap: the
        // generation runs on an owned clone.
        let wanted = req
            .params
            .session_id
            .as_deref()
            .filter(|id| {
                !id.trim().is_empty() && reqwest::header::HeaderValue::from_str(id).is_ok()
            })
            .unwrap_or(&self.session_id);
        let transport = {
            let mut aimed = self
                .transport
                .lock()
                .expect("the transport lock is not poisoned");
            if aimed.session_id != wanted {
                *aimed = aimed_transport(&self.base_url, &self.api_key, wanted, self.api);
            }
            std::sync::Arc::clone(&aimed.provider)
        };
        transport.complete(req, sink).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_prints_presence_never_the_key() {
        let provider = OpenCodeProvider::new("oc_sk_live_secret");
        let printed = format!("{provider:?}");
        assert!(!printed.contains("oc_sk_live_secret"), "{printed}");
        assert!(printed.contains("<redacted>"), "{printed}");
        assert!(printed.contains("base_url"), "{printed}");
        assert!(printed.contains(&provider.session_id), "{printed}");
    }

    #[test]
    fn session_ids_are_unique_and_spelled_for_console() {
        let first = OpenCodeProvider::new("k").session_id().to_owned();
        let second = OpenCodeProvider::new("k").session_id().to_owned();
        assert!(first.starts_with("ses_"), "{first}");
        assert_ne!(first, second);
    }

    #[test]
    fn the_responses_choice_survives_the_transport_rebuilds() {
        let provider = OpenCodeProvider::new("k")
            .with_api(Api::Responses)
            .with_base_url(GO_BASE_URL)
            .with_session_id("ses_kept");
        assert_eq!(provider.api, Api::Responses);
        assert_eq!(provider.base_url, GO_BASE_URL);
        // Chat stays the default, before and after a rebuild.
        let provider = OpenCodeProvider::new("k").with_base_url(GO_BASE_URL);
        assert_eq!(provider.api, Api::Chat);
    }

    #[test]
    fn a_chosen_session_id_survives_a_base_url_change() {
        let provider = OpenCodeProvider::new("k")
            .with_session_id("ses_kept")
            .with_base_url(GO_BASE_URL);
        assert_eq!(provider.session_id(), "ses_kept");
        assert_eq!(provider.base_url, GO_BASE_URL);
        // A blank override never blanks the header.
        let provider = provider.with_session_id("   ");
        assert_eq!(provider.session_id(), "ses_kept");
    }

    #[test]
    fn a_session_id_no_header_can_carry_keeps_the_current_one() {
        // A control character in a stored profile must not panic
        // construction: the unusable override is ignored like a blank
        // one, and the fresh id from construction is what the header
        // carries.
        let provider = OpenCodeProvider::new("k");
        let fresh = provider.session_id().to_owned();
        let provider = provider.with_session_id("bad\u{1f}id");
        assert_eq!(provider.session_id(), fresh);
        assert_eq!(provider.headers()[SESSION_HEADER], fresh.as_str());
        // A usable override still lands after a rejected one.
        let provider = provider.with_session_id("ses_kept");
        assert_eq!(provider.session_id(), "ses_kept");
    }

    #[test]
    fn a_key_with_a_control_character_skips_the_header() {
        // A newline pasted into the key must not panic the
        // constructor: the header is dropped, the constants around it
        // stay.
        let provider = OpenCodeProvider::new("oc_sk\nbroken");
        let headers = provider.headers();
        assert!(!headers.contains_key(reqwest::header::AUTHORIZATION));
        assert!(headers.contains_key(SESSION_HEADER));
        assert!(headers.contains_key(reqwest::header::USER_AGENT));
    }

    #[test]
    fn go_starts_at_the_go_base() {
        assert_eq!(OpenCodeProvider::go("k").base_url, GO_BASE_URL);
    }
}
