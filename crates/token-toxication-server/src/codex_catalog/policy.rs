//! Eligibility policy for Codex subscription models.
//!
//! TT supports a narrower model set than OpenCode's legacy Codex OAuth path.
//! OpenCode enables every `gpt-5.<minor > 4>` model plus two exact exceptions;
//! TT supports only the GPT-6 family:
//!
//! ```text
//! supported:     gpt-6-*, gpt-6.<minor <= 1>[*], and their `-fast` aliases
//! rejected:      gpt-5.x and older, gpt-6.2 and newer, gpt-7+
//! ```
//!
//! This matches the GPT-6 entries the Codex backend reports for a subscription
//! account (`gpt-6-astra`, `gpt-6-sol`, `gpt-6-luna`, `gpt-6.1-sol`) without
//! silently enabling a version TT has not qualified.
//!
//! # No limits here
//!
//! This module deliberately carries **no** context or input limits. Model
//! metadata is taken from the account's own upstream catalog, so there is a
//! single source of truth and nothing to keep in sync. Earlier revisions
//! hard-coded OpenCode's `628_000`/`500_000` and a generic `400_000`/`272_000`;
//! those were removed on purpose. Do not reintroduce them — an operator override
//! belongs in a stored model profile, not in a constant.
//!
//! The module is pure: no I/O, no credentials, no state.

/// Highest supported GPT major version.
const SUPPORTED_MAJOR: u32 = 6;
/// Highest supported minor version within [`SUPPORTED_MAJOR`].
const SUPPORTED_MAX_MINOR: u32 = 1;

/// Reasoning mode that disables a model regardless of its identifier.
const PRO_REASONING_MODE: &str = "pro";

/// Suffix stripped before evaluation so fast aliases share their base model's policy.
const FAST_SUFFIX: &str = "-fast";

/// The catalog policy decision for one candidate model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Eligible,
    Ineligible(IneligibleReason),
}

/// Reason a candidate model is hidden from the Codex subscription catalog.
///
/// Kept small and stable so it can be used as a metric label and logged without
/// leaking user input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IneligibleReason {
    /// The model requested the unsupported `pro` reasoning mode.
    ReasoningModePro,
    /// The model is older than the supported GPT-6 floor.
    NotCodexCapable,
    /// The model is newer than this build has qualified.
    UnsupportedVersion,
    /// The identifier does not match the `gpt-<major>` shape at all.
    UnrecognizedFormat,
}

impl IneligibleReason {
    /// Bounded code for metrics and structured logs.
    pub fn code(self) -> &'static str {
        match self {
            Self::ReasoningModePro => "reasoning_mode_pro",
            Self::NotCodexCapable => "not_codex_capable",
            Self::UnsupportedVersion => "unsupported_version",
            Self::UnrecognizedFormat => "unrecognized",
        }
    }
}

/// Decide whether a Codex subscription connection should expose `api_model_id`.
///
/// `api_model_id` is the identifier sent to the upstream, matching OpenCode's
/// `modelID ?? id`. `reasoning_mode` is the requested reasoning mode, if any.
pub fn decide(api_model_id: &str, reasoning_mode: Option<&str>) -> Decision {
    if reasoning_mode == Some(PRO_REASONING_MODE) {
        return Decision::Ineligible(IneligibleReason::ReasoningModePro);
    }

    let canonical = canonical_model_id(api_model_id.trim());

    match gpt_version(canonical) {
        Some((SUPPORTED_MAJOR, minor)) if minor <= SUPPORTED_MAX_MINOR => Decision::Eligible,
        Some((major, _)) if major < SUPPORTED_MAJOR => {
            Decision::Ineligible(IneligibleReason::NotCodexCapable)
        }
        Some(_) => Decision::Ineligible(IneligibleReason::UnsupportedVersion),
        None => Decision::Ineligible(IneligibleReason::UnrecognizedFormat),
    }
}

/// Strip a trailing `-fast` so an alias is judged as its base model.
///
/// `-fast` is a valid variant syntax, not a distinct model family: eligibility
/// is decided from the base id. The public model id stays whatever clients send.
pub fn canonical_model_id(api_model_id: &str) -> &str {
    api_model_id
        .strip_suffix(FAST_SUFFIX)
        .unwrap_or(api_model_id)
}

/// Parse the leading `gpt-<major>[.<minor>]` prefix.
///
/// Anything after the version is ignored, matching OpenCode's
/// `^gpt-(\d+)(?:\.(\d+))?` regex. Identifiers with an overflowing or missing
/// numeric component are treated as unrecognized.
fn gpt_version(api_model_id: &str) -> Option<(u32, u32)> {
    let rest = api_model_id.strip_prefix("gpt-")?;
    let bytes = rest.as_bytes();

    let mut index = 0;
    while index < bytes.len() && bytes[index].is_ascii_digit() {
        index += 1;
    }
    if index == 0 {
        return None;
    }
    let major: u32 = rest[..index].parse().ok()?;

    let mut minor = 0;
    if bytes.get(index) == Some(&b'.') {
        let start = index + 1;
        let mut end = start;
        while end < bytes.len() && bytes[end].is_ascii_digit() {
            end += 1;
        }
        if end > start {
            minor = rest[start..end].parse().ok()?;
        }
    }

    Some((major, minor))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn eligible(api_model_id: &str) -> bool {
        decide(api_model_id, None) == Decision::Eligible
    }

    fn ineligible(api_model_id: &str) -> IneligibleReason {
        match decide(api_model_id, None) {
            Decision::Eligible => panic!("expected {api_model_id} to be hidden"),
            Decision::Ineligible(reason) => reason,
        }
    }

    #[test]
    fn every_supported_gpt_6_model_is_eligible() {
        for id in [
            "gpt-6",
            "gpt-6-astra",
            "gpt-6-sol",
            "gpt-6-luna",
            "gpt-6.1-sol",
            "gpt-6-astra-fast",
            "gpt-6-sol-fast",
            "gpt-6-luna-fast",
            "gpt-6.1-sol-fast",
        ] {
            assert!(eligible(id), "expected {id} to be eligible");
        }
    }

    #[test]
    fn fast_alias_is_judged_as_its_base_model() {
        assert_eq!(canonical_model_id("gpt-6.1-sol-fast"), "gpt-6.1-sol");
        assert_eq!(canonical_model_id("gpt-6.1-sol"), "gpt-6.1-sol");
        // The alias must not widen the window: the base model is still rejected.
        assert_eq!(
            ineligible("gpt-6.2-fast"),
            IneligibleReason::UnsupportedVersion
        );
        assert_eq!(
            ineligible("gpt-5.6-fast"),
            IneligibleReason::NotCodexCapable
        );
    }

    #[test]
    fn gpt_5_and_older_models_are_rejected() {
        for id in [
            "gpt-5",
            "gpt-5.4",
            "gpt-5.5",
            "gpt-5.5-pro",
            "gpt-5.6",
            "gpt-5.6-sol",
            "gpt-5.6-terra",
            "gpt-5.6-luna",
            "gpt-5.3-codex-spark",
            "gpt-5.10",
            "gpt-5.04-astra",
            "gpt-5.x",
            "gpt-4.99",
            "gpt-4.1",
        ] {
            assert_eq!(
                ineligible(id),
                IneligibleReason::NotCodexCapable,
                "unexpected decision for {id}"
            );
        }
    }

    #[test]
    fn gpt_6_2_and_newer_models_are_rejected_as_unqualified() {
        for id in ["gpt-6.2", "gpt-6.2-sol", "gpt-6.10", "gpt-7", "gpt-7.1-sol"] {
            assert_eq!(
                ineligible(id),
                IneligibleReason::UnsupportedVersion,
                "unexpected decision for {id}"
            );
        }
    }

    #[test]
    fn identifiers_without_a_gpt_version_are_unrecognized() {
        for id in ["", "codex", "gpt", "gpt-", "gpt-x", " openai/gpt-6.1-sol"] {
            assert_eq!(
                ineligible(id),
                IneligibleReason::UnrecognizedFormat,
                "unexpected decision for {id:?}"
            );
        }
    }

    #[test]
    fn pro_reasoning_mode_disables_an_otherwise_eligible_model() {
        assert_eq!(
            decide("gpt-6-astra", Some("pro")),
            Decision::Ineligible(IneligibleReason::ReasoningModePro)
        );
        assert_eq!(
            decide("gpt-6.1-sol", Some("pro")),
            Decision::Ineligible(IneligibleReason::ReasoningModePro)
        );
    }

    #[test]
    fn non_pro_reasoning_mode_keeps_normal_eligibility() {
        assert_eq!(decide("gpt-6.1-sol", Some("high")), Decision::Eligible);
        assert_eq!(decide("gpt-6.1-sol", None), Decision::Eligible);
    }

    #[test]
    fn version_prefix_ignores_trailing_text() {
        assert!(eligible("gpt-6-astra-preview-2027"));
        assert!(eligible("gpt-6.1-sol-2027"));
    }

    #[test]
    fn ineligible_reason_codes_are_bounded_and_stable() {
        assert_eq!(
            IneligibleReason::ReasoningModePro.code(),
            "reasoning_mode_pro"
        );
        assert_eq!(
            IneligibleReason::NotCodexCapable.code(),
            "not_codex_capable"
        );
        assert_eq!(
            IneligibleReason::UnsupportedVersion.code(),
            "unsupported_version"
        );
        assert_eq!(IneligibleReason::UnrecognizedFormat.code(), "unrecognized");
    }
}
