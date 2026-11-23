//! The request's shaping and the response's parsing. Always compiled:
//! a host without sockets maps its own HTTP through these.

use serde_json::{Value, json};

use rhema_decision::{DecisionError, DecisionRequest, DecisionResponse};

/// The model the provider asks for when the instance names none: the
/// moving alias, what the vendor's own examples send.
pub const DEFAULT_MODEL: &str = "jev-latest";

/// The request body: the seam's request plus the model field, in the
/// documented key order. The seam's own serialization is the wire
/// shape, so nothing is translated, only prefixed.
pub fn request_body(request: &DecisionRequest, model: &str) -> Value {
    json!({
        "model": model,
        "state": request.state,
        "questions": request.questions,
    })
}

/// Parses a response body. The vendor's shape is the seam's, so the
/// mapping is a parse; anything else is a `Parse` error carrying the
/// reason, which the chain falls through on.
pub fn response_from_json(body: &str) -> Result<DecisionResponse, DecisionError> {
    let value: Value =
        serde_json::from_str(body).map_err(|err| DecisionError::Parse(err.to_string()))?;
    DecisionResponse::from_value(value).map_err(|err| DecisionError::Parse(err.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rhema_decision::Question;
    use serde_json::json;

    #[test]
    fn the_body_carries_the_model_then_the_seams_request() {
        let request = DecisionRequest::new("charged twice").with_question(
            "team",
            Question::choice("Which team", [("billing", json!("Payments"))]),
        );
        let body = request_body(&request, "jev-preview");
        assert_eq!(
            body,
            json!({
                "model": "jev-preview",
                "state": "charged twice",
                "questions": {"team": {
                    "type": "choice",
                    "instructions": "Which team",
                    "criteria": {"billing": "Payments"}
                }}
            })
        );
    }

    #[test]
    fn the_documented_response_parses() {
        let body = r#"{
            "model": "jev-1.13.0",
            "answers": {
                "department": {"type": "choice", "choice": "returns", "confidence": 1.0,
                               "probabilities": {"shipping": 0.0, "returns": 1.0}},
                "is_urgent": {"type": "noul", "noul": 0.99}
            },
            "usage": {"input_tokens": 328, "output_tokens": 34}
        }"#;
        let response = response_from_json(body).unwrap();
        assert_eq!(response.model, "jev-1.13.0");
        assert_eq!(response.answers.len(), 2);
        assert_eq!(response.usage.input_tokens, 328);
    }

    #[test]
    fn a_body_that_is_not_the_shape_is_a_parse_error() {
        assert!(matches!(
            response_from_json("<html>nope</html>"),
            Err(DecisionError::Parse(_))
        ));
        // A box answer is a kind the seam carries as `Other`, so the
        // response parses; what does not is a body missing its parts.
        assert!(
            response_from_json(
                r#"{"model": "jev-1.13.0", "answers": {"q": {"type": "box", "box": {}}}}"#
            )
            .is_ok()
        );
        assert!(matches!(
            response_from_json(r#"{"model": "jev-1.13.0"}"#),
            Err(DecisionError::Parse(_))
        ));
    }
}
