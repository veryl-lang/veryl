use crate::AnalyzerError;
use crate::conv::Context;
use crate::ir::{Type, TypeKind, TypedValue, Value, ValueVariant};
use std::fmt;
use veryl_parser::token_range::TokenRange;

#[derive(Clone, Copy, Debug)]
pub enum SystemFunctionKind {
    Clog2,
    Bits,
    Unsupported,
}

impl SystemFunctionKind {
    pub fn eval(&self, _args: &[Option<Value>]) -> Option<Value> {
        // TODO
        None
    }

    pub fn eval_type(
        &self,
        context: &mut Context,
        args: &[TypedValue],
        token: &TokenRange,
    ) -> TypedValue {
        if let Some(x) = self.arity()
            && x != args.len()
        {
            context.insert_error(AnalyzerError::mismatch_function_arity(
                &self.to_string(),
                x,
                args.len(),
                token,
            ));
            return TypedValue::create_unknown();
        }

        let arg_types_error = self.arg_types(args);
        if !arg_types_error.is_empty() {
            for err in self.arg_types(args) {
                context.insert_error(AnalyzerError::mismatch_function_arg(
                    &self.to_string(),
                    &err,
                    token,
                ));
            }
            return TypedValue::create_unknown();
        }

        let return_type = self.return_type();
        let mut ret = TypedValue::from_type(return_type);

        ret.is_const = true;
        ret.is_global = args.iter().all(|x| x.is_global);

        // TODO
        match self {
            SystemFunctionKind::Clog2 => {
                ret.value = ValueVariant::Unknown;
            }
            SystemFunctionKind::Bits => {
                ret.value = ValueVariant::Unknown;
            }
            _ => (),
        }

        ret
    }

    fn arity(&self) -> Option<usize> {
        match self {
            SystemFunctionKind::Clog2 => Some(1),
            SystemFunctionKind::Bits => Some(1),
            SystemFunctionKind::Unsupported => None,
        }
    }

    fn arg_types(&self, args: &[TypedValue]) -> Vec<String> {
        let mut ret = vec![];
        match self {
            SystemFunctionKind::Clog2 => {
                let r#type = &args[0].r#type;
                if r#type.is_array() || r#type.is_type() {
                    ret.push(r#type.to_string());
                }
            }
            SystemFunctionKind::Bits => {
                let r#type = &args[0].r#type;
                if r#type.is_array() {
                    ret.push(r#type.to_string());
                }
            }
            SystemFunctionKind::Unsupported => (),
        }
        ret
    }

    fn return_type(&self) -> Type {
        match self {
            SystemFunctionKind::Clog2 => Type::new(TypeKind::Bit, vec![32], false),
            SystemFunctionKind::Bits => Type::new(TypeKind::Bit, vec![32], false),
            SystemFunctionKind::Unsupported => Type::new(TypeKind::Unknown, vec![], false),
        }
    }
}

impl fmt::Display for SystemFunctionKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SystemFunctionKind::Clog2 => "$clog2".fmt(f),
            SystemFunctionKind::Bits => "$bits".fmt(f),
            SystemFunctionKind::Unsupported => "$unsupported".fmt(f),
        }
    }
}
