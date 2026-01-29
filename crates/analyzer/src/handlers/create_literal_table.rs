use crate::analyzer_error::AnalyzerError;
use crate::literal::Literal;
use crate::literal_table;
use crate::value::Value;
use paste::paste;
use veryl_parser::ParolError;
use veryl_parser::veryl_grammar_trait::*;
use veryl_parser::veryl_walker::{Handler, HandlerPoint};

const BINARY_CHARS: [char; 6] = ['0', '1', 'x', 'z', 'X', 'Z'];
const OCTAL_CHARS: [char; 12] = ['0', '1', '2', '3', '4', '5', '6', '7', 'x', 'z', 'X', 'Z'];
const DECIMAL_CHARS: [char; 10] = ['0', '1', '2', '3', '4', '5', '6', '7', '8', '9'];

#[derive(Default)]
pub struct CreateLiteralTable {
    pub errors: Vec<AnalyzerError>,
    point: HandlerPoint,
}

impl CreateLiteralTable {
    pub fn new() -> Self {
        Self::default()
    }
}

impl Handler for CreateLiteralTable {
    fn set_point(&mut self, p: HandlerPoint) {
        self.point = p;
    }
}

macro_rules! impl_type_literal {
    ($x:ident) => {
        paste! {
            fn [<$x:snake>](&mut self, arg: &$x) -> Result<(), ParolError> {
                if let HandlerPoint::Before = self.point {
                    let id = arg.[<$x:snake _token>].token.id;
                    let literal = arg.into();
                    literal_table::insert(id, literal);
                }
                Ok(())
            }
        }
    };
}

impl VerylGrammarTrait for CreateLiteralTable {
    impl_type_literal!(Bit);
    impl_type_literal!(BBool);
    impl_type_literal!(LBool);
    impl_type_literal!(Clock);
    impl_type_literal!(ClockPosedge);
    impl_type_literal!(ClockNegedge);
    impl_type_literal!(F32);
    impl_type_literal!(F64);
    impl_type_literal!(I8);
    impl_type_literal!(I16);
    impl_type_literal!(I32);
    impl_type_literal!(I64);
    impl_type_literal!(Logic);
    impl_type_literal!(Reset);
    impl_type_literal!(ResetAsyncHigh);
    impl_type_literal!(ResetAsyncLow);
    impl_type_literal!(ResetSyncHigh);
    impl_type_literal!(ResetSyncLow);
    impl_type_literal!(U8);
    impl_type_literal!(U16);
    impl_type_literal!(U32);
    impl_type_literal!(U64);

    fn strin(&mut self, arg: &Strin) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            let id = arg.string_token.token.id;
            let literal = arg.into();
            literal_table::insert(id, literal);
        }
        Ok(())
    }

    fn based(&mut self, arg: &Based) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            let token = &arg.based_token.token;
            let id = token.id;
            let value: Value = arg.into();

            let actual_width = value
                .payload
                .bits()
                .max(value.mask_x.bits())
                .max(value.mask_z.bits()) as usize;
            if actual_width > value.width {
                self.errors
                    .push(AnalyzerError::too_large_number(value.width, &token.into()));
            }

            let literal = Literal::Value(value);
            literal_table::insert(id, literal);

            let text = token.to_string();
            let (_, tail) = text.split_once('\'').unwrap();
            let signed = &tail[0..1] == "s";
            let base = if signed { &tail[1..2] } else { &tail[0..1] };
            let number = if signed { &tail[2..] } else { &tail[1..] };
            let number = number.replace('_', "");
            let number = number.trim_start_matches('0');

            match base {
                "b" => {
                    if let Some(x) = number.chars().find(|x| !BINARY_CHARS.contains(x)) {
                        self.errors.push(AnalyzerError::invalid_number_character(
                            x,
                            "binary",
                            &token.into(),
                        ));
                    }
                }
                "o" => {
                    if let Some(x) = number.chars().find(|x| !OCTAL_CHARS.contains(x)) {
                        self.errors.push(AnalyzerError::invalid_number_character(
                            x,
                            "octal",
                            &token.into(),
                        ));
                    }
                }
                "d" => {
                    if let Some(x) = number.chars().find(|x| !DECIMAL_CHARS.contains(x)) {
                        self.errors.push(AnalyzerError::invalid_number_character(
                            x,
                            "decimal",
                            &token.into(),
                        ));
                    }
                }
                "h" => (),
                _ => unreachable!(),
            }
        }

        Ok(())
    }

    fn base_less(&mut self, arg: &BaseLess) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            let id = arg.base_less_token.token.id;
            let value: Value = arg.into();
            let literal = Literal::Value(value);
            literal_table::insert(id, literal);
        }
        Ok(())
    }

    fn all_bit(&mut self, arg: &AllBit) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            let id = arg.all_bit_token.token.id;
            let value: Value = arg.into();
            let literal = Literal::Value(value);
            literal_table::insert(id, literal);
        }
        Ok(())
    }

    fn fixed_point(&mut self, arg: &FixedPoint) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            let id = arg.fixed_point_token.token.id;
            let value: Value = arg.into();
            let literal = Literal::Value(value);
            literal_table::insert(id, literal);
        }
        Ok(())
    }

    fn exponent(&mut self, arg: &Exponent) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            let id = arg.exponent_token.token.id;
            let value: Value = arg.into();
            let literal = Literal::Value(value);
            literal_table::insert(id, literal);
        }
        Ok(())
    }

    fn r#true(&mut self, arg: &True) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            let id = arg.true_token.token.id;
            let literal = Literal::Boolean(true);
            literal_table::insert(id, literal);
        }
        Ok(())
    }

    fn r#false(&mut self, arg: &False) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            let id = arg.false_token.token.id;
            let literal = Literal::Boolean(false);
            literal_table::insert(id, literal);
        }
        Ok(())
    }

    fn string_literal(&mut self, arg: &StringLiteral) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            let id = arg.string_literal_token.token.id;
            let text = arg.string_literal_token.token.text;
            let literal = Literal::String(text);
            literal_table::insert(id, literal);
        }
        Ok(())
    }
}
