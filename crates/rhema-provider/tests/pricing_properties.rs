//! Property tests for the cost arithmetic.
//!
//! `Pricing::cost` is the only figure a session's spend report hangs
//! on, so its shape is the contract: zero tokens cost zero, more
//! tokens never cost less (at fixed rates), a cached token never
//! costs more than the same token uncached when the published cached
//! rate is a discount, and the unpublished-rate fallback is exactly
//! the quarter rate it documents.

use hegel::TestCase;
use hegel::generators::{self as gs};
use rhema_provider::Pricing;

fn a_rate() -> impl gs::PrintableGenerator<f64> {
    // Per-MTok rates in the range real catalogs publish: cents to
    // hundreds of dollars, finite, positive.
    gs::floats::<f64>()
        .min_value(0.01)
        .max_value(400.0)
        .allow_nan(false)
        .allow_infinity(false)
}

fn a_pricing(tc: &TestCase) -> Pricing {
    let input = tc.draw(a_rate());
    let output = tc.draw(a_rate());
    let cached = tc.draw(gs::optional(a_rate()));
    Pricing {
        input_per_mtok: input,
        output_per_mtok: output,
        cached_input_per_mtok: cached,
    }
}

fn tokens(tc: &TestCase, max: u64) -> (u64, u64, u64) {
    let input = tc.draw(gs::integers::<u64>().max_value(max));
    let output = tc.draw(gs::integers::<u64>().max_value(max));
    let cached = tc.draw(gs::integers::<u64>().max_value(input + max / 4));
    (input, output, cached)
}

#[hegel::test(test_cases = 500)]
fn the_cost_keeps_its_shape(tc: TestCase) {
    for _ in 0..4 {
        let pricing = a_pricing(&tc);

        // No tokens, no cost.
        assert_eq!(pricing.cost(0, 0, 0), 0.0, "an empty call was billed");

        // More output never costs less, at fixed input: the terms are
        // non-negative products over an unchanged prefix.
        let (input, _, cached) = tokens(&tc, 1_000_000);
        let smaller = pricing.cost(input, 0, cached);
        let larger = pricing.cost(
            input,
            tc.draw(gs::integers::<u64>().max_value(1_000_000)),
            cached,
        );
        assert!(
            larger >= smaller,
            "output growth lowered the cost: {smaller} -> {larger}"
        );

        // More input never costs less, at fixed output and cache.
        let (_, output, cached) = tokens(&tc, 1_000_000);
        let smaller = pricing.cost(0, output, cached);
        let larger = pricing.cost(
            tc.draw(gs::integers::<u64>().max_value(1_000_000)),
            output,
            cached,
        );
        assert!(
            larger >= smaller,
            "input growth lowered the cost: {smaller} -> {larger}"
        );

        // A published discount: shifting tokens into the cache never
        // raises the bill. Rates compare exactly; the shift can round,
        // so the verdict carries a small tolerance.
        if let Some(cached_rate) = pricing
            .cached_input_per_mtok
            .filter(|rate| *rate < pricing.input_per_mtok)
        {
            let (input, output, _) = tokens(&tc, 1_000_000);
            let uncached_all = pricing.cost(input, output, 0);
            let fully_cached = Pricing {
                cached_input_per_mtok: Some(cached_rate),
                ..pricing
            }
            .cost(input, output, input);
            assert!(
                fully_cached <= uncached_all * (1.0 + 1e-9) + 1e-12,
                "full caching cost more than no caching: {uncached_all} -> {fully_cached}"
            );
        }

        // The fallback is the documented quarter of the input rate:
        // an absent published rate prices exactly that.
        let (input, output, cached) = tokens(&tc, 1_000_000);
        let absent = Pricing {
            cached_input_per_mtok: None,
            ..pricing
        };
        let published = Pricing {
            cached_input_per_mtok: Some(absent.input_per_mtok * 0.25),
            ..absent
        };
        assert_eq!(
            absent.cost(input, output, cached),
            published.cost(input, output, cached),
            "the fallback rate is not the quarter rate"
        );
    }
}
