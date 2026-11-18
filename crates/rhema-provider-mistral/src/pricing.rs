//! Pay-as-you-go prices for Mistral's chat models, USD per 1M tokens.
//!
//! The ListModels API carries no prices, so this is a curated table.
//! Source: docs.mistral.ai/inference/pricing (fetched 2026-11-18). A
//! Vibe subscription draws from the same included monthly usage and
//! pay-as-you-go balance, at the same rates; unknown models show no
//! cost.
//!
//! The Vibe catalog also routes models the pricing page never lists:
//! Mistral's own Medium and the zai pair it hosts. Those three were
//! measured, not read: each row below is the rate the live
//! `billing/budget` meter charged for a single request with known
//! token counts (2026-11-18), cross-checked to the meter on at least
//! two independent requests. The `-vibe-free` flash alias draws a
//! rate that does not match the flash table's, so it stays unpriced.

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
        // Medium, the model behind the Vibe CLI's own alias. The catalog
        // id and the alias bill identically on the meter.
        (
            "mistral-medium-latest",
            Pricing {
                input_per_mtok: 1.50,
                output_per_mtok: 7.50,
                cached_input_per_mtok: Some(0.15),
            },
        ),
        (
            "mistral-vibe-cli-latest",
            Pricing {
                input_per_mtok: 1.50,
                output_per_mtok: 7.50,
                cached_input_per_mtok: Some(0.15),
            },
        ),
        // The zai pair Mistral hosts for Vibe, measured off the meter.
        // The flash variant reports zero cached tokens on every request,
        // fresh or repeated, so it publishes no cached rate.
        (
            "zai-glm-5-3",
            Pricing {
                input_per_mtok: 1.40,
                output_per_mtok: 4.40,
                cached_input_per_mtok: Some(0.14),
            },
        ),
        (
            "zai-glm-5-3-flash",
            Pricing {
                input_per_mtok: 0.15,
                output_per_mtok: 0.50,
                cached_input_per_mtok: None,
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

        // The measured Vibe rows: Medium bills like its CLI alias, the
        // zai pair carries the meter's own numbers, and the free flash
        // alias stays unpriced.
        let medium = model_pricing("mistral-medium-latest").unwrap();
        let alias = model_pricing("mistral-vibe-cli-latest").unwrap();
        assert_eq!(medium, alias);
        let glm = model_pricing("zai-glm-5-3").unwrap();
        assert_eq!(glm.input_per_mtok, 1.40);
        assert_eq!(glm.output_per_mtok, 4.40);
        assert_eq!(glm.cached_input_per_mtok, Some(0.14));
        let flash = model_pricing("zai-glm-5-3-flash").unwrap();
        assert_eq!(flash.input_per_mtok, 0.15);
        assert_eq!(flash.output_per_mtok, 0.50);
        assert_eq!(flash.cached_input_per_mtok, None);
        assert_eq!(model_pricing("zai-glm-5-3-flash-vibe-free"), None);
    }
}
