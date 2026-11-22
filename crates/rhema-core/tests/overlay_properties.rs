//! Property tests for the overlay combiner.
//!
//! The model sees one resolver built from a primary (the built-ins)
//! and an extra (an extension's tools). The contract: the extra
//! shadows a primary of the same name, everywhere at once — the spec
//! list carries the name once, from the extra side, and a call for
//! that name dispatches to the extra side — and every other name
//! dispatches to the primary. The properties run over generated name
//! sets, so the shadowing, the ordering, and the dispatch agree on
//! every combination, not just the hand-picked ones.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use hegel::TestCase;
use hegel::generators::{self as gs, TextGenerator};
use rhema_core::{
    OutputOnly, Overlay, ReplaySafety, ToolCall, ToolContext, ToolResolver, ToolResult, ToolSpec,
};
use serde_json::json;

/// Which side answered: the primary (`P`) or the extra (`E`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Side {
    Primary,
    Extra,
}

/// A resolver offering drawn names, recording every call it takes and
/// answering a drawn safety class for the names it offers.
struct Fake {
    tag: Side,
    names: Vec<String>,
    safety: ReplaySafety,
    taken: Arc<Mutex<Vec<(Side, String)>>>,
}

impl Fake {
    fn resolver(tag: Side, names: Vec<String>, safety: ReplaySafety) -> Arc<Fake> {
        Arc::new(Self {
            tag,
            names,
            safety,
            taken: Arc::new(Mutex::new(Vec::new())),
        })
    }

    fn taken(&self) -> Vec<(Side, String)> {
        self.taken
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

#[async_trait]
impl ToolResolver for Fake {
    fn specs(&self) -> Vec<ToolSpec> {
        self.names
            .iter()
            .map(|name| ToolSpec::new(name.clone(), "a fake tool", json!({"type": "object"})))
            .collect()
    }

    fn replay_safety(&self, name: &str) -> ReplaySafety {
        if self.names.iter().any(|offered| offered == name) {
            self.safety
        } else {
            ReplaySafety::Unsafe
        }
    }

    async fn resolve(&self, call: ToolCall, _ctx: &ToolContext) -> ToolResult {
        self.taken
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push((self.tag, call.name));
        ToolResult::success(call.id, "done")
    }

    async fn resolve_streaming(
        &self,
        call: ToolCall,
        _ctx: &ToolContext,
        _progress: &mut dyn rhema_core::ToolProgress,
    ) -> ToolResult {
        self.taken
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push((self.tag, call.name));
        ToolResult::success(call.id, "done")
    }
}

/// Short names over a small alphabet: collisions between the sides
/// (the shadowing case) happen often, and unknown names (the alphabet
/// is wider than the offered sets) stay in the draw.
fn name() -> TextGenerator {
    gs::text().min_size(1).max_size(2).alphabet("abcd")
}

fn unique(names: Vec<String>) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    names
        .into_iter()
        .filter(|name| seen.insert(name.clone()))
        .collect()
}

#[hegel::test(test_cases = 300)]
fn the_extra_shadows_the_primary_everywhere_at_once(tc: TestCase) {
    let primary_names = unique(tc.draw(gs::vecs(name()).max_size(4)));
    let extra_names = unique(tc.draw(gs::vecs(name()).max_size(4)));
    let primary_safety = tc.draw(gs::sampled_from(&[
        ReplaySafety::Safe,
        ReplaySafety::Unsafe,
    ]));
    let extra_safety = tc.draw(gs::sampled_from(&[
        ReplaySafety::Safe,
        ReplaySafety::Unsafe,
    ]));

    let primary = Fake::resolver(Side::Primary, primary_names.clone(), primary_safety);
    let extra = Fake::resolver(Side::Extra, extra_names.clone(), extra_safety);
    let overlay = Overlay::new(primary.clone(), extra.clone());

    // The spec list: the primary in its own order minus every name the
    // extra offers, then the extra in its own order. A shadowed name
    // appears exactly once, and every offered name appears.
    let expected_specs: Vec<String> = primary_names
        .iter()
        .filter(|name| !extra_names.contains(name))
        .cloned()
        .chain(extra_names.iter().cloned())
        .collect();
    let specs = overlay.specs();
    let spec_names: Vec<String> = specs.iter().map(|spec| spec.name.clone()).collect();
    assert_eq!(spec_names, expected_specs, "the spec list diverged");

    // Dispatch: a call for a name the extra offers lands on the extra,
    // whatever else offers it; every other name lands on the primary.
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("a runtime");
    let ctx = ToolContext::default();
    let mut expected_taken = Vec::new();
    let mut calls = Vec::new();
    for name in primary_names.iter().chain(extra_names.iter()) {
        calls.push(name.clone());
        let side = if extra_names.contains(name) {
            Side::Extra
        } else {
            Side::Primary
        };
        expected_taken.push((side, name.clone()));
    }
    // Names nobody offers still reach the primary: the fallback side.
    for name in ["zz", "qq"] {
        if primary_names.contains(&name.to_string()) || extra_names.contains(&name.to_string()) {
            continue;
        }
        calls.push(name.to_owned());
        expected_taken.push((Side::Primary, name.to_owned()));
    }
    for name in &calls {
        let call = ToolCall {
            name: name.clone(),
            ..ToolCall::default()
        };
        let result = runtime.block_on(overlay.resolve(call, &ctx));
        assert!(!result.is_error, "a dispatched call failed");
    }
    let expected_side = |side: Side| -> Vec<(Side, String)> {
        expected_taken
            .iter()
            .filter(|(landed, _)| *landed == side)
            .cloned()
            .collect()
    };
    assert_eq!(
        primary.taken(),
        expected_side(Side::Primary),
        "dispatch diverged"
    );
    assert_eq!(
        extra.taken(),
        expected_side(Side::Extra),
        "dispatch diverged"
    );
    primary.taken.lock().unwrap().clear();
    extra.taken.lock().unwrap().clear();

    // Streaming dispatch follows the same side, call for call.
    for name in &calls {
        let call = ToolCall {
            name: name.clone(),
            ..ToolCall::default()
        };
        let result =
            runtime.block_on(overlay.resolve_streaming(call, &ctx, &mut OutputOnly(|_| {})));
        assert!(!result.is_error, "a streamed dispatch failed");
    }
    assert_eq!(
        primary.taken(),
        expected_side(Side::Primary),
        "streaming dispatch diverged"
    );
    assert_eq!(
        extra.taken(),
        expected_side(Side::Extra),
        "streaming dispatch diverged"
    );

    // The safety class follows the dispatch side: the extra's answer
    // for a name it offers, the primary's for anything else.
    for name in expected_specs.iter().chain(calls.iter()) {
        let expected = if extra_names.contains(name) {
            extra_safety
        } else if primary_names.contains(name) {
            primary_safety
        } else {
            ReplaySafety::Unsafe
        };
        assert_eq!(
            overlay.replay_safety(name),
            expected,
            "replay safety diverged for {name}"
        );
    }
}
