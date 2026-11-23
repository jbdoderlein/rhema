//! The three question kinds a decision model answers, the System One
//! vocabulary both vendors share.
//!
//! A question is a JSON object tagged by `type`: `noul` asks whether a
//! statement holds and answers with the probability that it does;
//! `choice` picks one option out of a set the caller names and
//! describes; `score` places the state on an ordered rubric the caller
//! writes, lowest level first. `instructions` and every criteria
//! description accept a string or any JSON structure: the vendors
//! read a structure as labeled data, so a question built by code can
//! carry the record it compares the state against.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The fewest options a `choice` question carries. One option is a
/// question with one answer, which both vendors refuse.
pub const MIN_OPTIONS: usize = 2;
/// The most options a `choice` question carries: the shared ceiling,
/// Jev's and Clef's.
pub const MAX_OPTIONS: usize = 255;
/// The fewest levels a `score` rubric carries: a scale needs two
/// points.
pub const MIN_LEVELS: usize = 2;
/// The most levels a `score` rubric carries: the shared ceiling.
pub const MAX_LEVELS: usize = 10;

/// What a yes and a no mean for a `noul` question, optional
/// descriptions the model reads alongside the instructions.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct NoulCriteria {
    /// What a yes (a probability near 1) means.
    #[serde(rename = "true", default, skip_serializing_if = "Option::is_none")]
    pub yes: Option<Value>,
    /// What a no (a probability near 0) means.
    #[serde(rename = "false", default, skip_serializing_if = "Option::is_none")]
    pub no: Option<Value>,
}

/// One typed question, serialized as the `{"type": ..., ...}` object
/// the System One wire carries.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Question {
    /// The probability that the statement in `instructions` holds.
    Noul {
        /// The yes/no question, or the statement to judge.
        instructions: Value,
        /// Optional descriptions of what a yes and a no mean.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        criteria: Option<NoulCriteria>,
    },
    /// One option out of `criteria`, an id to its description.
    Choice {
        /// What the model should decide.
        instructions: Value,
        /// The options: a non-empty id to its description, which may
        /// be a string, a structure, or null when the id says it all.
        criteria: BTreeMap<String, Value>,
    },
    /// A position on the ordered rubric `criteria`, level 0 first.
    Score {
        /// What the model should rate.
        instructions: Value,
        /// The levels, lowest first, each a description.
        criteria: Vec<Value>,
    },
}

impl Question {
    /// A yes/no question over `instructions`.
    pub fn noul(instructions: impl Into<Value>) -> Self {
        Self::Noul {
            instructions: instructions.into(),
            criteria: None,
        }
    }

    /// A choice over `options`, `(id, description)` pairs.
    pub fn choice<I, S>(instructions: impl Into<Value>, options: I) -> Self
    where
        I: IntoIterator<Item = (S, Value)>,
        S: Into<String>,
    {
        Self::Choice {
            instructions: instructions.into(),
            criteria: options
                .into_iter()
                .map(|(id, description)| (id.into(), description))
                .collect(),
        }
    }

    /// A score over `levels`, lowest first.
    pub fn score<I>(instructions: impl Into<Value>, levels: I) -> Self
    where
        I: IntoIterator,
        I::Item: Into<Value>,
    {
        Self::Score {
            instructions: instructions.into(),
            criteria: levels.into_iter().map(Into::into).collect(),
        }
    }

    /// Whether the question is well formed by the rules both vendors
    /// share: a non-blank string of instructions is not required
    /// (a structure is legal), a choice carries
    /// [`MIN_OPTIONS`]..=[`MAX_OPTIONS`] non-empty ids, a score
    /// carries [`MIN_LEVELS`]..=[`MAX_LEVELS`] levels. The message
    /// names the rule, for the model to read.
    pub fn validate(&self) -> Result<(), String> {
        match self {
            Self::Noul { .. } => Ok(()),
            Self::Choice { criteria, .. } => {
                if criteria.len() < MIN_OPTIONS {
                    return Err(format!(
                        "a choice needs at least {MIN_OPTIONS} options, got {}",
                        criteria.len()
                    ));
                }
                if criteria.len() > MAX_OPTIONS {
                    return Err(format!(
                        "a choice takes at most {MAX_OPTIONS} options, got {}",
                        criteria.len()
                    ));
                }
                if let Some(empty) = criteria.keys().find(|id| id.trim().is_empty()) {
                    return Err(format!("an option id is blank: {empty:?}"));
                }
                Ok(())
            }
            Self::Score { criteria, .. } => {
                if !(MIN_LEVELS..=MAX_LEVELS).contains(&criteria.len()) {
                    return Err(format!(
                        "a score takes {MIN_LEVELS} to {MAX_LEVELS} levels, got {}",
                        criteria.len()
                    ));
                }
                Ok(())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_three_kinds_serialize_to_the_documented_wire_shape() {
        let noul = Question::noul("Is this message about billing?");
        assert_eq!(
            serde_json::to_value(&noul).unwrap(),
            json!({"type": "noul", "instructions": "Is this message about billing?"})
        );

        let choice = Question::choice(
            "Which team should handle this",
            [
                ("billing", json!("Payment issues")),
                ("technical", json!("Bugs")),
            ],
        );
        assert_eq!(
            serde_json::to_value(&choice).unwrap(),
            json!({
                "type": "choice",
                "instructions": "Which team should handle this",
                "criteria": {"billing": "Payment issues", "technical": "Bugs"}
            })
        );

        let score = Question::score("How severe", ["Cosmetic", "Broken", "Blocking"]);
        assert_eq!(
            serde_json::to_value(&score).unwrap(),
            json!({
                "type": "score",
                "instructions": "How severe",
                "criteria": ["Cosmetic", "Broken", "Blocking"]
            })
        );
    }

    #[test]
    fn noul_criteria_ride_under_their_two_names() {
        let question = Question::Noul {
            instructions: Value::String("Urgent?".into()),
            criteria: Some(NoulCriteria {
                yes: Some(json!("needs an answer today")),
                no: None,
            }),
        };
        assert_eq!(
            serde_json::to_value(&question).unwrap(),
            json!({
                "type": "noul",
                "instructions": "Urgent?",
                "criteria": {"true": "needs an answer today"}
            })
        );
    }

    #[test]
    fn structured_instructions_and_descriptions_pass_through() {
        let question = Question::choice(
            json!({"question": "Same person?", "record": {"id": 7}}),
            [("yes", json!(null)), ("no", json!(["a", "b"]))],
        );
        let value = serde_json::to_value(&question).unwrap();
        assert_eq!(value["instructions"]["record"]["id"], json!(7));
        assert!(value["criteria"]["yes"].is_null());
        // A round trip keeps the structure.
        let back: Question = serde_json::from_value(value).unwrap();
        assert_eq!(back, question);
    }

    #[test]
    fn validation_names_the_broken_rule() {
        assert!(Question::noul("x").validate().is_ok());
        assert!(Question::score("x", ["a", "b"]).validate().is_ok());
        assert!(Question::score("x", ["a"]).validate().is_err());
        assert!(
            Question::score("x", (0..=10).map(|i| i.to_string()))
                .validate()
                .is_err(),
            "eleven levels is one too many"
        );
        assert!(Question::choice("x", [("a", json!(1))]).validate().is_err());
        assert!(
            Question::choice("x", [("a", json!(1)), (" ", json!(2))])
                .validate()
                .is_err(),
            "a blank option id is refused"
        );
        let many = (0..=255).map(|i| (i.to_string(), json!(null)));
        assert!(Question::choice("x", many).validate().is_err());
    }
}
