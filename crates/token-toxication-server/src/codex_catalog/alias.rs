//! Client-facing model id aliases.
//!
//! Fast and Ultrafast are service tiers on a model, not models of their own.
//! Some harnesses only understand model ids, so TT accepts suffixed ids and
//! translates them back to the base route plus the native service tier.

/// Suffix a client may append to ask for the Fast service tier.
pub const FAST_SUFFIX: &str = "-fast";

/// Suffix a client may append to ask for the Ultrafast service tier.
pub const ULTRAFAST_SUFFIX: &str = "-ultrafast";

/// Strip a trailing speed-tier suffix so route selection uses the base model.
pub fn base_model_id(api_model_id: &str) -> &str {
    api_model_id
        .strip_suffix(ULTRAFAST_SUFFIX)
        .or_else(|| api_model_id.strip_suffix(FAST_SUFFIX))
        .unwrap_or(api_model_id)
}

/// Return the native OpenAI service tier represented by a client-facing alias.
pub fn service_tier_for_model_id(api_model_id: &str) -> Option<&'static str> {
    if api_model_id.ends_with(ULTRAFAST_SUFFIX) {
        Some("ultrafast")
    } else if api_model_id.ends_with(FAST_SUFFIX) {
        Some("priority")
    } else {
        None
    }
}

/// Apply an alias tier only when route selection resolved the alias to its base.
pub fn apply_service_tier_alias(
    value: &mut serde_json::Value,
    requested_model: &str,
    routed_model: &str,
) {
    if requested_model == routed_model || value.get("service_tier").is_some() {
        return;
    }
    let Some(service_tier) = service_tier_for_model_id(requested_model) else {
        return;
    };
    value["service_tier"] = serde_json::Value::String(service_tier.to_string());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_speed_tier_suffixes() {
        assert_eq!(base_model_id("gpt-6.1-sol-fast"), "gpt-6.1-sol");
        assert_eq!(base_model_id("gpt-6.1-sol-ultrafast"), "gpt-6.1-sol");
        assert_eq!(base_model_id("gpt-6.1-sol"), "gpt-6.1-sol");
        assert_eq!(base_model_id("gpt-6.1-sol-Fast"), "gpt-6.1-sol-Fast");
        assert_eq!(base_model_id("fast-gpt-6-sol"), "fast-gpt-6-sol");
    }

    #[test]
    fn maps_speed_aliases_to_native_tiers() {
        assert_eq!(
            service_tier_for_model_id("gpt-6.1-sol-fast"),
            Some("priority")
        );
        assert_eq!(
            service_tier_for_model_id("gpt-6.1-sol-ultrafast"),
            Some("ultrafast")
        );
        assert_eq!(service_tier_for_model_id("gpt-6.1-sol"), None);
    }

    #[test]
    fn preserves_an_explicit_tier() {
        let mut value = serde_json::json!({"service_tier": "priority"});
        apply_service_tier_alias(&mut value, "gpt-6.1-sol-ultrafast", "gpt-6.1-sol");
        assert_eq!(value["service_tier"], "priority");
    }
}
