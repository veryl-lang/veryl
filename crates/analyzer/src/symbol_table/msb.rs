use crate::AnalyzerError;
use crate::msb_table;
use crate::symbol::{Direction, SymbolKind, Type, TypeKind};
use crate::symbol_table::{self, Msb};

fn trace_type(r#type: &Type) -> Vec<(Type, Option<SymbolKind>)> {
    let mut ret = vec![(r#type.clone(), None)];
    if let TypeKind::UserDefined(ref x) = r#type.kind
        && let Some(id) = x.symbol
    {
        let symbol = symbol_table::get(id).unwrap();
        ret.last_mut().unwrap().1 = Some(symbol.kind.clone());
        if let SymbolKind::TypeDef(ref x) = symbol.kind {
            ret.append(&mut trace_type(&x.r#type));
        } else if let SymbolKind::ProtoTypeDef(ref x) = symbol.kind
            && let Some(ref r#type) = x.r#type
        {
            ret.append(&mut trace_type(r#type));
        }
    }
    ret
}

pub fn check_msb(list: Vec<Msb>) -> Vec<AnalyzerError> {
    let mut errors = Vec::new();

    for x in list {
        let token = x.token;
        let path = x.path;
        let mut select_dimension = x.dimension;

        let resolved = if let Ok(x) = symbol_table::resolve(&path) {
            let via_interface = x.full_path.iter().any(|path| {
                let symbol = symbol_table::get(*path).unwrap();
                match symbol.kind {
                    SymbolKind::Port(x) => {
                        matches!(x.direction, Direction::Interface | Direction::Modport)
                    }
                    SymbolKind::Instance(_) => true,
                    _ => false,
                }
            });
            let r#type = if !via_interface {
                match x.found.kind {
                    SymbolKind::Variable(x) => Some(x.r#type),
                    SymbolKind::Port(x) => Some(x.r#type),
                    SymbolKind::Parameter(x) => Some(x.r#type),
                    SymbolKind::StructMember(x) => Some(x.r#type),
                    SymbolKind::UnionMember(x) => Some(x.r#type),
                    _ => None,
                }
            } else {
                // msb through interface is forbidden
                // https://github.com/veryl-lang/veryl/pull/1154
                None
            };

            if let Some(x) = r#type {
                let types = trace_type(&x);

                let mut demension_number = None;
                for (i, (t, k)) in types.iter().enumerate() {
                    if select_dimension < t.array.len() {
                        demension_number = Some(select_dimension + 1);
                        break;
                    }
                    select_dimension -= t.array.len();

                    if select_dimension < t.width.len() {
                        demension_number = Some(select_dimension + 1);

                        break;
                    }
                    select_dimension -= t.width.len();

                    if select_dimension == 0
                        && (i + 1) == types.len()
                        && matches!(
                            k,
                            Some(SymbolKind::Enum(_))
                                | Some(SymbolKind::Struct(_))
                                | Some(SymbolKind::Union(_))
                        )
                    {
                        demension_number = Some(0);
                        break;
                    }
                }

                if let Some(demension_number) = demension_number {
                    msb_table::insert(token.id, demension_number);
                    true
                } else {
                    false
                }
            } else {
                false
            }
        } else {
            false
        };
        if !resolved {
            errors.push(AnalyzerError::unknown_msb(&token.into()));
        }
    }

    errors
}
