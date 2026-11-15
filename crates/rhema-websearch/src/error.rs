//! The failure classes an adapter reports, and the aggregate the
//! chain returns when every provider failed.

/// Why one provider's attempt failed. The classes are the chain's
/// fallback input: every one of them falls through, the vendor and
/// the reason ride the display string the aggregate renders, never a
/// control decision (see `specs/rhema-websearch.md` §5).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SearchError {
    /// The key or plan was refused: 401 or 403 from the vendor.
    #[error("the key or plan was refused")]
    Auth,
    /// The vendor throttled the call: 429.
    #[error("rate limited")]
    RateLimited,
    /// Any other non-2xx status, with the body's head for context.
    #[error("HTTP {status}: {body}")]
    Http {
        /// The status code the vendor returned.
        status: u16,
        /// The first bytes of the body, whitespace-trimmed, newlines
        /// dropped: context, never a whole error page.
        body: String,
    },
    /// The attempt's budget blew before the vendor answered.
    #[error("timed out")]
    Timeout,
    /// The request never reached a status: DNS, connect, reset.
    #[error("transport: {0}")]
    Transport(String),
    /// The response was not the documented shape.
    #[error("unexpected response: {0}")]
    Parse(String),
}

impl SearchError {
    /// Maps a vendor status to its class, the one mapping every
    /// adapter repeats. `body_head` is the response body's head; the
    /// helper trims it to one line so an HTML error page stays one
    /// line.
    pub fn from_status(status: u16, body_head: &str) -> Self {
        let one_line: String = body_head
            .lines()
            .next()
            .unwrap_or("")
            .trim()
            .chars()
            .take(200)
            .collect();
        match status {
            401 | 403 => Self::Auth,
            429 => Self::RateLimited,
            _ => Self::Http {
                status,
                body: one_line,
            },
        }
    }
}

/// Why the whole chain failed: every provider's name and reason, in
/// attempt order. One error for the whole run, because the model
/// reading a failed `search` command needs the ladder's reasons in
/// one place, not a choice between them.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("every provider failed: {}", Self::summarize(.attempts))]
pub struct ChainError {
    /// One `(instance name, reason)` pair per attempt, in order.
    pub attempts: Vec<(String, SearchError)>,
}

impl ChainError {
    /// The aggregate for `attempts`, empty attempts included (a chain
    /// with no providers fails before any attempt runs).
    pub fn new(attempts: Vec<(String, SearchError)>) -> Self {
        Self { attempts }
    }

    /// The display's rendering of the attempts: one line each.
    fn summarize(attempts: &[(String, SearchError)]) -> String {
        if attempts.is_empty() {
            return "no provider was configured".to_owned();
        }
        attempts
            .iter()
            .map(|(name, err)| format!("{name}: {err}"))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn statuses_map_to_their_classes() {
        assert_eq!(SearchError::from_status(401, "x"), SearchError::Auth);
        assert_eq!(SearchError::from_status(403, "x"), SearchError::Auth);
        assert_eq!(
            SearchError::from_status(429, "slow down"),
            SearchError::RateLimited
        );
        assert_eq!(
            SearchError::from_status(500, "boom"),
            SearchError::Http {
                status: 500,
                body: "boom".into()
            }
        );
    }

    #[test]
    fn a_body_head_becomes_one_short_line() {
        let err = SearchError::from_status(502, "  Bad gateway\n\ngateway details\nmore");
        let SearchError::Http { body, .. } = err else {
            panic!("expected Http, got {err:?}");
        };
        assert_eq!(body, "Bad gateway");
    }

    #[test]
    fn the_aggregate_names_every_attempt_in_order() {
        let err = ChainError::new(vec![
            ("brave-main".into(), SearchError::RateLimited),
            ("exa-work".into(), SearchError::Timeout),
        ]);
        assert_eq!(
            err.to_string(),
            "every provider failed: brave-main: rate limited\nexa-work: timed out"
        );
    }

    #[test]
    fn an_empty_chain_says_so() {
        let err = ChainError::new(Vec::new());
        assert_eq!(
            err.to_string(),
            "every provider failed: no provider was configured"
        );
    }

    #[test]
    fn the_error_is_std_and_display_and_debug_safe() {
        fn assert_error<T: std::error::Error + std::fmt::Debug>() {}
        assert_error::<ChainError>();
        assert_error::<SearchError>();
    }
}
