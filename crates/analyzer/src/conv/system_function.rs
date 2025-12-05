use crate::conv::{Context, Conv};
use crate::ir::SystemFunctionKind;
use veryl_parser::resource_table::StrId;

impl Conv<StrId> for SystemFunctionKind {
    fn conv(_context: &mut Context, value: StrId) -> Self {
        let text = value.to_string();
        match text.as_str() {
            "$clog2" => SystemFunctionKind::Clog2,
            "$bits" => SystemFunctionKind::Bits,
            _ => SystemFunctionKind::Unsupported,
        }
    }
}
