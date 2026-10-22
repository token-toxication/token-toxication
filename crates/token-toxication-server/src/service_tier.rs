//! Bounded observations of upstream-reported service tiers, not scheduling guarantees.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa::ToSchema;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ReportedServiceTier {
    Auto,
    Default,
    Standard,
    Priority,
    Fast,
    Ultrafast,
    Flex,
    Scale,
    #[serde(other)]
    Unknown,
}

impl ReportedServiceTier {
    pub(crate) fn from_response(value: &Value) -> Option<Self> {
        let tier = value
            .pointer("/response/service_tier")
            .or_else(|| value.get("service_tier"))?;
        if tier.is_null() {
            return None;
        }
        Some(match tier.as_str() {
            Some("auto") => Self::Auto,
            Some("default") => Self::Default,
            Some("standard") => Self::Standard,
            Some("priority") => Self::Priority,
            Some("fast") => Self::Fast,
            Some("ultrafast") => Self::Ultrafast,
            Some("flex") => Self::Flex,
            Some("scale") => Self::Scale,
            _ => Self::Unknown,
        })
    }

    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Default => "default",
            Self::Standard => "standard",
            Self::Priority => "priority",
            Self::Fast => "fast",
            Self::Ultrafast => "ultrafast",
            Self::Flex => "flex",
            Self::Scale => "scale",
            Self::Unknown => "unknown",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn reported_tiers_distinguish_absent_default_and_unknown_without_retaining_values() {
        for value in [json!({}), json!({"service_tier":null})] {
            assert_eq!(ReportedServiceTier::from_response(&value), None);
        }
        for tier in [
            "auto",
            "default",
            "standard",
            "priority",
            "fast",
            "ultrafast",
            "flex",
            "scale",
        ] {
            for value in [
                json!({"service_tier":tier}),
                json!({"response":{"service_tier":tier}}),
            ] {
                let observed = ReportedServiceTier::from_response(&value).unwrap();
                assert_eq!(observed.as_str(), tier);
                assert_eq!(serde_json::to_value(observed).unwrap(), tier);
            }
        }
        for value in [
            json!("synthetic-private-tier"),
            json!(17),
            json!({"private":"value"}),
        ] {
            let observed = ReportedServiceTier::from_response(&json!({"service_tier":value}));
            assert_eq!(observed, Some(ReportedServiceTier::Unknown));
            assert_eq!(serde_json::to_value(observed).unwrap(), "unknown");
        }
        assert_eq!(
            ReportedServiceTier::from_response(
                &json!({"service_tier":"priority","response":{"service_tier":"default"}})
            ),
            Some(ReportedServiceTier::Default)
        );
    }
}
