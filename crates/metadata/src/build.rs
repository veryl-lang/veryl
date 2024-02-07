use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Build {
    #[serde(default)]
    pub clock_type: ClockType,
    #[serde(default)]
    pub reset_type: ResetType,
    #[serde(default)]
    pub filelist_type: FilelistType,
    #[serde(default)]
    pub target: Target,
    #[serde(default)]
    pub implicit_parameter_types: Vec<BuiltinType>,
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub enum ClockType {
    #[default]
    #[serde(rename = "posedge")]
    PosEdge,
    #[serde(rename = "negedge")]
    NegEdge,
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub enum ResetType {
    #[default]
    #[serde(rename = "async_low")]
    AsyncLow,
    #[serde(rename = "async_high")]
    AsyncHigh,
    #[serde(rename = "sync_low")]
    SyncLow,
    #[serde(rename = "sync_high")]
    SyncHigh,
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub enum FilelistType {
    #[default]
    #[serde(rename = "absolute")]
    Absolute,
    #[serde(rename = "relative")]
    Relative,
    #[serde(rename = "flgen")]
    Flgen,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type")]
pub enum Target {
    #[default]
    #[serde(rename = "source")]
    Source,
    #[serde(rename = "directory")]
    Directory { path: PathBuf },
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum BuiltinType {
    #[serde(rename = "u32")]
    U32,
    #[serde(rename = "u64")]
    U64,
    #[serde(rename = "i32")]
    I32,
    #[serde(rename = "i64")]
    I64,
    #[serde(rename = "f32")]
    F32,
    #[serde(rename = "f64")]
    F64,
    #[serde(rename = "string")]
    String,
    #[serde(rename = "type")]
    Type,
}
