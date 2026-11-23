//! The three answer kinds, one per question kind, as the vendors
//! return them.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// One answer, serialized as the `{"type": ..., ...}` object the
/// System One wire carries, keyed under its question's name.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Answer {
    /// The probability that the answer is yes: near 1 a strong yes,
    /// near 0 a strong no, near 0.5 uncertain. No separate confidence:
    /// the number is the whole answer.
    Noul {
        /// The probability of yes, 0 to 1.
        noul: f64,
    },
    /// The option with the highest probability, the full
    /// distribution, and a confidence derived from how peaked that
    /// distribution is.
    Choice {
        /// The option with the highest probability.
        choice: String,
        /// How certain the model is, 0 to 1, computed from the spread
        /// of `probabilities`.
        confidence: f64,
        /// The probability per option; the values sum to 1.
        probabilities: BTreeMap<String, f64>,
    },
    /// A position along the rubric that can fall between two levels,
    /// the levels by number, the distribution, and a confidence.
    Score {
        /// The probability-weighted level, 0 (the lowest) to
        /// `levels - 1`.
        score: f64,
        /// How certain the model is, 0 to 1.
        confidence: f64,
        /// The levels by number, as the request wrote them. Jev
        /// echoes the rubric back; OpenRouter does not, so it may be
        /// empty. The renderer never reads it.
        #[serde(default)]
        legend: BTreeMap<String, Value>,
        /// The probability per level; the values sum to 1.
        probabilities: BTreeMap<String, f64>,
    },
    /// The provider declined this one question (a content policy,
    /// usually) while answering the rest: not answered, never a
    /// negative verdict. OpenRouter's wire carries it as
    /// `{"type": "refusal"}`.
    Refusal,
    /// A kind this seam does not model (the image `box`). A request
    /// built here can never ask one, so the variant exists only so a
    /// response carrying one parses instead of failing whole.
    #[serde(other)]
    Other,
}

impl Answer {
    /// The confidence the model reports, `None` for a `noul`, whose
    /// probability is its own certainty.
    pub fn confidence(&self) -> Option<f64> {
        match self {
            Self::Choice { confidence, .. } | Self::Score { confidence, .. } => Some(*confidence),
            Self::Noul { .. } | Self::Refusal | Self::Other => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_documented_answers_parse() {
        let noul: Answer = serde_json::from_value(json!({"type": "noul", "noul": 0.98})).unwrap();
        assert_eq!(noul, Answer::Noul { noul: 0.98 });
        assert_eq!(noul.confidence(), None);

        let choice: Answer = serde_json::from_value(json!({
            "type": "choice", "choice": "calm", "confidence": 0.7,
            "probabilities": {"angry": 0.15, "calm": 0.8, "excited": 0.05}
        }))
        .unwrap();
        assert_eq!(choice.confidence(), Some(0.7));
        let Answer::Choice {
            choice,
            probabilities,
            ..
        } = choice
        else {
            panic!("expected a choice");
        };
        assert_eq!(choice, "calm");
        assert_eq!(probabilities["excited"], 0.05);

        let score: Answer = serde_json::from_value(json!({
            "type": "score", "score": 1.7, "confidence": 0.55,
            "legend": {"0": "Can wait", "1": "This week", "2": "Today"},
            "probabilities": {"0": 0.1, "1": 0.1, "2": 0.8}
        }))
        .unwrap();
        let Answer::Score {
            score,
            legend,
            probabilities,
            ..
        } = score
        else {
            panic!("expected a score");
        };
        assert_eq!(score, 1.7);
        assert_eq!(legend["2"], json!("Today"));
        assert_eq!(probabilities.len(), 3);
    }

    #[test]
    fn a_structured_legend_round_trips() {
        let score: Answer = serde_json::from_value(json!({
            "type": "score", "score": 1.09, "confidence": 0.87,
            "legend": {"0": {"what": "Cosmetic", "examples": ["typo"]}},
            "probabilities": {"0": 0.0}
        }))
        .unwrap();
        let Answer::Score { legend, .. } = &score else {
            panic!("expected a score");
        };
        assert_eq!(legend["0"]["examples"][0], json!("typo"));
        assert_eq!(
            serde_json::to_value(&score).unwrap(),
            serde_json::to_value(&score).unwrap()
        );
    }

    #[test]
    fn a_score_without_a_legend_parses() {
        // OpenRouter's wire: the rubric is not echoed back.
        let score: Answer = serde_json::from_value(json!({
            "type": "score", "score": 1.84, "confidence": 0.85,
            "probabilities": {"0": 0.01, "1": 0.14, "2": 0.85}
        }))
        .unwrap();
        let Answer::Score { legend, .. } = &score else {
            panic!("expected a score");
        };
        assert!(legend.is_empty());
    }

    #[test]
    fn a_refusal_is_its_own_kind_and_round_trips() {
        let refusal: Answer = serde_json::from_value(json!({"type": "refusal"})).unwrap();
        assert_eq!(refusal, Answer::Refusal);
        assert_eq!(refusal.confidence(), None);
        assert_eq!(
            serde_json::to_value(&refusal).unwrap(),
            json!({"type": "refusal"})
        );
    }

    #[test]
    fn an_unknown_kind_parses_as_other() {
        let other: Answer =
            serde_json::from_value(json!({"type": "box", "box": {"top": 0.2}})).unwrap();
        assert_eq!(other, Answer::Other);
    }

    #[test]
    fn answers_round_trip_through_the_wire_shape() {
        let choice = Answer::Choice {
            choice: "a".into(),
            confidence: 1.0,
            probabilities: BTreeMap::from([("a".to_owned(), 1.0)]),
        };
        let value = serde_json::to_value(&choice).unwrap();
        assert_eq!(
            value,
            json!({"type": "choice", "choice": "a", "confidence": 1.0, "probabilities": {"a": 1.0}})
        );
        let back: Answer = serde_json::from_value(value).unwrap();
        assert_eq!(back, choice);
    }
}
