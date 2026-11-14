//! The web search fleet's name-to-adapter wiring.
//!
//! Every frontend spells the same recipes, the `brave` kind is the
//! Brave adapter with its `X-Subscription-Token` header, the `tavily`
//! kind is the Tavily adapter under bearer auth, and without this
//! crate each would spell them in its own assembly. This is the one
//! spelling: [`build`] takes the kind as the config spells it, plus
//! the key the caller resolved, and returns the provider as a trait
//! object. What stays with the caller is policy: the chain, the
//! order, the credential resolution itself, a registry call here is
//! wiring, not settings (`specs/rhema-websearch.md` §8).
#![forbid(unsafe_code)]

use std::sync::Arc;

use rhema_websearch::{MockSearchProvider, WebSearchProvider};
use rhema_websearch_brave::BraveProvider;
use rhema_websearch_exa::ExaProvider;
use rhema_websearch_tavily::TavilyProvider;

/// Why a construction failed: a spelling no adapter answers to, or a
/// kind whose key the caller never resolved.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BuildError {
    /// The kind names nothing in the fleet.
    #[error("unknown websearch provider kind \"{0}\"")]
    Unknown(String),
    /// Every kind is keyed and the caller passed no key.
    #[error("{0} needs an api key")]
    MissingKey(String),
}

/// The vendor spellings the config uses, in the env-only tier's fixed
/// order (`specs/rhema-websearch.md` §3).
pub const KINDS: [&str; 3] = ["brave", "exa", "tavily"];

/// Builds the kind's provider from the key the caller resolved. The
/// constructors are the adapters' defaults: no route overrides, no
/// vendor knobs, the settings have no fields for them in v1.
///
/// # Errors
///
/// [`BuildError::Unknown`] when `kind` names nothing in
/// [`KINDS`], [`BuildError::MissingKey`] when `api_key` is none:
/// every kind here is keyed.
pub fn build(
    kind: &str,
    api_key: Option<String>,
) -> Result<Arc<dyn WebSearchProvider>, BuildError> {
    if !KINDS.contains(&kind) {
        return Err(BuildError::Unknown(kind.to_owned()));
    }
    let Some(api_key) = api_key else {
        return Err(BuildError::MissingKey(kind.to_owned()));
    };
    match kind {
        "brave" => Ok(Arc::new(BraveProvider::new(api_key))),
        "exa" => Ok(Arc::new(ExaProvider::new(api_key))),
        // The spelling is validated above; the arm is exhaustive.
        _ => Ok(Arc::new(TavilyProvider::new(api_key))),
    }
}

/// Builds the test provider the registry's own consumers' offline
/// tests drive: the base crate's [`MockSearchProvider`] behind the
/// same trait object `build` returns, under any kind spelling.
/// Feature `mock`, a test seam: production never calls it.
#[cfg(feature = "mock")]
pub fn build_mock(kind: &'static str) -> Arc<dyn WebSearchProvider> {
    Arc::new(MockSearchProvider::new(kind))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_kind_builds_from_a_key() {
        for kind in KINDS {
            let provider = build(kind, Some("key".into())).unwrap();
            assert_eq!(provider.kind(), kind);
        }
    }

    #[test]
    fn a_missing_key_is_a_missing_key_not_an_unknown() {
        assert!(
            matches!(build("brave", None), Err(BuildError::MissingKey(kind)) if kind == "brave")
        );
    }

    #[test]
    fn an_unknown_spelling_names_itself() {
        assert!(
            matches!(build("kagi", Some("key".into())), Err(BuildError::Unknown(kind)) if kind == "kagi")
        );
        assert!(
            matches!(build("kagi", None), Err(BuildError::Unknown(kind)) if kind == "kagi"),
            "an unknown kind is unknown before it is keyed"
        );
    }
}
