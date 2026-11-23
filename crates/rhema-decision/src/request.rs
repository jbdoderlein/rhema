//! The request: a state, and the named questions asked about it.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::question::Question;

/// One decision request: the content every question refers to, and
/// the questions, each under a name the caller chooses and the
/// answers come back under. Serialized, this is the System One body
/// minus the model, which is the adapter's (Clef's model picks the
/// route, so a request cannot carry one for the adapter to honor).
///
/// A `model` key in hand-written JSON is read and dropped, so the
/// documented vendor examples paste into `decide` as they are.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DecisionRequest {
    /// The content to evaluate: a string, or any JSON value (records,
    /// chat logs, application state).
    pub state: Value,
    /// The questions, keyed by the name the answer comes back under.
    /// At least one; [`crate::decide`] refuses an empty map before any
    /// vendor is asked.
    pub questions: BTreeMap<String, Question>,
}

impl DecisionRequest {
    /// A request over `state` with no questions yet.
    pub fn new(state: impl Into<Value>) -> Self {
        Self {
            state: state.into(),
            questions: BTreeMap::new(),
        }
    }

    /// Adds one question under `name`, builder style.
    pub fn with_question(mut self, name: impl Into<String>, question: Question) -> Self {
        self.questions.insert(name.into(), question);
        self
    }

    /// Parses a hand-written JSON request. Unknown top-level keys
    /// (`model`, a vendor's extension) are ignored, so a vendor's
    /// documented example is a valid request here.
    ///
    /// # Errors
    ///
    /// The JSON does not parse, or does not match the request shape.
    pub fn from_json(text: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(text)
    }

    /// Whether the request is well formed by the rules both vendors
    /// share: at least one question, a non-blank name per question,
    /// and every question valid by
    /// [`Question::validate`]. [`crate::decide`] runs this before the
    /// vendor is asked, so a broken request costs no call.
    pub fn validate(&self) -> Result<(), String> {
        if self.questions.is_empty() {
            return Err("a request needs at least one question".to_owned());
        }
        for (name, question) in &self.questions {
            if name.trim().is_empty() {
                return Err("a question name is blank".to_owned());
            }
            question
                .validate()
                .map_err(|problem| format!("question {name:?}: {problem}"))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::question::NoulCriteria;
    use serde_json::json;

    #[test]
    fn the_request_serializes_to_state_and_questions() {
        let request = DecisionRequest::new("I was charged twice.")
            .with_question(
                "department",
                Question::choice(
                    "Which team",
                    [("billing", json!("Payments")), ("sales", json!("Plans"))],
                ),
            )
            .with_question("urgent", Question::noul("Urgent?"));
        assert_eq!(
            serde_json::to_value(&request).unwrap(),
            json!({
                "state": "I was charged twice.",
                "questions": {
                    "department": {
                        "type": "choice",
                        "instructions": "Which team",
                        "criteria": {"billing": "Payments", "sales": "Plans"}
                    },
                    "urgent": {"type": "noul", "instructions": "Urgent?"}
                }
            })
        );
    }

    #[test]
    fn a_vendors_documented_example_parses_with_its_model_dropped() {
        let text = r#"{
            "model": "jev-latest",
            "state": "Our API integration started returning 500 errors.",
            "questions": {
                "department": {
                    "type": "choice",
                    "instructions": "Which team should handle this",
                    "criteria": {"billing": "Payment", "technical": "Bugs"}
                },
                "frustration": {
                    "type": "score",
                    "instructions": "How frustrated",
                    "criteria": ["Calm", "Frustrated", "Angry"]
                },
                "is_urgent": {"type": "noul", "instructions": "Conveys urgency"}
            }
        }"#;
        let request = DecisionRequest::from_json(text).unwrap();
        assert_eq!(request.questions.len(), 3);
        assert!(matches!(
            request.questions["is_urgent"],
            Question::Noul { criteria: None, .. }
        ));
        assert!(matches!(
            request.questions["frustration"],
            Question::Score { ref criteria, .. } if criteria.len() == 3
        ));
        assert!(request.validate().is_ok());
    }

    #[test]
    fn noul_criteria_parse_back() {
        let text = r#"{"state": "s", "questions": {"q": {
            "type": "noul", "instructions": "x",
            "criteria": {"true": "means yes", "false": "means no"}}}}"#;
        let request = DecisionRequest::from_json(text).unwrap();
        let Question::Noul {
            criteria: Some(criteria),
            ..
        } = &request.questions["q"]
        else {
            panic!("expected a noul with criteria");
        };
        assert_eq!(
            criteria,
            &NoulCriteria {
                yes: Some(json!("means yes")),
                no: Some(json!("means no")),
            }
        );
    }

    #[test]
    fn a_malformed_request_is_an_error() {
        assert!(DecisionRequest::from_json("not json").is_err());
        assert!(
            DecisionRequest::from_json(r#"{"state": "s", "questions": {"q": {"type": "box"}}}"#)
                .is_err(),
            "a question kind this seam does not model does not parse"
        );
        assert!(DecisionRequest::from_json(r#"{"questions": {}}"#).is_err());
    }

    #[test]
    fn validation_names_the_question_and_the_rule() {
        let empty = DecisionRequest::new("s");
        assert_eq!(
            empty.validate().unwrap_err(),
            "a request needs at least one question"
        );
        let broken = DecisionRequest::new("s")
            .with_question("team", Question::choice("Which", [("only", json!(1))]));
        assert_eq!(
            broken.validate().unwrap_err(),
            "question \"team\": a choice needs at least 2 options, got 1"
        );
        let blank = DecisionRequest::new("s").with_question("  ", Question::noul("x"));
        assert_eq!(blank.validate().unwrap_err(), "a question name is blank");
    }

    #[test]
    fn a_structured_state_passes_through() {
        let request =
            DecisionRequest::new(json!({"invoice": {"total": 1250.0, "status": "overdue"}}))
                .with_question("large", Question::noul("Over 1000?"));
        let value = serde_json::to_value(&request).unwrap();
        assert_eq!(value["state"]["invoice"]["total"], json!(1250.0));
    }
}
