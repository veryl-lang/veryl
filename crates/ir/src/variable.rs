use num_bigint::BigUint;
use num_traits::FromPrimitive;
use std::fmt;
use veryl_parser::resource_table::StrId;

#[derive(Clone, Copy, Eq, PartialEq, Hash)]
pub struct VarId(u32);

pub struct VarPath(Vec<(StrId, Vec<u32>)>);

impl fmt::Display for VarPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut ret = String::new();

        for (id, index) in &self.0 {
            ret.push('.');
            ret.push_str(&format!("{id}"));
            for i in index {
                ret.push_str(&format!("[{i}]"));
            }
        }

        ret[1..].fmt(f)
    }
}

#[derive(Clone)]
pub struct Value {
    pub payload: BigUint,
    pub mask_x: BigUint,
    pub mask_z: BigUint,
    pub signed: bool,
    pub width: usize,
}

impl From<u32> for Value {
    fn from(value: u32) -> Self {
        Value {
            payload: BigUint::from_u32(value).unwrap(),
            mask_x: 0u32.into(),
            mask_z: 0u32.into(),
            signed: false,
            width: 32,
        }
    }
}

impl fmt::LowerHex for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        use std::fmt::Display;
        let ret = format!("{:x}", self.payload);
        ret.fmt(f)
    }
}

pub enum VarKind {
    Param,
    Input,
    Output,
    Variable,
}

pub struct Variable {
    pub id: VarId,
    pub path: VarPath,
    pub kind: VarKind,
    pub value: Value,
}

impl fmt::Display for Variable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut ret = String::new();

        ret.push_str(&format!("var {}: ", self.path));
        if self.value.signed {
            ret.push_str("signed logic");
        } else {
            ret.push_str("logic");
        }
        if self.value.width != 1 {
            ret.push_str(&format!("<{}>", self.value.width));
        }

        ret.push_str(&format!(" = 0x{:x};", self.value));
        ret.fmt(f)
    }
}
