use serde::{Deserialize, Serialize};
use std::fmt;
use ucan::capability::{Ability, CapabilitySemantics, Scope};
use url::Url;

/// Hypha Resource Scope
/// Format: hypha://<node_id>/<resource_type>
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HyphaScope {
    pub origin_node: String,
    pub resource_type: String,
}

impl Scope for HyphaScope {
    fn contains(&self, other: &Self) -> bool {
        self.origin_node == other.origin_node && self.resource_type == other.resource_type
    }
}

impl fmt::Display for HyphaScope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "hypha://{}/{}", self.origin_node, self.resource_type)
    }
}

impl TryFrom<Url> for HyphaScope {
    type Error = anyhow::Error;

    fn try_from(value: Url) -> Result<Self, Self::Error> {
        if value.scheme() != "hypha" {
            return Err(anyhow::anyhow!("Invalid scheme"));
        }

        let origin = value
            .host_str()
            .ok_or_else(|| anyhow::anyhow!("Missing host"))?
            .to_string();
        let path = value.path().trim_start_matches('/').to_string();

        Ok(HyphaScope {
            origin_node: origin,
            resource_type: path,
        })
    }
}

/// Hypha Abilities
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum HyphaAbility {
    Execute, // For Compute
    Store,   // For Storage
    Sense,   // For Sensing
    Admin,   // Full control
}

impl Ability for HyphaAbility {}

impl fmt::Display for HyphaAbility {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            HyphaAbility::Execute => write!(f, "hypha/execute"),
            HyphaAbility::Store => write!(f, "hypha/store"),
            HyphaAbility::Sense => write!(f, "hypha/sense"),
            HyphaAbility::Admin => write!(f, "hypha/admin"),
        }
    }
}

impl TryFrom<String> for HyphaAbility {
    type Error = anyhow::Error;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        match value.as_str() {
            "hypha/execute" => Ok(HyphaAbility::Execute),
            "hypha/store" => Ok(HyphaAbility::Store),
            "hypha/sense" => Ok(HyphaAbility::Sense),
            "hypha/admin" => Ok(HyphaAbility::Admin),
            _ => Err(anyhow::anyhow!("Invalid ability: {}", value)),
        }
    }
}

pub struct HyphaSemantics;

impl CapabilitySemantics<HyphaScope, HyphaAbility> for HyphaSemantics {
    fn parse_scope(&self, uri: &Url) -> Option<HyphaScope> {
        HyphaScope::try_from(uri.clone()).ok()
    }

    fn parse_action(&self, ability: &str) -> Option<HyphaAbility> {
        HyphaAbility::try_from(ability.to_string()).ok()
    }
}
