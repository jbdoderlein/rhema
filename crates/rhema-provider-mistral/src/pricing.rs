//! Pay-as-you-go prices for Mistral's chat models, USD per 1M tokens.
//!
//! The ListModels API carries no prices, so this is a curated table.
//! Source: docs.mistral.ai/inference/pricing (fetched 2026-11-18). A
//! Vibe subscription draws from the same included monthly usage and
//! pay-as-you-go balance, at the same rates; unknown models show no
//! cost.

use rhema_provider::Pricing;

/// The table entry for `id`, or `None` when unknown. Entries are exact
/// ids: not part of the family fall back rather than inheriting a
/// sibling's rate.
pub fn model_pricing(id: &str) -> Option<Pricing> {
    let table: &[(&str, Pricing)] = &[
        // Generalist models.
        (
            "mistral-large-4",
            Pricing {
                input_per_mtok: 0.70,
                output_per_mtok: 2.10,
                cached_input_per_mtok: Some(0.07),
            },
        ),
        (
            "mistral-large-4-0",
            Pricing {
                input_per_mtok: 0.70,
                output_per_mtok: 2.10,
                cached_input_per_mtok: Some(0.07),
            },
        ),
        (
            "mistral-small-4",
            Pricing {
                input_per_mtok: 0.15,
                output_per_mtok: 0.60,
                cached_input_per_mtok: Some(0.015),
            },
        ),
        (
            "mistral-small-4-0",
            Pricing {
                input_per_mtok: 0.15,
                output_per_mtok: 0.60,
                cached_input_per_mtok: Some(0.015),
            },
        ),
        (
            "mistral-small-4-0-26-03",
            Pricing {
                input_per_mtok: 0.15,
                output_per_mtok: 0.60,
                cached_input_per_mtok: Some(0.015),
            },
        ),
        // The Vibe CLI's Medium 3.5 alias.
        (
            "mistral-vibe-cli-latest",
            Pricing {
                input_per_mtok: 1.50,
                output_per_mtok: 7.50,
                cached_input_per_mtok: Some(0.15),
            },
        ),
        // Ministral family.
        (
            "ministral-3-14b-25-12",
            Pricing {
                input_per_mtok: 0.20,
                output_per_mtok: 0.20,
                cached_input_per_mtok: Some(0.02),
            },
        ),
        (
            "ministral-3-8b-25-12",
            Pricing {
                input_per_mtok: 0.15,
                output_per_mtok: 0.15,
                cached_input_per_mtok: Some(0.015),
            },
        ),
        (
            "ministral-3-3b-25-12",
            Pricing {
                input_per_mtok: 0.10,
                output_per_mtok: 0.10,
                cached_input_per_mtok: Some(0.01),
            },
        ),
        // Code models.
        (
            "codestral-25-08",
            Pricing {
                input_per_mtok: 0.30,
                output_per_mtok: 0.90,
                cached_input_per_mtok: Some(0.03),
            },
        ),
        (
            "codestral-2508",
            Pricing {
                input_per_mtok: 0.30,
                output_per_mtok: 0.90,
                cached_input_per_mtok: Some(0.03),
            },
        ),
    ];
    table
        .iter()
        .find(|(entry, _)| *entry == id)
        .map(|(_, pricing)| *pricing)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_models_price_and_unknown_ids_do_not() {
        let large = model_pricing("mistral-large-4-0").unwrap();
        assert_eq!(large.input_per_mtok, 0.70);
        assert_eq!(large.output_per_mtok, 2.10);
        assert_eq!(large.cached_input_per_mtok, Some(0.07));
        // The API's shorter alias prices the same.
        assert_eq!(model_pricing("mistral-large-4"), Some(large));
        assert_eq!(model_pricing("mistral-embed-23-12"), None);
        assert_eq!(model_pricing("some-fine-tune"), None);
    }
}
