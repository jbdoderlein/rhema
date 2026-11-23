//! The OpenRouter Decisions wire: the body the adapter sends and the
//! response it reads, pure functions over JSON values.

use serde_json::{Value, json};

use rhema_decision::{DecisionError, DecisionRequest, DecisionResponse};

/// The model asked when the instance names none: TypeSafe's Jev 1.14
/// through the router, the reference System One model and the
/// cheaper per input token of the two open ones (the catalog prices
/// `cloudflare/clef` at four times its rate).
pub const DEFAULT_MODEL: &str = "typesafe/jev-1.14";

/// The body: the seam's request under `state` and `questions`, the
/// slug as `model`. Jev's own body with another spelling in the
/// model field, which is what the router made of the System One
/// wire (`specs/rhema-decision.md` §4).
pub fn request_body(request: &DecisionRequest, model: &str) -> Value {
    json!({
        "model": model,
        "state": request.state,
        "questions": request.questions,
    })
}

/// The response off the wire: the seam's shape, plus `id`,
/// `provider` and a `cost` in the usage the seam does not model and
/// serde passes over. A score carries no `legend` here (Jev echoes
/// the rubric back, the router does not), so the seam's legend
/// defaults empty.
///
/// # Errors
///
/// [`DecisionError::Parse`] when the body is not that shape.
pub fn response_from_json(body: &str) -> Result<DecisionResponse, DecisionError> {
    let value: Value =
        serde_json::from_str(body).map_err(|err| DecisionError::Parse(err.to_string()))?;
    DecisionResponse::from_value(value).map_err(|err| DecisionError::Parse(err.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rhema_decision::{Answer, Question};
    use serde_json::json;

    #[test]
    fn the_body_carries_the_slug_then_the_seams_request() {
        let request = DecisionRequest::new("charged twice").with_question(
            "team",
            Question::choice("Which team", [("billing", json!("Payments"))]),
        );
        let body = request_body(&request, "cloudflare/clef-flash");
        assert_eq!(
            body,
            json!({
                "model": "cloudflare/clef-flash",
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
        // The response example of the Decisions API docs, verbatim.
        let body = r#"{
            "id": "gen-dec-1791209504-cXRBLHSCBmsjQX93jkWe",
            "model": "typesafe/jev-1.14",
            "provider": "TypeSafe",
            "answers": {
                "is_churn_risk": {"type": "noul", "noul": 0.0412, "confidence": 0.9588},
                "department": {
                    "type": "choice", "choice": "billing", "confidence": 0.9821,
                    "probabilities": {"billing": 0.9821, "technical_support": 0.0142, "sales": 0.0037}
                },
                "urgency": {
                    "type": "score", "score": 1.84, "confidence": 0.85,
                    "probabilities": {"0": 0.01, "1": 0.14, "2": 0.85}
                }
            },
            "usage": {"input_tokens": 128, "output_tokens": 0, "cost": 0.0000384,
                      "input_tokens_details": {"cached_tokens": 0}}
        }"#;
        let response = response_from_json(body).unwrap();
        assert_eq!(response.model, "typesafe/jev-1.14");
        assert_eq!(response.usage.input_tokens, 128);
        // The charge rides through the seam; the id and the provider
        // name do not.
        assert_eq!(response.usage.cost, Some(0.0000384));
        let Answer::Score { score, legend, .. } = &response.answers["urgency"] else {
            panic!("expected a score");
        };
        assert_eq!(*score, 1.84);
        // No rubric comes back: the legend is empty, not a parse failure.
        assert!(legend.is_empty());
    }

    #[test]
    fn a_refused_question_is_its_own_answer() {
        let body = r#"{
            "model": "cloudflare/clef",
            "answers": {
                "safe_question": {"type": "noul", "noul": 0.92, "confidence": 0.92},
                "flagged_question": {"type": "refusal"}
            },
            "usage": {"input_tokens": 10, "output_tokens": 0}
        }"#;
        let response = response_from_json(body).unwrap();
        assert_eq!(response.answers["flagged_question"], Answer::Refusal);
        assert_eq!(response.answers["flagged_question"].confidence(), None);
    }

    #[test]
    fn a_body_that_is_not_the_shape_is_a_parse_error() {
        let err = response_from_json(r#"{"error": {"code": 400, "message": "nope"}}"#).unwrap_err();
        assert!(matches!(err, DecisionError::Parse(_)), "{err:?}");
    }
}
