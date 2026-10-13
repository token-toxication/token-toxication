use serde_json::{Value, json};

use crate::{db::RoutableModelCatalogEntry, models::CodexModelCatalogResponse};

const FAST_SERVICE_TIER: &str = "priority";

/// Build the Codex-native catalog for the currently routable Responses models.
///
/// Codex uses this metadata to decide which models and service-tier controls to
/// expose. The relay only advertises Fast for model families known to support
/// the `priority` request value; the request path still forwards the value to
/// the selected provider unchanged.
pub fn catalog_response(models: Vec<RoutableModelCatalogEntry>) -> CodexModelCatalogResponse {
    CodexModelCatalogResponse {
        models: models
            .into_iter()
            .map(|model| model_metadata(&model))
            .collect(),
    }
}

fn model_metadata(model: &RoutableModelCatalogEntry) -> Value {
    let supports_fast = supports_fast_mode(&model.id);
    let display_name = if model.display_name.trim().is_empty() {
        &model.id
    } else {
        &model.display_name
    };
    json!({
        "slug": model.id,
        "display_name": display_name,
        "description": format!("{} model routed by Token Toxication.", model.family),
        "base_instructions": "",
        "default_reasoning_level": "low",
        "supported_reasoning_levels": supported_reasoning_levels(&model.id),
        "shell_type": "shell_command",
        "visibility": "list",
        "supported_in_api": true,
        "priority": 0,
        "additional_speed_tiers": if supports_fast { json!(["fast"]) } else { json!([]) },
        "service_tiers": if supports_fast {
            json!([{
                "id": FAST_SERVICE_TIER,
                "name": "Fast",
                "description": "2x speed, increased usage"
            }])
        } else {
            json!([])
        },
        "default_service_tier": null,
        "availability_nux": null,
        "upgrade": null,
        "support_verbosity": true,
        "default_verbosity": "low",
        "truncation_policy": {"mode": "tokens", "limit": 10000},
        "experimental_supported_tools": [],
        "default_reasoning_summary": "none",
        "supports_reasoning_summary_parameter": true,
        "context_window": 272000,
        "max_context_window": 872000,
        "effective_context_window_percent": 90,
        "supports_search_tool": true,
        "supports_experimental_context": false,
        "use_responses_lite": false,
        "supports_reasoning_effort_updates": true,
        "node_repl_auto_review_required": false,
        "node_repl_disabled": false,
        "include_skills_usage_instructions": false,
        "include_plugin_usage_instructions": false,
        "include_apps_usage_instructions": false,
        "input_modalities": ["text", "image"]
    })
}

fn supports_fast_mode(model_id: &str) -> bool {
    matches!(
        model_id,
        "gpt-6-astra" | "gpt-6.1-sol" | "gpt-6-sol" | "gpt-6-luna"
    )
}

fn supported_reasoning_levels(model_id: &str) -> Value {
    let mut levels = vec![
        json!({
            "effort": "low",
            "description": "Fast responses with lighter reasoning"
        }),
        json!({
            "effort": "medium",
            "description": "Balances speed and reasoning depth for everyday tasks"
        }),
        json!({
            "effort": "high",
            "description": "Greater reasoning depth for complex problems"
        }),
        json!({
            "effort": "xhigh",
            "description": "Extra high reasoning depth for complex problems"
        }),
    ];

    if supports_max_reasoning(model_id) {
        levels.push(json!({
            "effort": "max",
            "description": "Maximum reasoning depth for the hardest problems"
        }));
    }
    if supports_ultra_reasoning(model_id) {
        levels.push(json!({
            "effort": "ultra",
            "description": "Maximum reasoning with automatic task delegation"
        }));
    }

    Value::Array(levels)
}

fn supports_max_reasoning(model_id: &str) -> bool {
    matches!(
        model_id,
        "gpt-6-astra" | "gpt-6.1-sol" | "gpt-6-sol" | "gpt-6-luna"
    )
}

fn supports_ultra_reasoning(model_id: &str) -> bool {
    matches!(model_id, "gpt-6-astra" | "gpt-6.1-sol" | "gpt-6-sol")
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{DateTime, Utc};

    fn model(id: &str) -> RoutableModelCatalogEntry {
        RoutableModelCatalogEntry {
            id: id.to_string(),
            display_name: id.to_string(),
            family: "responses".to_string(),
            provider: "synthetic".to_string(),
            wire_api: "openai-responses".to_string(),
            created_at: DateTime::<Utc>::UNIX_EPOCH,
        }
    }

    #[test]
    fn advertises_fast_for_supported_models() {
        let response = catalog_response(vec![model("gpt-6.1-sol")]);
        assert_eq!(response.models[0]["base_instructions"], "");
        assert_eq!(
            response.models[0]["supported_reasoning_levels"]
                .as_array()
                .expect("reasoning levels")
                .iter()
                .map(|level| level["effort"].as_str().expect("effort"))
                .collect::<Vec<_>>(),
            vec!["low", "medium", "high", "xhigh", "max", "ultra"]
        );
        assert_eq!(
            response.models[0]["service_tiers"][0]["id"],
            FAST_SERVICE_TIER
        );
        assert_eq!(response.models[0]["additional_speed_tiers"][0], "fast");
    }

    #[test]
    fn does_not_advertise_fast_for_unknown_models() {
        let response = catalog_response(vec![model("custom-model")]);
        assert_eq!(
            response.models[0]["supported_reasoning_levels"]
                .as_array()
                .expect("reasoning levels")
                .iter()
                .map(|level| level["effort"].as_str().expect("effort"))
                .collect::<Vec<_>>(),
            vec!["low", "medium", "high", "xhigh"]
        );
        assert_eq!(response.models[0]["service_tiers"], json!([]));
        assert_eq!(response.models[0]["additional_speed_tiers"], json!([]));
    }

    #[test]
    fn luna_stops_reasoning_levels_at_max() {
        let response = catalog_response(vec![model("gpt-6-luna")]);
        assert_eq!(
            response.models[0]["supported_reasoning_levels"]
                .as_array()
                .expect("reasoning levels")
                .iter()
                .map(|level| level["effort"].as_str().expect("effort"))
                .collect::<Vec<_>>(),
            vec!["low", "medium", "high", "xhigh", "max"]
        );
    }

    #[test]
    fn gpt6_reasoning_levels_match_codex_catalog() {
        let response = catalog_response(vec![
            model("gpt-6-astra"),
            model("gpt-6.1-sol"),
            model("gpt-6-sol"),
            model("gpt-6-luna"),
        ]);
        let efforts = |index: usize| {
            response.models[index]["supported_reasoning_levels"]
                .as_array()
                .expect("reasoning levels")
                .iter()
                .map(|level| level["effort"].as_str().expect("effort"))
                .collect::<Vec<_>>()
        };

        assert_eq!(
            efforts(0),
            vec!["low", "medium", "high", "xhigh", "max", "ultra"]
        );
        assert_eq!(
            efforts(1),
            vec!["low", "medium", "high", "xhigh", "max", "ultra"]
        );
        assert_eq!(
            efforts(2),
            vec!["low", "medium", "high", "xhigh", "max", "ultra"]
        );
        assert_eq!(efforts(3), vec!["low", "medium", "high", "xhigh", "max"]);
    }
}
