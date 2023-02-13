use crate::MetadataError;
use semver::Version;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::str::FromStr;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Pubdata {
    pub releases: Vec<Release>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Release {
    pub version: Version,
    pub revision: String,
}

impl Pubdata {
    pub fn load<T: AsRef<Path>>(path: T) -> Result<Self, MetadataError> {
        let path = path.as_ref().canonicalize()?;
        let text = std::fs::read_to_string(path)?;
        let pubadata: Pubdata = Self::from_str(&text)?;

        Ok(pubadata)
    }
}

impl FromStr for Pubdata {
    type Err = MetadataError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let pubdata: Pubdata = toml::from_str(s)?;
        Ok(pubdata)
    }
}
