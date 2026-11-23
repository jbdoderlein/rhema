//! The failure classes an adapter reports.

/// Why one call failed. [`Invalid`](Self::Invalid) is the request's
/// fault, the seam's own validation or the vendor's refusal of it; the
/// rest are the vendor's, and whether to retry is the caller's call,
/// not the model's.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
#[non_exhaustive]
pub enum DecisionError {
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
    /// The request itself is wrong: the seam's validation, run before
    /// any vendor is asked, or the vendor's 400 or 422, a question it
    /// cannot score, an option count out of range. The fix is in the
    /// request; the same bytes fail the same way again.
    #[error("invalid request: {0}")]
    Invalid(String),
    /// The call's budget blew before the vendor answered.
    #[error("timed out")]
    Timeout,
    /// The request never reached a status: DNS, connect, reset.
    #[error("transport: {0}")]
    Transport(String),
    /// The response was not the documented shape.
    #[error("unexpected response: {0}")]
    Parse(String),
}

impl DecisionError {
    /// Maps a vendor status to its class, the one mapping every
    /// adapter repeats: 400 and 422 are [`Invalid`] with the body's
    /// detail extracted ([`error_detail`]), 401 and 403 `Auth`, 429
    /// `RateLimited`, anything else `Http` with the same detail (a
    /// one-line body head when the body is not the vendor's JSON).
    pub fn from_status(status: u16, body: &str) -> Self {
        match status {
            400 | 422 => Self::Invalid(error_detail(body)),
            401 | 403 => Self::Auth,
            429 => Self::RateLimited,
            // The extracted detail when the body is the vendor's JSON
            // (Workers AI's `errors[].message`), the head otherwise.
            _ => Self::Http {
                status,
                body: error_detail(body),
            },
        }
    }
}

/// The body's first line, trimmed and capped, for the `Http` class's
/// context.
fn body_head(body: &str) -> String {
    body.lines()
        .next()
        .unwrap_or("")
        .trim()
        .chars()
        .take(200)
        .collect()
}

/// The reason a vendor gives for refusing a request, out of the
/// error body's JSON: Jev's `detail` (a string, or a validation
/// error's `msg` entries), Cloudflare's `errors[].message`,
/// OpenRouter's `error.message`. A body that is none of these is its
/// own one-line head.
pub fn error_detail(body: &str) -> String {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(body) else {
        return body_head(body);
    };
    if let Some(detail) = value.get("detail") {
        return detail_text(detail);
    }
    if let Some(errors) = value.get("errors").and_then(|e| e.as_array()) {
        let messages: Vec<String> = errors
            .iter()
            .filter_map(|error| {
                error
                    .get("message")
                    .and_then(|m| m.as_str())
                    .map(str::to_owned)
            })
            .collect();
        if !messages.is_empty() {
            return messages.join("; ");
        }
    }
    if let Some(error) = value.get("error") {
        if let Some(text) = error.as_str() {
            return text.chars().take(400).collect();
        }
        return detail_text(error);
    }
    body_head(body)
}

/// One `detail` value to text: a string as is, a validation error's
/// `msg` field, OpenRouter's `message` one, an array joined, anything
/// else rendered compact.
fn detail_text(detail: &serde_json::Value) -> String {
    match detail {
        serde_json::Value::String(text) => text.chars().take(400).collect(),
        serde_json::Value::Array(items) => {
            items.iter().map(detail_text).collect::<Vec<_>>().join("; ")
        }
        serde_json::Value::Object(_) => detail
            .get("msg")
            .or_else(|| detail.get("message"))
            .and_then(|msg| msg.as_str())
            .map(str::to_owned)
            .unwrap_or_else(|| detail.to_string()),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn statuses_map_to_their_classes() {
        assert_eq!(DecisionError::from_status(401, "x"), DecisionError::Auth);
        assert_eq!(DecisionError::from_status(403, "x"), DecisionError::Auth);
        assert_eq!(
            DecisionError::from_status(429, "slow down"),
            DecisionError::RateLimited
        );
        assert_eq!(
            DecisionError::from_status(500, "boom"),
            DecisionError::Http {
                status: 500,
                body: "boom".into()
            }
        );
    }

    #[test]
    fn a_bad_request_is_invalid_with_its_detail() {
        let err = DecisionError::from_status(400, r#"{"detail": "questions: field required"}"#);
        assert_eq!(
            err,
            DecisionError::Invalid("questions: field required".into())
        );
        assert!(matches!(err, DecisionError::Invalid(_)));
        // Jev's validation shape: a list of located errors.
        let err = DecisionError::from_status(
            422,
            r#"{"detail": [{"loc": ["body", "state"], "msg": "Field required", "type": "missing"}]}"#,
        );
        assert_eq!(err, DecisionError::Invalid("Field required".into()));
        // Cloudflare's envelope: errors with codes and messages.
        let err = DecisionError::from_status(
            400,
            r#"{"success": false, "errors": [{"code": 1001, "message": "questions.0: invalid"}]}"#,
        );
        assert_eq!(err, DecisionError::Invalid("questions.0: invalid".into()));
        // OpenRouter's envelope: the message under `error`.
        let err = DecisionError::from_status(
            400,
            r#"{"error": {"code": 400, "message": "Invalid request parameters"}}"#,
        );
        assert_eq!(
            err,
            DecisionError::Invalid("Invalid request parameters".into())
        );
        // Not JSON: the head of the body.
        let err = DecisionError::from_status(400, "  <html>nope</html>\nmore");
        assert_eq!(err, DecisionError::Invalid("<html>nope</html>".into()));
    }

    #[test]
    fn a_body_head_becomes_one_short_line() {
        let err = DecisionError::from_status(502, "  Bad gateway\n\ngateway details\nmore");
        let DecisionError::Http { body, .. } = err else {
            panic!("expected Http, got {err:?}");
        };
        assert_eq!(body, "Bad gateway");
    }

    #[test]
    fn the_errors_are_std_and_display_and_debug_safe() {
        fn assert_error<T: std::error::Error + std::fmt::Debug>() {}
        assert_error::<DecisionError>();
    }
}
