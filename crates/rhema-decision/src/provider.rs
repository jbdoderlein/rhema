//! The adapter seam: one vendor's System One API behind one method.

use async_trait::async_trait;

use crate::error::DecisionError;
use crate::request::DecisionRequest;
use crate::response::DecisionResponse;

/// One vendor's decision model. The profile names one; the frontend
/// holds it behind the trait and [`crate::decide`] is the call. Send +
/// Sync like rhema-core's other seams, the call is one short awaited
/// HTTP request.
#[async_trait]
pub trait DecisionProvider: Send + Sync {
    /// The vendor spelling the config uses: `jev`, `clef`. A constant
    /// per adapter, it names the error and output lines, never a
    /// second constructor path.
    fn kind(&self) -> &'static str;

    /// Asks every question about the state, one answer per question.
    /// A request the vendor refuses is
    /// [`Invalid`](DecisionError::Invalid); every other failure is one
    /// of [`DecisionError`]'s classes.
    async fn decide(&self, request: &DecisionRequest) -> Result<DecisionResponse, DecisionError>;
}
