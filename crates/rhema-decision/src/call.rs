//! One call: one provider, one request, validated first and bounded
//! by a budget.

use std::time::Duration;

use tokio::time::timeout;

use crate::error::DecisionError;
use crate::provider::DecisionProvider;
use crate::request::DecisionRequest;
use crate::response::DecisionResponse;

/// The call's budget. The vendors answer in well under a second for
/// short states; this sits far under the shell tool's command
/// timeout, so the call's own error text reaches the model instead
/// of a killed command's.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(15);

/// Runs `request` against `provider`, the one call every `decide`
/// makes, under [`DEFAULT_TIMEOUT`].
///
/// The request is validated first, so a malformed one costs no vendor
/// call; the vendor's own refusal ([`DecisionError::Invalid`]) comes
/// back as is; the budget bounds the wait, and a vendor that exceeds
/// it fails with [`DecisionError::Timeout`].
pub async fn decide(
    provider: &dyn DecisionProvider,
    request: &DecisionRequest,
) -> Result<DecisionResponse, DecisionError> {
    decide_within(provider, request, DEFAULT_TIMEOUT).await
}

/// [`decide`] under a caller's budget.
pub async fn decide_within(
    provider: &dyn DecisionProvider,
    request: &DecisionRequest,
    budget: Duration,
) -> Result<DecisionResponse, DecisionError> {
    request.validate().map_err(DecisionError::Invalid)?;
    timeout(budget, provider.decide(request))
        .await
        .unwrap_or(Err(DecisionError::Timeout))
}

#[cfg(all(test, feature = "mock"))]
mod tests {
    use super::*;
    use crate::answer::Answer;
    use crate::mock::MockDecisionProvider;
    use crate::question::Question;
    use std::collections::BTreeMap;

    fn response(noul: f64) -> DecisionResponse {
        DecisionResponse::new(
            "mock-1",
            BTreeMap::from([("q".to_owned(), Answer::Noul { noul })]),
        )
    }

    fn request() -> DecisionRequest {
        DecisionRequest::new("state").with_question("q", Question::noul("yes?"))
    }

    #[tokio::test]
    async fn the_answer_comes_back_and_the_provider_sees_the_request() {
        let provider = MockDecisionProvider::new("jev").shared();
        provider.push_ok(response(0.9));
        let answer = decide(provider.as_ref(), &request()).await.unwrap();
        assert_eq!(answer.answers["q"], Answer::Noul { noul: 0.9 });
        assert_eq!(provider.last_request().unwrap(), request());
    }

    #[tokio::test]
    async fn a_malformed_request_costs_no_vendor_call() {
        let provider = MockDecisionProvider::new("jev").shared();
        let err = decide(provider.as_ref(), &DecisionRequest::new("state"))
            .await
            .unwrap_err();
        assert_eq!(
            err,
            DecisionError::Invalid("a request needs at least one question".into())
        );
        assert_eq!(provider.calls(), 0);
    }

    #[tokio::test]
    async fn the_vendors_refusal_and_every_other_failure_come_back_as_is() {
        let provider = MockDecisionProvider::new("jev").shared();
        provider.push_err(DecisionError::Invalid("too many options".into()));
        provider.push_err(DecisionError::Auth);
        provider.push_err(DecisionError::RateLimited);
        let request = request();
        assert_eq!(
            decide(provider.as_ref(), &request).await.unwrap_err(),
            DecisionError::Invalid("too many options".into())
        );
        assert_eq!(
            decide(provider.as_ref(), &request).await.unwrap_err(),
            DecisionError::Auth
        );
        assert_eq!(
            decide(provider.as_ref(), &request).await.unwrap_err(),
            DecisionError::RateLimited
        );
    }

    #[tokio::test]
    async fn a_slow_vendor_times_out() {
        let slow = MockDecisionProvider::new("jev")
            .with_delay(Duration::from_millis(500))
            .shared();
        slow.push_ok(response(0.5));
        let err = decide_within(slow.as_ref(), &request(), Duration::from_millis(50))
            .await
            .unwrap_err();
        assert_eq!(err, DecisionError::Timeout);
    }
}
