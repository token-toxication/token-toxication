//! Upstream Codex subscription catalog handling.
//!
//! The relay serves a Codex-native catalog by **proxying the account's own
//! upstream catalog** rather than inventing model metadata. Upstream entries
//! already carry the instruction templates Codex requires, so TT never authors,
//! vendors, or redistributes a system prompt.
//!
//! Two properties of the Codex client drive the rules here:
//!
//! 1. A catalog supplied through `model_catalog_url` **replaces** the bundled
//!    catalog; it is not merged field-by-field.
//! 2. `ModelsResponse` deserializes with
//!    `deserialize_model_infos_with_legacy_base`, which **hard-errors** when an
//!    entry has neither `model_messages.instructions_template` nor the legacy
//!    top-level `base_instructions`. A model without instructions is not a
//!    degraded model; it breaks the whole catalog.
//!
//! Verified against `codex-cli 0.162.1`: an entry with instructions omitted
//! fails with `model \`…\` is missing both \`base_instructions\` and
//! \`model_messages.instructions_template\``.
//!
//! Entries are carried as opaque [`Value`]s so every field Codex needs survives
//! the round trip. This module is pure: no I/O, and no logging of model bodies.

use serde_json::Value;

use crate::codex_catalog::policy;

/// A selected model keeps its upstream slug alongside the untouched payload.
#[derive(Debug, Clone, PartialEq)]
pub struct SelectedModel {
    pub slug: String,
    pub value: Value,
}

/// Why an upstream catalog could not be served.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CatalogError {
    #[error("upstream catalog is missing the `models` array")]
    MissingModels,
    #[error("upstream model at index {index} has no usable slug")]
    MissingSlug { index: usize },
    #[error(
        "upstream model `{slug}` is missing both `base_instructions` and `model_messages.instructions_template`"
    )]
    MissingInstructions { slug: String },
    #[error("upstream catalog contains duplicate slug `{slug}`")]
    DuplicateSlug { slug: String },
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

fn is_listed(model: &Value) -> bool {
    model
        .get("visibility")
        .and_then(Value::as_str)
        .is_none_or(|visibility| visibility == "list")
}

fn is_supported_in_api(model: &Value) -> bool {
    model
        .get("supported_in_api")
        .and_then(Value::as_bool)
        .unwrap_or(true)
}

/// Select the upstream models TT is willing to serve.
///
/// Models are dropped when the eligibility policy rejects them, when upstream
/// marks them hidden, or when they are not API-supported. Dropping is safe: the
/// Codex client resolves unknown slugs through its own local fallback metadata.
///
/// The result fails closed on the properties that would break the whole
/// catalog: a duplicate slug or an entry without instructions.
pub fn select_models(models: &[Value]) -> Result<Vec<SelectedModel>, CatalogError> {
    let mut selected: Vec<SelectedModel> = Vec::new();

    for (index, model) in models.iter().enumerate() {
        let Some(slug) = slug_of(model) else {
            return Err(CatalogError::MissingSlug { index });
        };

        if !is_listed(model) || !is_supported_in_api(model) {
            continue;
        }

        if !matches!(policy::decide(slug, None), policy::Decision::Eligible) {
            continue;
        }

        // Only check instructions for entries we intend to serve, so an unrelated
        // hidden model cannot take down the catalog.
        if !has_instructions(model) {
            return Err(CatalogError::MissingInstructions {
                slug: slug.to_string(),
            });
        }

        if selected.iter().any(|existing| existing.slug == slug) {
            return Err(CatalogError::DuplicateSlug {
                slug: slug.to_string(),
            });
        }

        selected.push(SelectedModel {
            slug: slug.to_string(),
            value: model.clone(),
        });
    }

    Ok(selected)
}

/// Suffix TT appends when deriving a `-fast` variant.
const FAST_SUFFIX: &str = "-fast";

/// Service tier id the Codex client uses for the Fast speed tier.
///
/// Taken from the catalog Codex itself ships for these models:
/// `service_tiers: [{"id": "priority", "name": "Fast", "description": "2x speed,
/// increased usage"}]`. The client sends this value on the wire as
/// `service_tier`. TT writes it as the entry's `default_service_tier`, so a
/// `-fast` model actually selects Fast instead of being a duplicate.
const FAST_SERVICE_TIER: &str = "priority";

/// Add a `-fast` clone for every selected model that does not already have one.
///
/// The upstream catalog has no `-fast` slugs, so TT derives them. Each clone
/// reuses the base entry's instruction template and every other field, changing
/// only `slug`, `display_name`, and `default_service_tier`. That keeps upstream
/// as the single source of truth: nothing but the variant identity and the
/// speed tier declared by the shipped catalog is added.
///
/// Bases that already end in `-fast`, and slugs upstream already provides, are
/// left alone so the catalog never contains a duplicate.
pub fn expand_fast_variants(models: Vec<SelectedModel>) -> Vec<SelectedModel> {
    let existing: std::collections::HashSet<String> =
        models.iter().map(|model| model.slug.clone()).collect();
    let mut expanded = Vec::with_capacity(models.len() * 2);

    for model in models {
        let fast = if model.slug.ends_with(FAST_SUFFIX) {
            None
        } else {
            let slug = format!("{}{FAST_SUFFIX}", model.slug);
            if existing.contains(&slug) {
                None
            } else {
                fast_variant(&model, slug)
            }
        };
        expanded.push(model);
        if let Some(fast) = fast {
            expanded.push(fast);
        }
    }

    expanded
}

fn fast_variant(base: &SelectedModel, slug: String) -> Option<SelectedModel> {
    let mut value = base.value.clone();
    let object = value.as_object_mut()?;
    let display_name = object
        .get("display_name")
        .and_then(Value::as_str)
        .filter(|name| !name.trim().is_empty())
        .map(|name| format!("{name} Fast"))
        .unwrap_or_else(|| slug.clone());
    object.insert("slug".to_string(), Value::from(slug.clone()));
    object.insert("display_name".to_string(), Value::from(display_name));
    object.insert(
        "default_service_tier".to_string(),
        Value::from(FAST_SERVICE_TIER),
    );
    Some(SelectedModel { slug, value })
}

/// Apply an explicit context override to one selected model.
///
/// `effective_context_window_percent` is written alongside `context_window`
/// because the Codex client computes the usable budget as
/// `context_window * effective_context_window_percent / 100`. Omitting it would
/// silently apply the client's 95% default.
pub fn apply_context_override(
    model: &mut Value,
    context_window: i64,
    effective_context_window_percent: u8,
) -> Result<(), &'static str> {
    if context_window <= 0 {
        return Err("context window must be positive");
    }
    if effective_context_window_percent > 100 {
        return Err("effective context window percent must be between 0 and 100");
    }
    let Some(object) = model.as_object_mut() else {
        return Err("model entry must be a JSON object");
    };
    object.insert("context_window".to_string(), Value::from(context_window));
    object.insert(
        "effective_context_window_percent".to_string(),
        Value::from(effective_context_window_percent),
    );
    Ok(())
}

/// Usable context the Codex client will derive from a catalog entry.
///
/// Mirrors `ModelInfo::usable_context_window`: `context_window` wins over
/// `max_context_window`, then the effective percentage is applied.
pub fn usable_context_window(model: &Value) -> Option<i64> {
    let context = model
        .get("context_window")
        .and_then(Value::as_i64)
        .or_else(|| model.get("max_context_window").and_then(Value::as_i64))?;
    let percent = model
        .get("effective_context_window_percent")
        .and_then(Value::as_i64)
        .unwrap_or(95);
    Some(context.saturating_mul(percent) / 100)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

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
            "model_messages": {"instructions_template": instructions(64)},
        })
    }

    fn slugs(models: &[SelectedModel]) -> Vec<&str> {
        models.iter().map(|model| model.slug.as_str()).collect()
    }

    #[test]
    fn keeps_supported_gpt_6_models_only() {
        let models = vec![
            upstream_model("gpt-6.1-sol"),
            upstream_model("gpt-6-astra"),
            upstream_model("gpt-6-sol"),
            upstream_model("gpt-6-luna"),
            upstream_model("gpt-5.6-sol"),
            upstream_model("gpt-5.6-terra"),
            upstream_model("gpt-5.6-luna"),
            upstream_model("gpt-5.5"),
        ];
        let selected = select_models(&models).expect("select models");
        assert_eq!(
            slugs(&selected),
            vec!["gpt-6.1-sol", "gpt-6-astra", "gpt-6-sol", "gpt-6-luna"]
        );
    }

    #[test]
    fn drops_hidden_and_non_api_models_without_inspecting_instructions() {
        let mut hidden = upstream_model("gpt-reserve");
        hidden["visibility"] = json!("hide");
        hidden.as_object_mut().unwrap().remove("model_messages");

        let mut not_in_api = upstream_model("codex-auto-review");
        not_in_api["supported_in_api"] = json!(false);
        not_in_api.as_object_mut().unwrap().remove("model_messages");

        // Neither entry has instructions; both must be skipped, not fail the catalog.
        let selected = select_models(&[hidden, not_in_api]).expect("select models");
        assert!(selected.is_empty());
    }

    #[test]
    fn fails_closed_when_a_served_model_lacks_instructions() {
        let mut model = upstream_model("gpt-6.1-sol");
        model.as_object_mut().unwrap().remove("model_messages");

        assert_eq!(
            select_models(&[model]),
            Err(CatalogError::MissingInstructions {
                slug: "gpt-6.1-sol".to_string()
            })
        );
    }

    #[test]
    fn accepts_the_legacy_base_instructions_field() {
        let mut model = upstream_model("gpt-6-astra");
        model.as_object_mut().unwrap().remove("model_messages");
        model["base_instructions"] = json!(instructions(32));

        let selected = select_models(&[model]).expect("select models");
        assert_eq!(slugs(&selected), vec!["gpt-6-astra"]);
    }

    #[test]
    fn rejects_blank_instruction_templates() {
        let mut model = upstream_model("gpt-6-sol");
        model["model_messages"] = json!({"instructions_template": "   "});

        assert!(matches!(
            select_models(&[model]),
            Err(CatalogError::MissingInstructions { .. })
        ));
    }

    #[test]
    fn rejects_duplicate_slugs() {
        let models = vec![upstream_model("gpt-6-sol"), upstream_model("gpt-6-sol")];
        assert_eq!(
            select_models(&models),
            Err(CatalogError::DuplicateSlug {
                slug: "gpt-6-sol".to_string()
            })
        );
    }

    #[test]
    fn rejects_entries_without_a_slug() {
        let mut model = upstream_model("gpt-6-sol");
        model.as_object_mut().unwrap().remove("slug");
        assert_eq!(
            select_models(&[model]),
            Err(CatalogError::MissingSlug { index: 0 })
        );
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
    fn override_sets_context_and_effective_percent() {
        let mut model = upstream_model("gpt-6.1-sol");
        apply_context_override(&mut model, 628_000, 100).expect("apply override");

        assert_eq!(model["context_window"], json!(628_000));
        assert_eq!(model["effective_context_window_percent"], json!(100));
        assert_eq!(usable_context_window(&model), Some(628_000));
    }

    #[test]
    fn override_rejects_invalid_values() {
        let mut model = upstream_model("gpt-6-sol");
        assert_eq!(
            apply_context_override(&mut model, 0, 100),
            Err("context window must be positive")
        );
        assert_eq!(
            apply_context_override(&mut model, 1_000, 101),
            Err("effective context window percent must be between 0 and 100")
        );
    }

    #[test]
    fn pass_through_uses_upstream_context_and_client_default_percent() {
        // Deliberate outcome: TT serves the account's own metadata unchanged.
        // 500_000 is the upstream value for gpt-6.1-sol; the client applies its
        // own 95% headroom, yielding 475_000 usable. TT invents no number here,
        // and the old bundled-catalog path reported only 272_000.
        let model = upstream_model("gpt-6.1-sol");
        assert_eq!(usable_context_window(&model), Some(475_000));
    }

    #[test]
    fn usable_window_prefers_context_window_over_max() {
        let mut model = upstream_model("gpt-6-sol");
        model["effective_context_window_percent"] = json!(100);
        assert_eq!(usable_context_window(&model), Some(500_000));

        model.as_object_mut().unwrap().remove("context_window");
        assert_eq!(usable_context_window(&model), Some(872_000));
    }

    #[test]
    fn fast_variants_are_cloned_with_the_fast_service_tier() {
        let models = select_models(&[upstream_model("gpt-6.1-sol"), upstream_model("gpt-6-astra")])
            .expect("select models");

        let expanded = expand_fast_variants(models);
        assert_eq!(
            slugs(&expanded),
            vec![
                "gpt-6.1-sol",
                "gpt-6.1-sol-fast",
                "gpt-6-astra",
                "gpt-6-astra-fast",
            ]
        );

        let fast = expanded
            .iter()
            .find(|model| model.slug == "gpt-6.1-sol-fast")
            .expect("fast variant exists");
        assert_eq!(fast.value["default_service_tier"], json!("priority"));
        assert_eq!(fast.value["display_name"], json!("gpt-6.1-sol Fast"));
        // Everything else, including instructions and context, is carried over.
        assert_eq!(fast.value["context_window"], json!(500_000));
        assert!(has_instructions(&fast.value));
        assert_eq!(
            fast.value["model_messages"]["instructions_template"],
            upstream_model("gpt-6.1-sol")["model_messages"]["instructions_template"]
        );
        assert_eq!(usable_context_window(&fast.value), Some(475_000));
    }

    #[test]
    fn fast_expansion_does_not_duplicate_a_slug_upstream_already_provides() {
        let models = select_models(&[
            upstream_model("gpt-6-sol"),
            upstream_model("gpt-6-sol-fast"),
        ])
        .expect("select models");

        let expanded = expand_fast_variants(models);
        assert_eq!(
            slugs(&expanded),
            vec!["gpt-6-sol", "gpt-6-sol-fast"],
            "an upstream-provided fast slug must not be cloned again"
        );
    }

    #[test]
    fn fast_expansion_leaves_existing_fast_bases_alone() {
        let models = select_models(&[upstream_model("gpt-6-luna-fast")]).expect("select models");
        let expanded = expand_fast_variants(models);
        assert_eq!(slugs(&expanded), vec!["gpt-6-luna-fast"]);
    }
}
