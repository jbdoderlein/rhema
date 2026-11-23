//! The scripted provider: what the call's tests and a frontend's
//! offline tests drive instead of a vendor. A feature (`mock`), not a
//! dev-dependency, because consumers of this crate need it in their
//! own test graphs; production code never names it.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;

use crate::error::DecisionError;
use crate::provider::DecisionProvider;
use crate::request::DecisionRequest;
use crate::response::DecisionResponse;

/// One provider whose answers come off a script: push outcomes, then
/// decide. An empty script fails with a transport error, so a test
/// that forgets to script an answer sees a failure rather than a
/// silent empty response.
pub struct MockDecisionProvider {
    kind: &'static str,
    script: Mutex<VecDeque<Result<DecisionResponse, DecisionError>>>,
    delay: Duration,
    calls: AtomicUsize,
    last_request: Mutex<Option<DecisionRequest>>,
}

impl MockDecisionProvider {
    /// A provider of `kind` with no script and no delay.
    pub fn new(kind: &'static str) -> Self {
        Self {
            kind,
            script: Mutex::new(VecDeque::new()),
            delay: Duration::ZERO,
            calls: AtomicUsize::new(0),
            last_request: Mutex::new(None),
        }
    }

    /// Appends one successful answer.
    pub fn push_ok(&self, response: DecisionResponse) -> &Self {
        self.script.lock().unwrap().push_back(Ok(response));
        self
    }

    /// Appends one failure.
    pub fn push_err(&self, err: DecisionError) -> &Self {
        self.script.lock().unwrap().push_back(Err(err));
        self
    }

    /// Sleeps this long before every answer, the budget test's stand-in
    /// for a slow vendor.
    pub fn with_delay(mut self, delay: Duration) -> Self {
        self.delay = delay;
        self
    }

    /// How many times `decide` ran: how a test sees that a malformed
    /// request never reached the vendor.
    pub fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }

    /// The request the last `decide` call carried.
    pub fn last_request(&self) -> Option<DecisionRequest> {
        self.last_request.lock().unwrap().clone()
    }

    /// A shared handle, the shape the registry hands a frontend.
    pub fn shared(self) -> Arc<Self> {
        Arc::new(self)
    }
}

#[async_trait]
impl DecisionProvider for MockDecisionProvider {
    fn kind(&self) -> &'static str {
        self.kind
    }

    async fn decide(&self, request: &DecisionRequest) -> Result<DecisionResponse, DecisionError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        *self.last_request.lock().unwrap() = Some(request.clone());
        if !self.delay.is_zero() {
            tokio::time::sleep(self.delay).await;
        }
        self.script.lock().unwrap().pop_front().unwrap_or_else(|| {
            Err(DecisionError::Transport(
                "the mock has no scripted answer".to_owned(),
            ))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::answer::Answer;
    use crate::question::Question;
    use std::collections::BTreeMap;

    #[tokio::test]
    async fn an_empty_script_fails_loudly() {
        let provider = MockDecisionProvider::new("mock");
        let request = DecisionRequest::new("s").with_question("q", Question::noul("x"));
        let answer = provider.decide(&request).await;
        assert!(matches!(answer, Err(DecisionError::Transport(_))));
        assert_eq!(provider.calls(), 1);
        assert_eq!(provider.last_request().unwrap(), request);
    }

    #[tokio::test]
    async fn a_scripted_answer_comes_back_in_order() {
        let provider = MockDecisionProvider::new("jev");
        provider.push_ok(DecisionResponse::new(
            "jev-1.13.0",
            BTreeMap::from([("q".to_owned(), Answer::Noul { noul: 0.9 })]),
        ));
        let request = DecisionRequest::new("s").with_question("q", Question::noul("x"));
        let answer = provider.decide(&request).await.unwrap();
        assert_eq!(answer.model, "jev-1.13.0");
        assert_eq!(provider.kind(), "jev");
    }
}
