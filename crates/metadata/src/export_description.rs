//! The description of the packages and interfaces marked `#[export(rust)]`,
//! written as `veryl.types.json` into each component crate so that
//! `veryl_component::import!` expands them as Rust types.
//!
//! The reader is `veryl-component-macros`, which mirrors these types; a
//! change to the format bumps [`EXPORT_DESCRIPTION_VERSION`].

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// File name of the description, next to a component crate's `Cargo.toml`.
pub const EXPORT_DESCRIPTION_FILE: &str = "veryl.types.json";

/// Format version of the description.
pub const EXPORT_DESCRIPTION_VERSION: u32 = 1;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExportDescription {
    pub version: u32,
    /// Exported packages by name.
    pub packages: BTreeMap<String, ExportPackage>,
    /// Exported interfaces by name.
    pub interfaces: BTreeMap<String, ExportInterface>,
}

impl Default for ExportDescription {
    fn default() -> Self {
        Self {
            version: EXPORT_DESCRIPTION_VERSION,
            packages: BTreeMap::new(),
            interfaces: BTreeMap::new(),
        }
    }
}

impl ExportDescription {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_empty(&self) -> bool {
        self.packages.is_empty() && self.interfaces.is_empty()
    }

    /// The description as written to the file: pretty-printed with a
    /// trailing newline, so that an unchanged description is byte-identical.
    pub fn to_json(&self) -> String {
        let mut ret = serde_json::to_string_pretty(self).expect("serializable");
        ret.push('\n');
        ret
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExportPackage {
    pub consts: Vec<ExportConst>,
    pub types: Vec<ExportTypeAlias>,
    pub enums: Vec<ExportEnum>,
    pub structs: Vec<ExportStruct>,
}

/// A constant: its type and its value.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExportConst {
    pub name: String,
    pub r#type: ExportType,
    pub value: ExportValue,
}

/// The value of a constant, following its type.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExportValue {
    /// A vector of `width` bits: `value` is a decimal, negative for a
    /// negative value of a signed vector.
    Bits {
        width: u32,
        signed: bool,
        value: String,
    },
    /// The variant of an enum of an exported package.
    Variant {
        package: String,
        r#enum: String,
        variant: String,
    },
    /// The fields of a struct of an exported package, in their order.
    Struct {
        package: String,
        name: String,
        fields: Vec<ExportFieldValue>,
    },
    /// The elements of an array, the first the most significant.
    Array(Vec<ExportValue>),
    /// A string.
    String(String),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExportFieldValue {
    pub name: String,
    pub value: ExportValue,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExportTypeAlias {
    pub name: String,
    pub r#type: ExportType,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExportEnum {
    pub name: String,
    pub width: u32,
    pub variants: Vec<ExportVariant>,
}

/// An enum variant with its value as a decimal string.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExportVariant {
    pub name: String,
    pub value: String,
}

/// A packed struct; the first field is the most significant.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExportStruct {
    pub name: String,
    pub fields: Vec<ExportField>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExportField {
    pub name: String,
    pub r#type: ExportType,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExportInterface {
    pub modports: Vec<ExportModport>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExportModport {
    pub name: String,
    pub members: Vec<ExportMember>,
}

/// A variable member of a modport, with the direction the modport gives it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExportMember {
    pub name: String,
    pub direction: ExportDirection,
    pub r#type: ExportType,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExportDirection {
    Input,
    Output,
}

/// The type of a constant, a field or a member.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExportType {
    /// A vector of `width` bits.
    Bits { width: u32, signed: bool },
    /// A clock; a modport member only.
    Clock,
    /// A reset; a modport member only.
    Reset,
    /// A string; a constant only.
    String,
    /// An enum or a struct of an exported package.
    User { package: String, name: String },
    /// `len` elements of `elem`, the first element the most significant.
    Array { len: u32, elem: Box<ExportType> },
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `veryl-component-macros` reads the description with types of its own;
    /// the description its tests expand has to be one this writes.
    #[test]
    fn reader_fixture_is_written_alike() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../component/veryl.types.json");
        let text = std::fs::read_to_string(path).unwrap();
        let description: ExportDescription = serde_json::from_str(&text).unwrap();
        assert_eq!(description.version, EXPORT_DESCRIPTION_VERSION);
        let written: serde_json::Value = serde_json::from_str(&description.to_json()).unwrap();
        let fixture: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(written, fixture);
    }
}
