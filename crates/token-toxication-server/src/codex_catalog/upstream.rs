//! Turning an upstream Codex catalog into the body TT serves.
//!
//! TT proxies the account's own catalog, so upstream is the single source of
//! truth for which models exist, their context windows, their service tiers, and
//! their instruction templates. TT authors nothing and rewrites nothing.
//!
//! Two properties of the Codex client decide what TT must still do:
//!
//! 1. A catalog supplied through `model_catalog_url` **replaces** the bundled
//!    catalog, and it is parsed as a whole. `ModelsResponse` deserializes with
//!    `deserialize_model_infos_with_legacy_base`, which hard-errors when an entry
//!    has neither `model_messages.instructions_template` nor the legacy
//!    `base_instructions`; the client also rejects duplicate or empty slugs.
//!    One unusable entry therefore costs the client *every* model, so TT drops
//!    the entry instead of forwarding a catalog that cannot be parsed.
//! 2. The client refuses an explicitly configured catalog larger than 1 MiB
//!    before decoding it. TT keeps the served body under that cap.
//!
//! Everything else is passed through verbatim, including models upstream marks
//! hidden and models TT has no route for. Clients render their picker from
//! metadata that includes models TT cannot route, and dropping those entries
//! breaks the client's model list; an unroutable selection fails per request
//! instead.
//!
//! The module is pure: no I/O, and no logging of model bodies.

use serde_json::{Value, json};

/// Largest body TT serves to a client.
///
/// The Codex client rejects an explicitly configured catalog above 1 MiB before
/// decoding it, and a rejected catalog leaves the client with no models at all.
/// TT stays under that cap with margin for the JSON envelope.
pub const CLIENT_CATALOG_BUDGET_BYTES: usize = 1_048_576 - 32_768;

/// Why an upstream catalog could not be read at all.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CatalogError {
    #[error("upstream catalog is missing the `models` array")]
    MissingModels,
}

/// Why one upstream entry was left out of the served catalog.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DropReason {
    /// The entry has no usable `slug`, so the client cannot address it.
    MissingSlug,
    /// An earlier entry already used this slug.
    DuplicateSlug,
    /// The client would reject the entire catalog for this entry.
    MissingInstructions,
    /// The entry would push the served body past the client's size cap.
    OverSizeBudget,
}

impl DropReason {
    /// Bounded code for structured logs. Never carries upstream text.
    pub fn code(self) -> &'static str {
        match self {
            Self::MissingSlug => "missing_slug",
            Self::DuplicateSlug => "duplicate_slug",
            Self::MissingInstructions => "missing_instructions",
            Self::OverSizeBudget => "over_size_budget",
        }
    }
}

/// One omitted entry, identified by slug when it had one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DroppedModel {
    pub slug: String,
    pub reason: DropReason,
}

/// The body TT serves, with bounded counters for structured logs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderedCatalog {
    /// A complete `{"models": [...]}` JSON document.
    pub body: Vec<u8>,
    /// Entries included in `body`.
    pub kept: usize,
    /// Entries left out, in upstream order.
    pub dropped: Vec<DroppedModel>,
}

/// Field Codex reads for the instruction template.
const MODEL_MESSAGES: &str = "model_messages";
const INSTRUCTIONS_TEMPLATE: &str = "instructions_template";
/// Legacy top-level field Codex promotes into `model_messages`.
const BASE_INSTRUCTIONS: &str = "base_instructions";

/// Extract the `models` array from an upstream catalog body.
pub fn models_from_body(body: &Value) -> Result<&Vec<Value>, CatalogError> {
    body.get("models")
        .and_then(Value::as_array)
        .ok_or(CatalogError::MissingModels)
}

/// Whether an entry carries a non-empty instruction template in either form.
pub fn has_instructions(model: &Value) -> bool {
    let template = model
        .get(MODEL_MESSAGES)
        .and_then(|messages| messages.get(INSTRUCTIONS_TEMPLATE))
        .and_then(Value::as_str);
    if template.is_some_and(|value| !value.trim().is_empty()) {
        return true;
    }
    model
        .get(BASE_INSTRUCTIONS)
        .and_then(Value::as_str)
        .is_some_and(|value| !value.trim().is_empty())
}

fn slug_of(model: &Value) -> Option<&str> {
    model
        .get("slug")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|slug| !slug.is_empty())
}

/// Serve the upstream catalog with only the entries a client could not accept
/// removed, in upstream order.
///
/// Nothing is dropped for product policy: every model upstream advertises is
/// passed through, including ones TT has no route for.
pub fn render_catalog(models: &[Value]) -> RenderedCatalog {
    let mut kept: Vec<Value> = Vec::with_capacity(models.len());
    let mut dropped: Vec<DroppedModel> = Vec::new();

    for model in models {
        let Some(slug) = slug_of(model) else {
            dropped.push(DroppedModel {
                slug: String::new(),
                reason: DropReason::MissingSlug,
            });
            continue;
        };
        if kept.iter().any(|existing| slug_of(existing) == Some(slug)) {
            dropped.push(DroppedModel {
                slug: slug.to_string(),
                reason: DropReason::DuplicateSlug,
            });
            continue;
        }
        if !has_instructions(model) {
            dropped.push(DroppedModel {
                slug: slug.to_string(),
                reason: DropReason::MissingInstructions,
            });
            continue;
        }
        kept.push(model.clone());
    }

    let mut body = encode(&kept);
    // The client reads the whole document or nothing, so an oversized catalog is
    // trimmed from the end rather than served in a form the client rejects.
    while body.len() > CLIENT_CATALOG_BUDGET_BYTES && !kept.is_empty() {
        let removed = kept.pop().expect("a non-empty catalog has a last entry");
        dropped.push(DroppedModel {
            slug: slug_of(&removed).unwrap_or_default().to_string(),
            reason: DropReason::OverSizeBudget,
        });
        body = encode(&kept);
    }

    RenderedCatalog {
        body,
        kept: kept.len(),
        dropped,
    }
}

/// A `Value` always serializes, so encoding cannot fail.
fn encode(models: &[Value]) -> Vec<u8> {
    serde_json::to_vec(&json!({ "models": models })).expect("a JSON value always serializes")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn instructions(count: usize) -> String {
        "x".repeat(count)
    }

    fn upstream_model(slug: &str) -> Value {
        json!({
            "slug": slug,
            "display_name": slug,
            "supported_reasoning_levels": [],
            "shell_type": "unified_exec",
            "visibility": "list",
            "supported_in_api": true,
            "priority": 2,
            "truncation_policy": {"mode": "tokens", "limit": 10000},
            "context_window": 500_000,
            "max_context_window": 872_000,
            "service_tiers": [{"id": "priority", "name": "Fast"}],
            "additional_speed_tiers": ["fast"],
            "model_messages": {"instructions_template": instructions(64)},
        })
    }

    fn served_slugs(catalog: &RenderedCatalog) -> Vec<String> {
        let body: Value = serde_json::from_slice(&catalog.body).expect("decode served body");
        body["models"]
            .as_array()
            .expect("models array")
            .iter()
            .map(|model| model["slug"].as_str().unwrap_or_default().to_string())
            .collect()
    }

    #[test]
    fn passes_every_upstream_model_through_including_ones_tt_does_not_route() {
        // Regression: TT used to publish only the GPT-6 family. The Codex
        // Desktop picker builds its categories from the full account catalog,
        // and a trimmed catalog left the picker unable to list any model.
        let models = vec![
            upstream_model("gpt-5.6-sol"),
            upstream_model("gpt-5.6-terra"),
            upstream_model("gpt-5.6-luna"),
            upstream_model("gpt-5.5"),
            upstream_model("gpt-6-astra"),
            upstream_model("gpt-6.1-sol"),
            upstream_model("gpt-6-sol"),
            upstream_model("gpt-6-luna"),
        ];
        let catalog = render_catalog(&models);
        assert_eq!(
            served_slugs(&catalog),
            vec![
                "gpt-5.6-sol",
                "gpt-5.6-terra",
                "gpt-5.6-luna",
                "gpt-5.5",
                "gpt-6-astra",
                "gpt-6.1-sol",
                "gpt-6-sol",
                "gpt-6-luna",
            ]
        );
        assert!(catalog.dropped.is_empty());
        assert_eq!(catalog.kept, 8);
    }

    #[test]
    fn invents_no_fast_variants() {
        // Fast is a service tier on the base entry; a derived `-fast` slug is a
        // model the account does not have.
        let catalog = render_catalog(&[upstream_model("gpt-6.1-sol")]);
        assert_eq!(served_slugs(&catalog), vec!["gpt-6.1-sol"]);
    }

    #[test]
    fn keeps_hidden_and_non_api_entries_so_client_metadata_stays_complete() {
        let mut hidden = upstream_model("gpt-reserve");
        hidden["visibility"] = json!("hide");
        let mut not_in_api = upstream_model("codex-auto-review");
        not_in_api["supported_in_api"] = json!(false);

        let catalog = render_catalog(&[hidden, not_in_api]);
        assert_eq!(catalog.kept, 2);
        assert!(catalog.dropped.is_empty());
    }

    #[test]
    fn drops_an_entry_the_client_would_reject_instead_of_failing_the_catalog() {
        // One unparsable entry costs the client every model, so the catalog is
        // trimmed rather than forwarded.
        let mut broken = upstream_model("gpt-6-sol");
        broken.as_object_mut().unwrap().remove("model_messages");

        let catalog = render_catalog(&[upstream_model("gpt-6.1-sol"), broken]);
        assert_eq!(served_slugs(&catalog), vec!["gpt-6.1-sol"]);
        assert_eq!(
            catalog.dropped,
            vec![DroppedModel {
                slug: "gpt-6-sol".to_string(),
                reason: DropReason::MissingInstructions,
            }]
        );
    }

    #[test]
    fn accepts_the_legacy_base_instructions_field() {
        let mut model = upstream_model("gpt-6-astra");
        model.as_object_mut().unwrap().remove("model_messages");
        model["base_instructions"] = json!(instructions(32));

        let catalog = render_catalog(&[model]);
        assert_eq!(served_slugs(&catalog), vec!["gpt-6-astra"]);
    }

    #[test]
    fn rejects_blank_instruction_templates() {
        let mut model = upstream_model("gpt-6-sol");
        model["model_messages"] = json!({"instructions_template": "   "});

        let catalog = render_catalog(&[model]);
        assert_eq!(
            catalog.dropped.first().map(|entry| entry.reason),
            Some(DropReason::MissingInstructions)
        );
        assert_eq!(served_slugs(&catalog), Vec::<String>::new());
    }

    #[test]
    fn keeps_the_first_of_duplicate_slugs() {
        let mut second = upstream_model("gpt-6-sol");
        second["display_name"] = json!("duplicate");

        let catalog = render_catalog(&[upstream_model("gpt-6-sol"), second]);
        assert_eq!(served_slugs(&catalog), vec!["gpt-6-sol"]);
        assert_eq!(
            catalog.dropped,
            vec![DroppedModel {
                slug: "gpt-6-sol".to_string(),
                reason: DropReason::DuplicateSlug,
            }]
        );
        let body: Value = serde_json::from_slice(&catalog.body).expect("decode served body");
        assert_eq!(body["models"][0]["display_name"], json!("gpt-6-sol"));
    }

    #[test]
    fn drops_entries_without_a_slug() {
        let mut model = upstream_model("gpt-6-sol");
        model.as_object_mut().unwrap().remove("slug");

        let catalog = render_catalog(&[model]);
        assert_eq!(served_slugs(&catalog), Vec::<String>::new());
        assert_eq!(
            catalog.dropped.first().map(|entry| entry.reason),
            Some(DropReason::MissingSlug)
        );
    }

    #[test]
    fn trims_from_the_end_to_stay_under_the_client_size_cap() {
        let big = 400 * 1024;
        let models: Vec<Value> = (0..5)
            .map(|index| {
                let mut model = upstream_model(&format!("gpt-6-{index}"));
                model["model_messages"] = json!({"instructions_template": instructions(big)});
                model
            })
            .collect();

        let catalog = render_catalog(&models);
        assert!(
            catalog.body.len() <= CLIENT_CATALOG_BUDGET_BYTES,
            "served body must stay under the client cap: {} bytes",
            catalog.body.len()
        );
        assert!(catalog.kept > 0, "the catalog must keep what fits");
        // Trimming happens from the end, so what survives is the upstream prefix.
        assert_eq!(
            served_slugs(&catalog),
            (0..catalog.kept)
                .map(|index| format!("gpt-6-{index}"))
                .collect::<Vec<_>>()
        );
        assert_eq!(
            catalog
                .dropped
                .iter()
                .filter(|entry| entry.reason == DropReason::OverSizeBudget)
                .count(),
            5 - catalog.kept
        );
    }

    #[test]
    fn a_normal_catalog_is_far_under_the_client_cap() {
        // Measured shape: the account catalog is roughly 500 KB.
        let models: Vec<Value> = (0..8)
            .map(|index| {
                let mut model = upstream_model(&format!("gpt-6-{index}"));
                model["model_messages"] = json!({"instructions_template": instructions(20 * 1024)});
                model
            })
            .collect();

        let catalog = render_catalog(&models);
        assert_eq!(catalog.kept, 8);
        assert!(catalog.body.len() < CLIENT_CATALOG_BUDGET_BYTES);
    }

    #[test]
    fn missing_models_array_is_reported() {
        assert_eq!(
            models_from_body(&json!({})).unwrap_err(),
            CatalogError::MissingModels
        );
        assert_eq!(
            models_from_body(&json!({"models": []}))
                .expect("empty array is valid")
                .len(),
            0
        );
    }

    #[test]
    fn serving_the_upstream_value_keeps_the_account_context_window() {
        // The point of proxying: `gpt-6.1-sol` is 500_000 upstream against the
        // 272_000 the client's bundled catalog reports. TT invents no number.
        let catalog = render_catalog(&[upstream_model("gpt-6.1-sol")]);
        let body: Value = serde_json::from_slice(&catalog.body).expect("decode served body");
        assert_eq!(body["models"][0]["context_window"], json!(500_000));
        assert_eq!(
            body["models"][0]["model_messages"]["instructions_template"],
            json!(instructions(64))
        );
    }
}
