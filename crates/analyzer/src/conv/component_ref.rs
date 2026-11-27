use crate::analyzer_error::{AnalyzerError, ComponentInterfaceMismatchKind};
use crate::conv::Context;
use crate::conv::utils::var_path_to_assign_destination;
use crate::ir::{self, ComponentRefRole, VarIndex, VarPathSelect, VarSelect};
use veryl_metadata::ManifestRefKind;
use veryl_parser::resource_table::{self, StrId};
use veryl_parser::token_range::TokenRange;

/// Width of one element and packed dimensions (outermost first) of a
/// referenced variable's type: `[4, 8]` for `logic<4, 8>`, the struct's
/// width for a struct.
fn component_ref_shape(r#type: &ir::Type) -> (u32, Vec<u32>) {
    let width = r#type.total_width().unwrap_or(0) as u32;
    let mut dims: Vec<u32> = r#type
        .width()
        .iter()
        .filter_map(|x| x.map(|x| x as u32))
        .collect();
    if let Some(base) = r#type.kind.width()
        && base > 1
    {
        dims.push(base as u32);
    }
    if dims.is_empty() || dims.iter().product::<u32>() != width {
        dims = vec![width.max(1)];
    }
    (width, dims)
}

/// Converts a reference argument of a component method call: a variable
/// for `input`/`output`/`clock`/`reset`, a modport for `modport`. The
/// reference must name the variables statically, so the analyzer and the
/// simulator know which variables the component may read or drive.
pub fn conv_component_ref(
    context: &mut Context,
    component: StrId,
    method: &str,
    kind: ManifestRefKind,
    index: usize,
    arg: &(ir::Expression, Vec<VarPathSelect>, TokenRange),
) -> Option<ir::ComponentRef> {
    let (expr, dst, token) = arg;
    let token = *token;
    let reject = |context: &mut Context, reason: &str| {
        context.insert_error(AnalyzerError::component_interface_mismatch(
            ComponentInterfaceMismatchKind::InvalidReference {
                method: method.to_string(),
                position: index + 1,
                reason: reason.to_string(),
            },
            Some(component),
            &token,
        ));
        None
    };
    let comptime = expr.comptime().clone();

    if kind == ManifestRefKind::Modport {
        return conv_modport_ref(context, expr, dst, token, reject);
    }

    match &comptime.r#type.kind {
        ir::TypeKind::Modport(_, _) => {
            return reject(context, "pass a variable, not a modport");
        }
        ir::TypeKind::Instance(_, _) => {
            return reject(context, "pass a variable, not an instance");
        }
        _ => {}
    }
    if comptime.r#type.is_array() {
        return reject(context, "unpacked arrays cannot be referenced yet");
    }
    if kind == ManifestRefKind::Clock && !comptime.r#type.is_clock() {
        return reject(context, "pass a clock");
    }
    if kind == ManifestRefKind::Reset && !comptime.r#type.is_reset() {
        return reject(context, "pass a reset");
    }
    let plain = |index: &VarIndex, select: &VarSelect| {
        index.expressions().next().is_none() && select.0.is_empty() && select.1.is_none()
    };
    let ir::Expression::Term(factor) = expr else {
        return reject(
            context,
            "pass a variable, without an expression or a select",
        );
    };
    let role = match factor.as_ref() {
        ir::Factor::Variable(var_id, index, select, _) if plain(index, select) => match kind {
            ManifestRefKind::Clock => ComponentRefRole::Clock(*var_id),
            ManifestRefKind::Reset => ComponentRefRole::Reset(*var_id),
            ManifestRefKind::Output => {
                let Some(dst) = var_path_to_assign_destination(context, dst.clone(), true)
                    .into_iter()
                    .next()
                else {
                    return reject(context, "pass a variable the component can drive");
                };
                ComponentRefRole::Output(Box::new(dst))
            }
            _ => ComponentRefRole::Input,
        },
        ir::Factor::HierVariable(x) if plain(&x.index, &x.select) => match kind {
            ManifestRefKind::Output => {
                return reject(
                    context,
                    "a variable inside a child instance can only be read",
                );
            }
            ManifestRefKind::Clock => {
                return reject(
                    context,
                    "a clock inside a child instance cannot be passed yet",
                );
            }
            ManifestRefKind::Reset => {
                return reject(
                    context,
                    "a reset inside a child instance cannot be passed yet",
                );
            }
            _ => ComponentRefRole::Input,
        },
        _ => {
            return reject(
                context,
                "pass a variable, without an expression or a select",
            );
        }
    };
    let (width, dims) = component_ref_shape(&comptime.r#type);
    let path = match &dst[..] {
        [dst] => dst.0.to_string(),
        _ => expr.to_string(),
    };
    Some(ir::ComponentRef {
        modport: None,
        members: vec![ir::ComponentRefMember {
            member: None,
            expr: expr.clone(),
            role,
            width,
            dims,
            path,
            token,
        }],
    })
}

fn conv_modport_ref(
    context: &mut Context,
    expr: &ir::Expression,
    dst: &[VarPathSelect],
    token: TokenRange,
    reject: impl Fn(&mut Context, &str) -> Option<ir::ComponentRef>,
) -> Option<ir::ComponentRef> {
    let comptime = expr.comptime().clone();
    let ir::TypeKind::Modport(sig, modport) = &comptime.r#type.kind else {
        return reject(context, "pass a modport");
    };
    let [dst] = dst else {
        return reject(context, "pass a modport of an interface instance");
    };
    if !dst.1.0.is_empty() || dst.1.1.is_some() {
        return reject(
            context,
            "pass a modport of an interface instance, without a select",
        );
    }
    // The reference is written `<instance>.<modport>`; members live under
    // the instance.
    let mut path = dst.0.clone();
    if path.0.last() == Some(modport) {
        path.0.pop();
    }
    let mut members = vec![];
    for (member_path, direction) in comptime.r#type.expand_modport(context, &path, token).ok()? {
        if !direction.is_input() && !direction.is_output() {
            continue;
        }
        let Some((var_id, mut member_comptime)) = context.find_path(&member_path) else {
            context.insert_error(AnalyzerError::undefined_identifier(
                &member_path.to_string(),
                &token,
            ));
            continue;
        };
        if member_comptime.r#type.is_array() {
            return reject(context, "unpacked arrays cannot be referenced yet");
        }
        member_comptime.token = token;
        let member: Vec<String> = member_path.0[path.0.len()..]
            .iter()
            .map(|x| resource_table::canonical_str_id(*x).to_string())
            .collect();
        // An input member of a clock or reset type fires the component's
        // `on_clock` or `on_reset`, as a clock or reset passed on its own
        // does.
        let role = if direction.is_output() {
            let dst = vec![VarPathSelect(
                member_path.clone(),
                VarSelect::default(),
                token,
            )];
            match var_path_to_assign_destination(context, dst, true)
                .into_iter()
                .next()
            {
                Some(dst) => ComponentRefRole::Output(Box::new(dst)),
                None => ComponentRefRole::Input,
            }
        } else if member_comptime.r#type.is_clock() {
            ComponentRefRole::Clock(var_id)
        } else if member_comptime.r#type.is_reset() {
            ComponentRefRole::Reset(var_id)
        } else {
            ComponentRefRole::Input
        };
        let (width, dims) = component_ref_shape(&member_comptime.r#type);
        members.push(ir::ComponentRefMember {
            member: Some(resource_table::insert_str(&member.join("."))),
            expr: ir::Expression::Term(Box::new(ir::Factor::Variable(
                var_id,
                VarIndex::default(),
                VarSelect::default(),
                member_comptime,
            ))),
            role,
            width,
            dims,
            path: member_path.to_string(),
            token,
        });
    }
    Some(ir::ComponentRef {
        modport: Some((sig.to_string(), *modport)),
        members,
    })
}
