//! Client-facing model id aliases.
//!
//! Fast is a **service tier** on a model, not a model of its own: the catalog
//! declares `service_tiers: [{"id": "priority", "name": "Fast"}]` on the base
//! entry, and no upstream serves a `-fast` slug. Some clients still request one
//! (opencode derives its own `-fast` entries and maps them back to the base id
//! before sending, but a client that does not map them sends the suffix
//! unchanged).
//!
//! Route selection therefore resolves a `-fast` request to the base model that
//! owns the route. The requested id is still what request logs record, so Fast
//! traffic stays distinguishable from ordinary traffic on the same route.

/// Suffix a client may append to ask for the Fast service tier.
pub const FAST_SUFFIX: &str = "-fast";

/// Strip a trailing `-fast` so route selection uses the base model.
pub fn base_model_id(api_model_id: &str) -> &str {
    api_model_id
        .strip_suffix(FAST_SUFFIX)
        .unwrap_or(api_model_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_only_a_trailing_fast_suffix() {
        assert_eq!(base_model_id("gpt-6.1-sol-fast"), "gpt-6.1-sol");
        assert_eq!(base_model_id("gpt-6-astra-fast"), "gpt-6-astra");
        assert_eq!(base_model_id("gpt-6.1-sol"), "gpt-6.1-sol");
        // Case-sensitive: only the exact suffix is an alias.
        assert_eq!(base_model_id("gpt-6.1-sol-Fast"), "gpt-6.1-sol-Fast");
        // Not a prefix rule.
        assert_eq!(base_model_id("fast-gpt-6-sol"), "fast-gpt-6-sol");
    }
}
