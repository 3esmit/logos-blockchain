use core::{
    fmt::{self, Display, Formatter},
    str::FromStr,
    time::Duration,
};

use serde::{Deserialize, Serialize};

use crate::config::{
    OnUnknownKeys, blend::deployment::Settings as BlendDeploymentSettings,
    cryptarchia::deployment::Settings as CryptarchiaDeploymentSettings,
    deserialize_config_from_reader, mempool::deployment::Settings as MempoolDeploymentSettings,
    network::deployment::Settings as NetworkDeploymentSettings,
    time::deployment::Settings as TimeDeploymentSettings,
};

pub mod devnet;

#[derive(Serialize, Deserialize, Debug, Clone, Copy, Default)]
pub enum WellKnownDeployment {
    // Must match the `DEVNET` definition in the `devnet` module.
    #[serde(rename = "devnet")]
    #[default]
    Devnet,
}

impl FromStr for WellKnownDeployment {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            devnet::NAME => Ok(Self::Devnet),
            _ => Err(()),
        }
    }
}

impl Display for WellKnownDeployment {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Devnet => write!(f, "{}", devnet::NAME),
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct DeploymentSettings {
    pub blend: BlendDeploymentSettings,
    pub network: NetworkDeploymentSettings,
    pub cryptarchia: CryptarchiaDeploymentSettings,
    pub time: TimeDeploymentSettings,
    pub mempool: MempoolDeploymentSettings,
}

impl From<WellKnownDeployment> for DeploymentSettings {
    fn from(value: WellKnownDeployment) -> Self {
        match value {
            WellKnownDeployment::Devnet => deserialize_config_from_reader(
                devnet::SERIALIZED_DEPLOYMENT.as_bytes(),
                OnUnknownKeys::Fail,
            )
            .expect("Devnet deployment config is valid."),
        }
    }
}

impl DeploymentSettings {
    #[must_use]
    pub const fn blend_round_duration(&self) -> Duration {
        self.blend.round_duration(&self.time.slot_duration)
    }
}

#[cfg(test)]
mod tests {
    use crate::config::{DeploymentSettings, WellKnownDeployment};

    #[test]
    fn devnet_initialization() {
        drop(DeploymentSettings::from(WellKnownDeployment::Devnet));
    }
}
