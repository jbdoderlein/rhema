//! The response: the answers, the model that gave them, the usage.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::answer::Answer;

/// The token counts a decision costs: input only in practice, the
/// vendors charge nothing for output, but the field is the wire's.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct Usage {
    /// The tokens the state and the questions took.
    #[serde(default)]
    pub input_tokens: u64,
    /// The tokens the answers took, zero for every current vendor.
    #[serde(default)]
    pub output_tokens: u64,
    /// What the call was charged, in the vendor's credits, when it
    /// says: OpenRouter reports it, Jev and Clef do not. Absent on
    /// their wire and on this one when unset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost: Option<f64>,
}

/// One decision response: the answers keyed by the request's
/// question names, the model that answered (a concrete version when
/// the request named an alias), and the usage.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DecisionResponse {
    /// The model that answered, the vendor's concrete name.
    pub model: String,
    /// One answer per question, under the request's names.
    pub answers: BTreeMap<String, Answer>,
    /// The token counts, zero when the vendor sent none.
    #[serde(default)]
    pub usage: Usage,
}

impl DecisionResponse {
    /// A response from its model and answers, no usage.
    pub fn new(model: impl Into<String>, answers: BTreeMap<String, Answer>) -> Self {
        Self {
            model: model.into(),
            answers,
            usage: Usage::default(),
        }
    }

    /// Parses a response out of a JSON value, the shape both vendors
    /// answer with once the adapter has unwrapped its envelope.
    ///
    /// # Errors
    ///
    /// The value is not the response shape.
    pub fn from_value(value: serde_json::Value) -> Result<Self, serde_json::Error> {
        serde_json::from_value(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_documented_response_parses_with_its_usage() {
        let response = DecisionResponse::from_value(json!({
            "model": "jev-1.13.0",
            "answers": {
                "department": {
                    "type": "choice", "choice": "returns", "confidence": 1.0,
                    "probabilities": {"shipping": 0.0, "returns": 1.0, "billing": 0.0}
                }
            },
            "usage": {"input_tokens": 328, "output_tokens": 34}
        }))
        .unwrap();
        assert_eq!(response.model, "jev-1.13.0");
        assert_eq!(response.usage.input_tokens, 328);
        assert_eq!(response.answers.len(), 1);
    }

    #[test]
    fn a_response_without_usage_reads_zero() {
        let response = DecisionResponse::from_value(
            json!({"model": "clef", "answers": {"q": {"type": "noul", "noul": 0.5}}}),
        )
        .unwrap();
        assert_eq!(response.usage, Usage::default());
    }

    #[test]
    fn a_response_without_answers_or_model_is_refused() {
        assert!(DecisionResponse::from_value(json!({"model": "m"})).is_err());
        assert!(DecisionResponse::from_value(json!({"answers": {}})).is_err());
    }
}
