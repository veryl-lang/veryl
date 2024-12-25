use crate::veryl_grammar_trait::*;
use crate::veryl_token::VerylToken;

macro_rules! before {
    ($x:ident, $y:ident, $z:ident) => {
        if let Some(mut handlers) = $x.get_handlers() {
            for handler in handlers.iter_mut() {
                handler.set_point(HandlerPoint::Before);
                let _ = handler.$y($z);
            }
        }
    };
}

macro_rules! after {
    ($x:ident, $y:ident, $z:ident) => {
        if let Some(mut handlers) = $x.get_handlers() {
            for handler in handlers.iter_mut() {
                handler.set_point(HandlerPoint::After);
                let _ = handler.$y($z);
            }
        }
    };
}

pub trait VerylWalker {
    /// Semantic action for non-terminal 'VerylToken'
    fn veryl_token(&mut self, _arg: &VerylToken) {}

    /// Semantic action for non-terminal 'Start'
    fn start(&mut self, arg: &Start) {
        before!(self, start, arg);
        self.veryl_token(&arg.start_token);
        after!(self, start, arg);
    }

    /// Semantic action for non-terminal 'StringLiteral'
    fn string_literal(&mut self, arg: &StringLiteral) {
        before!(self, string_literal, arg);
        self.veryl_token(&arg.string_literal_token);
        after!(self, string_literal, arg);
    }

    /// Semantic action for non-terminal 'Exponent'
    fn exponent(&mut self, arg: &Exponent) {
        before!(self, exponent, arg);
        self.veryl_token(&arg.exponent_token);
        after!(self, exponent, arg);
    }

    /// Semantic action for non-terminal 'FixedPoint'
    fn fixed_point(&mut self, arg: &FixedPoint) {
        before!(self, fixed_point, arg);
        self.veryl_token(&arg.fixed_point_token);
        after!(self, fixed_point, arg);
    }

    /// Semantic action for non-terminal 'Based'
    fn based(&mut self, arg: &Based) {
        before!(self, based, arg);
        self.veryl_token(&arg.based_token);
        after!(self, based, arg);
    }

    /// Semantic action for non-terminal 'BaseLess'
    fn base_less(&mut self, arg: &BaseLess) {
        before!(self, base_less, arg);
        self.veryl_token(&arg.base_less_token);
        after!(self, base_less, arg);
    }

    /// Semantic action for non-terminal 'AllBit'
    fn all_bit(&mut self, arg: &AllBit) {
        before!(self, all_bit, arg);
        self.veryl_token(&arg.all_bit_token);
        after!(self, all_bit, arg);
    }

    /// Semantic action for non-terminal 'AssignmentOperator'
    fn assignment_operator(&mut self, arg: &AssignmentOperator) {
        before!(self, assignment_operator, arg);
        self.veryl_token(&arg.assignment_operator_token);
        after!(self, assignment_operator, arg);
    }

    /// Semantic action for non-terminal 'Operator01'
    fn operator01(&mut self, arg: &Operator01) {
        before!(self, operator01, arg);
        self.veryl_token(&arg.operator01_token);
        after!(self, operator01, arg);
    }

    /// Semantic action for non-terminal 'Operator02'
    fn operator02(&mut self, arg: &Operator02) {
        before!(self, operator02, arg);
        self.veryl_token(&arg.operator02_token);
        after!(self, operator02, arg);
    }

    /// Semantic action for non-terminal 'Operator03'
    fn operator03(&mut self, arg: &Operator03) {
        before!(self, operator03, arg);
        self.veryl_token(&arg.operator03_token);
        after!(self, operator03, arg);
    }

    /// Semantic action for non-terminal 'Operator04'
    fn operator04(&mut self, arg: &Operator04) {
        before!(self, operator04, arg);
        self.veryl_token(&arg.operator04_token);
        after!(self, operator04, arg);
    }

    /// Semantic action for non-terminal 'Operator05'
    fn operator05(&mut self, arg: &Operator05) {
        before!(self, operator05, arg);
        self.veryl_token(&arg.operator05_token);
        after!(self, operator05, arg);
    }

    /// Semantic action for non-terminal 'Operator06'
    fn operator06(&mut self, arg: &Operator06) {
        before!(self, operator06, arg);
        self.veryl_token(&arg.operator06_token);
        after!(self, operator06, arg);
    }

    /// Semantic action for non-terminal 'Operator07'
    fn operator07(&mut self, arg: &Operator07) {
        before!(self, operator07, arg);
        self.veryl_token(&arg.operator07_token);
        after!(self, operator07, arg);
    }

    /// Semantic action for non-terminal 'Operator08'
    fn operator08(&mut self, arg: &Operator08) {
        before!(self, operator08, arg);
        self.veryl_token(&arg.operator08_token);
        after!(self, operator08, arg);
    }

    /// Semantic action for non-terminal 'Operator09'
    fn operator09(&mut self, arg: &Operator09) {
        before!(self, operator09, arg);
        self.veryl_token(&arg.operator09_token);
        after!(self, operator09, arg);
    }

    /// Semantic action for non-terminal 'Operator10'
    fn operator10(&mut self, arg: &Operator10) {
        before!(self, operator10, arg);
        self.veryl_token(&arg.operator10_token);
        after!(self, operator10, arg);
    }

    /// Semantic action for non-terminal 'Operator11'
    fn operator11(&mut self, arg: &Operator11) {
        before!(self, operator11, arg);
        self.veryl_token(&arg.operator11_token);
        after!(self, operator11, arg);
    }

    /// Semantic action for non-terminal 'UnaryOperator'
    fn unary_operator(&mut self, arg: &UnaryOperator) {
        before!(self, unary_operator, arg);
        self.veryl_token(&arg.unary_operator_token);
        after!(self, unary_operator, arg);
    }

    /// Semantic action for non-terminal 'BackQuote'
    fn back_quote(&mut self, arg: &BackQuote) {
        before!(self, back_quote, arg);
        self.veryl_token(&arg.back_quote_token);
        after!(self, back_quote, arg);
    }

    /// Semantic action for non-terminal 'Colon'
    fn colon(&mut self, arg: &Colon) {
        before!(self, colon, arg);
        self.veryl_token(&arg.colon_token);
        after!(self, colon, arg);
    }

    /// Semantic action for non-terminal 'ColonColon'
    fn colon_colon(&mut self, arg: &ColonColon) {
        before!(self, colon_colon, arg);
        self.veryl_token(&arg.colon_colon_token);
        after!(self, colon_colon, arg);
    }

    /// Semantic action for non-terminal 'ColonColonLAngle'
    fn colon_colon_l_angle(&mut self, arg: &ColonColonLAngle) {
        before!(self, colon_colon_l_angle, arg);
        self.veryl_token(&arg.colon_colon_l_angle_token);
        after!(self, colon_colon_l_angle, arg);
    }

    /// Semantic action for non-terminal 'Comma'
    fn comma(&mut self, arg: &Comma) {
        before!(self, comma, arg);
        self.veryl_token(&arg.comma_token);
        after!(self, comma, arg);
    }

    /// Semantic action for non-terminal 'DotDot'
    fn dot_dot(&mut self, arg: &DotDot) {
        before!(self, dot_dot, arg);
        self.veryl_token(&arg.dot_dot_token);
        after!(self, dot_dot, arg);
    }

    /// Semantic action for non-terminal 'DotDotEqu'
    fn dot_dot_equ(&mut self, arg: &DotDotEqu) {
        before!(self, dot_dot_equ, arg);
        self.veryl_token(&arg.dot_dot_equ_token);
        after!(self, dot_dot_equ, arg);
    }

    /// Semantic action for non-terminal 'Dot'
    fn dot(&mut self, arg: &Dot) {
        before!(self, dot, arg);
        self.veryl_token(&arg.dot_token);
        after!(self, dot, arg);
    }

    /// Semantic action for non-terminal 'Equ'
    fn equ(&mut self, arg: &Equ) {
        before!(self, equ, arg);
        self.veryl_token(&arg.equ_token);
        after!(self, equ, arg);
    }

    /// Semantic action for non-terminal 'Hash'
    fn hash(&mut self, arg: &Hash) {
        before!(self, hash, arg);
        self.veryl_token(&arg.hash_token);
        after!(self, hash, arg);
    }

    /// Semantic action for non-terminal 'QuoteLBrace'
    fn quote_l_brace(&mut self, arg: &QuoteLBrace) {
        before!(self, quote_l_brace, arg);
        self.veryl_token(&arg.quote_l_brace_token);
        after!(self, quote_l_brace, arg);
    }

    /// Semantic action for non-terminal 'LAngle'
    fn l_angle(&mut self, arg: &LAngle) {
        before!(self, l_angle, arg);
        self.veryl_token(&arg.l_angle_token);
        after!(self, l_angle, arg);
    }

    /// Semantic action for non-terminal 'LBrace'
    fn l_brace(&mut self, arg: &LBrace) {
        before!(self, l_brace, arg);
        self.veryl_token(&arg.l_brace_token);
        after!(self, l_brace, arg);
    }

    /// Semantic action for non-terminal 'LBracket'
    fn l_bracket(&mut self, arg: &LBracket) {
        before!(self, l_bracket, arg);
        self.veryl_token(&arg.l_bracket_token);
        after!(self, l_bracket, arg);
    }

    /// Semantic action for non-terminal 'LParen'
    fn l_paren(&mut self, arg: &LParen) {
        before!(self, l_paren, arg);
        self.veryl_token(&arg.l_paren_token);
        after!(self, l_paren, arg);
    }

    /// Semantic action for non-terminal 'MinusColon'
    fn minus_colon(&mut self, arg: &MinusColon) {
        before!(self, minus_colon, arg);
        self.veryl_token(&arg.minus_colon_token);
        after!(self, minus_colon, arg);
    }

    /// Semantic action for non-terminal 'MinusGT'
    fn minus_g_t(&mut self, arg: &MinusGT) {
        before!(self, minus_g_t, arg);
        self.veryl_token(&arg.minus_g_t_token);
        after!(self, minus_g_t, arg);
    }

    /// Semantic action for non-terminal 'PlusColon'
    fn plus_colon(&mut self, arg: &PlusColon) {
        before!(self, plus_colon, arg);
        self.veryl_token(&arg.plus_colon_token);
        after!(self, plus_colon, arg);
    }

    /// Semantic action for non-terminal 'RAngle'
    fn r_angle(&mut self, arg: &RAngle) {
        before!(self, r_angle, arg);
        self.veryl_token(&arg.r_angle_token);
        after!(self, r_angle, arg);
    }

    /// Semantic action for non-terminal 'RBrace'
    fn r_brace(&mut self, arg: &RBrace) {
        before!(self, r_brace, arg);
        self.veryl_token(&arg.r_brace_token);
        after!(self, r_brace, arg);
    }

    /// Semantic action for non-terminal 'RBracket'
    fn r_bracket(&mut self, arg: &RBracket) {
        before!(self, r_bracket, arg);
        self.veryl_token(&arg.r_bracket_token);
        after!(self, r_bracket, arg);
    }

    /// Semantic action for non-terminal 'RParen'
    fn r_paren(&mut self, arg: &RParen) {
        before!(self, r_paren, arg);
        self.veryl_token(&arg.r_paren_token);
        after!(self, r_paren, arg);
    }

    /// Semantic action for non-terminal 'Semicolon'
    fn semicolon(&mut self, arg: &Semicolon) {
        before!(self, semicolon, arg);
        self.veryl_token(&arg.semicolon_token);
        after!(self, semicolon, arg);
    }

    /// Semantic action for non-terminal 'Star'
    fn star(&mut self, arg: &Star) {
        before!(self, star, arg);
        self.veryl_token(&arg.star_token);
        after!(self, star, arg);
    }

    /// Semantic action for non-terminal 'AlwaysComb'
    fn always_comb(&mut self, arg: &AlwaysComb) {
        before!(self, always_comb, arg);
        self.veryl_token(&arg.always_comb_token);
        after!(self, always_comb, arg);
    }

    /// Semantic action for non-terminal 'AlwaysFf'
    fn always_ff(&mut self, arg: &AlwaysFf) {
        before!(self, always_ff, arg);
        self.veryl_token(&arg.always_ff_token);
        after!(self, always_ff, arg);
    }

    /// Semantic action for non-terminal 'As'
    fn r#as(&mut self, arg: &As) {
        before!(self, r#as, arg);
        self.veryl_token(&arg.as_token);
        after!(self, r#as, arg);
    }

    /// Semantic action for non-terminal 'Assign'
    fn assign(&mut self, arg: &Assign) {
        before!(self, assign, arg);
        self.veryl_token(&arg.assign_token);
        after!(self, assign, arg);
    }

    /// Semantic action for non-terminal 'Bit'
    fn bit(&mut self, arg: &Bit) {
        before!(self, bit, arg);
        self.veryl_token(&arg.bit_token);
        after!(self, bit, arg);
    }

    /// Semantic action for non-terminal 'Case'
    fn case(&mut self, arg: &Case) {
        before!(self, case, arg);
        self.veryl_token(&arg.case_token);
        after!(self, case, arg);
    }

    /// Semantic action for non-terminal 'Clock'
    fn clock(&mut self, arg: &Clock) {
        before!(self, clock, arg);
        self.veryl_token(&arg.clock_token);
        after!(self, clock, arg);
    }

    /// Semantic action for non-terminal 'ClockPosedge'
    fn clock_posedge(&mut self, arg: &ClockPosedge) {
        before!(self, clock_posedge, arg);
        self.veryl_token(&arg.clock_posedge_token);
        after!(self, clock_posedge, arg);
    }

    /// Semantic action for non-terminal 'ClockNegedge'
    fn clock_negedge(&mut self, arg: &ClockNegedge) {
        before!(self, clock_negedge, arg);
        self.veryl_token(&arg.clock_negedge_token);
        after!(self, clock_negedge, arg);
    }

    /// Semantic action for non-terminal 'Const'
    fn r#const(&mut self, arg: &Const) {
        before!(self, r#const, arg);
        self.veryl_token(&arg.const_token);
        after!(self, r#const, arg);
    }

    /// Semantic action for non-terminal 'Defaul'
    fn defaul(&mut self, arg: &Defaul) {
        before!(self, defaul, arg);
        self.veryl_token(&arg.default_token);
        after!(self, defaul, arg);
    }

    /// Semantic action for non-terminal 'Else'
    fn r#else(&mut self, arg: &Else) {
        before!(self, r#else, arg);
        self.veryl_token(&arg.else_token);
        after!(self, r#else, arg);
    }

    /// Semantic action for non-terminal 'Embed'
    fn embed(&mut self, arg: &Embed) {
        before!(self, embed, arg);
        self.veryl_token(&arg.embed_token);
        after!(self, embed, arg);
    }

    /// Semantic action for non-terminal 'Enum'
    fn r#enum(&mut self, arg: &Enum) {
        before!(self, r#enum, arg);
        self.veryl_token(&arg.enum_token);
        after!(self, r#enum, arg);
    }

    /// Semantic action for non-terminal 'Export'
    fn export(&mut self, arg: &Export) {
        before!(self, export, arg);
        self.veryl_token(&arg.export_token);
        after!(self, export, arg);
    }

    /// Semantic action for non-terminal 'F32'
    fn f32(&mut self, arg: &F32) {
        before!(self, f32, arg);
        self.veryl_token(&arg.f32_token);
        after!(self, f32, arg);
    }

    /// Semantic action for non-terminal 'F64'
    fn f64(&mut self, arg: &F64) {
        before!(self, f64, arg);
        self.veryl_token(&arg.f64_token);
        after!(self, f64, arg);
    }

    /// Semantic action for non-terminal 'Final'
    fn r#final(&mut self, arg: &Final) {
        before!(self, r#final, arg);
        self.veryl_token(&arg.final_token);
        after!(self, r#final, arg);
    }

    /// Semantic action for non-terminal 'For'
    fn r#for(&mut self, arg: &For) {
        before!(self, r#for, arg);
        self.veryl_token(&arg.for_token);
        after!(self, r#for, arg);
    }

    /// Semantic action for non-terminal 'Function'
    fn function(&mut self, arg: &Function) {
        before!(self, function, arg);
        self.veryl_token(&arg.function_token);
        after!(self, function, arg);
    }

    /// Semantic action for non-terminal 'I32'
    fn i32(&mut self, arg: &I32) {
        before!(self, i32, arg);
        self.veryl_token(&arg.i32_token);
        after!(self, i32, arg);
    }

    /// Semantic action for non-terminal 'I64'
    fn i64(&mut self, arg: &I64) {
        before!(self, i64, arg);
        self.veryl_token(&arg.i64_token);
        after!(self, i64, arg);
    }

    /// Semantic action for non-terminal 'If'
    fn r#if(&mut self, arg: &If) {
        before!(self, r#if, arg);
        self.veryl_token(&arg.if_token);
        after!(self, r#if, arg);
    }

    /// Semantic action for non-terminal 'IfReset'
    fn if_reset(&mut self, arg: &IfReset) {
        before!(self, if_reset, arg);
        self.veryl_token(&arg.if_reset_token);
        after!(self, if_reset, arg);
    }

    /// Semantic action for non-terminal 'Import'
    fn import(&mut self, arg: &Import) {
        before!(self, import, arg);
        self.veryl_token(&arg.import_token);
        after!(self, import, arg);
    }

    /// Semantic action for non-terminal 'In'
    fn r#in(&mut self, arg: &In) {
        before!(self, r#in, arg);
        self.veryl_token(&arg.in_token);
        after!(self, r#in, arg);
    }

    /// Semantic action for non-terminal 'Include'
    fn include(&mut self, arg: &Include) {
        before!(self, include, arg);
        self.veryl_token(&arg.include_token);
        after!(self, include, arg);
    }

    /// Semantic action for non-terminal 'Initial'
    fn initial(&mut self, arg: &Initial) {
        before!(self, initial, arg);
        self.veryl_token(&arg.initial_token);
        after!(self, initial, arg);
    }

    /// Semantic action for non-terminal 'Inout'
    fn inout(&mut self, arg: &Inout) {
        before!(self, inout, arg);
        self.veryl_token(&arg.inout_token);
        after!(self, inout, arg);
    }

    /// Semantic action for non-terminal 'Input'
    fn input(&mut self, arg: &Input) {
        before!(self, input, arg);
        self.veryl_token(&arg.input_token);
        after!(self, input, arg);
    }

    /// Semantic action for non-terminal 'Inside'
    fn inside(&mut self, arg: &Inside) {
        before!(self, inside, arg);
        self.veryl_token(&arg.inside_token);
        after!(self, inside, arg);
    }

    /// Semantic action for non-terminal 'Inst'
    fn inst(&mut self, arg: &Inst) {
        before!(self, inst, arg);
        self.veryl_token(&arg.inst_token);
        after!(self, inst, arg);
    }

    /// Semantic action for non-terminal 'Interface'
    fn interface(&mut self, arg: &Interface) {
        before!(self, interface, arg);
        self.veryl_token(&arg.interface_token);
        after!(self, interface, arg);
    }

    /// Semantic action for non-terminal 'Let'
    fn r#let(&mut self, arg: &Let) {
        before!(self, r#let, arg);
        self.veryl_token(&arg.let_token);
        after!(self, r#let, arg);
    }

    /// Semantic action for non-terminal 'Logic'
    fn logic(&mut self, arg: &Logic) {
        before!(self, logic, arg);
        self.veryl_token(&arg.logic_token);
        after!(self, logic, arg);
    }

    /// Semantic action for non-terminal 'Lsb'
    fn lsb(&mut self, arg: &Lsb) {
        before!(self, lsb, arg);
        self.veryl_token(&arg.lsb_token);
        after!(self, lsb, arg);
    }

    /// Semantic action for non-terminal 'Modport'
    fn modport(&mut self, arg: &Modport) {
        before!(self, modport, arg);
        self.veryl_token(&arg.modport_token);
        after!(self, modport, arg);
    }

    /// Semantic action for non-terminal 'Module'
    fn module(&mut self, arg: &Module) {
        before!(self, module, arg);
        self.veryl_token(&arg.module_token);
        after!(self, module, arg);
    }

    /// Semantic action for non-terminal 'Msb'
    fn msb(&mut self, arg: &Msb) {
        before!(self, msb, arg);
        self.veryl_token(&arg.msb_token);
        after!(self, msb, arg);
    }

    /// Semantic action for non-terminal 'Output'
    fn output(&mut self, arg: &Output) {
        before!(self, output, arg);
        self.veryl_token(&arg.output_token);
        after!(self, output, arg);
    }

    /// Semantic action for non-terminal 'Outside'
    fn outside(&mut self, arg: &Outside) {
        before!(self, outside, arg);
        self.veryl_token(&arg.outside_token);
        after!(self, outside, arg);
    }

    /// Semantic action for non-terminal 'Package'
    fn package(&mut self, arg: &Package) {
        before!(self, package, arg);
        self.veryl_token(&arg.package_token);
        after!(self, package, arg);
    }

    /// Semantic action for non-terminal 'Param'
    fn param(&mut self, arg: &Param) {
        before!(self, param, arg);
        self.veryl_token(&arg.param_token);
        after!(self, param, arg);
    }

    /// Semantic action for non-terminal 'Proto'
    fn proto(&mut self, arg: &Proto) {
        before!(self, proto, arg);
        self.veryl_token(&arg.proto_token);
        after!(self, proto, arg);
    }

    /// Semantic action for non-terminal 'Pub'
    fn r#pub(&mut self, arg: &Pub) {
        before!(self, r#pub, arg);
        self.veryl_token(&arg.pub_token);
        after!(self, r#pub, arg);
    }

    /// Semantic action for non-terminal 'Ref'
    fn r#ref(&mut self, arg: &Ref) {
        before!(self, r#ref, arg);
        self.veryl_token(&arg.ref_token);
        after!(self, r#ref, arg);
    }

    /// Semantic action for non-terminal 'Repeat'
    fn repeat(&mut self, arg: &Repeat) {
        before!(self, repeat, arg);
        self.veryl_token(&arg.repeat_token);
        after!(self, repeat, arg);
    }

    /// Semantic action for non-terminal 'Reset'
    fn reset(&mut self, arg: &Reset) {
        before!(self, reset, arg);
        self.veryl_token(&arg.reset_token);
        after!(self, reset, arg);
    }

    /// Semantic action for non-terminal 'ResetAsyncHigh'
    fn reset_async_high(&mut self, arg: &ResetAsyncHigh) {
        before!(self, reset_async_high, arg);
        self.veryl_token(&arg.reset_async_high_token);
        after!(self, reset_async_high, arg);
    }

    /// Semantic action for non-terminal 'ResetAsyncLow'
    fn reset_async_low(&mut self, arg: &ResetAsyncLow) {
        before!(self, reset_async_low, arg);
        self.veryl_token(&arg.reset_async_low_token);
        after!(self, reset_async_low, arg);
    }

    /// Semantic action for non-terminal 'ResetSyncHigh'
    fn reset_sync_high(&mut self, arg: &ResetSyncHigh) {
        before!(self, reset_sync_high, arg);
        self.veryl_token(&arg.reset_sync_high_token);
        after!(self, reset_sync_high, arg);
    }

    /// Semantic action for non-terminal 'ResetSyncLow'
    fn reset_sync_low(&mut self, arg: &ResetSyncLow) {
        before!(self, reset_sync_low, arg);
        self.veryl_token(&arg.reset_sync_low_token);
        after!(self, reset_sync_low, arg);
    }

    /// Semantic action for non-terminal 'Return'
    fn r#return(&mut self, arg: &Return) {
        before!(self, r#return, arg);
        self.veryl_token(&arg.return_token);
        after!(self, r#return, arg);
    }

    /// Semantic action for non-terminal 'Break'
    fn r#break(&mut self, arg: &Break) {
        before!(self, r#break, arg);
        self.veryl_token(&arg.break_token);
        after!(self, r#break, arg);
    }

    /// Semantic action for non-terminal 'Signed'
    fn signed(&mut self, arg: &Signed) {
        before!(self, signed, arg);
        self.veryl_token(&arg.signed_token);
        after!(self, signed, arg);
    }

    /// Semantic action for non-terminal 'Step'
    fn step(&mut self, arg: &Step) {
        before!(self, step, arg);
        self.veryl_token(&arg.step_token);
        after!(self, step, arg);
    }

    /// Semantic action for non-terminal 'Strin'
    fn strin(&mut self, arg: &Strin) {
        before!(self, strin, arg);
        self.veryl_token(&arg.string_token);
        after!(self, strin, arg);
    }

    /// Semantic action for non-terminal 'Struct'
    fn r#struct(&mut self, arg: &Struct) {
        before!(self, r#struct, arg);
        self.veryl_token(&arg.struct_token);
        after!(self, r#struct, arg);
    }

    /// Semantic action for non-terminal 'Switch'
    fn switch(&mut self, arg: &Switch) {
        before!(self, switch, arg);
        self.veryl_token(&arg.switch_token);
        after!(self, switch, arg);
    }

    /// Semantic action for non-terminal 'Union'
    fn union(&mut self, arg: &Union) {
        before!(self, union, arg);
        self.veryl_token(&arg.union_token);
        after!(self, union, arg);
    }

    /// Semantic action for non-terminal 'Unsafe'
    fn r#unsafe(&mut self, arg: &Unsafe) {
        before!(self, r#unsafe, arg);
        self.veryl_token(&arg.unsafe_token);
        after!(self, r#unsafe, arg);
    }

    /// Semantic action for non-terminal 'Tri'
    fn tri(&mut self, arg: &Tri) {
        before!(self, tri, arg);
        self.veryl_token(&arg.tri_token);
        after!(self, tri, arg);
    }

    /// Semantic action for non-terminal 'Type'
    fn r#type(&mut self, arg: &Type) {
        before!(self, r#type, arg);
        self.veryl_token(&arg.type_token);
        after!(self, r#type, arg);
    }

    /// Semantic action for non-terminal 'U32'
    fn u32(&mut self, arg: &U32) {
        before!(self, u32, arg);
        self.veryl_token(&arg.u32_token);
        after!(self, u32, arg);
    }

    /// Semantic action for non-terminal 'U64'
    fn u64(&mut self, arg: &U64) {
        before!(self, u64, arg);
        self.veryl_token(&arg.u64_token);
        after!(self, u64, arg);
    }

    /// Semantic action for non-terminal 'Var'
    fn var(&mut self, arg: &Var) {
        before!(self, var, arg);
        self.veryl_token(&arg.var_token);
        after!(self, var, arg);
    }

    /// Semantic action for non-terminal 'DollarIdentifier'
    fn dollar_identifier(&mut self, arg: &DollarIdentifier) {
        before!(self, dollar_identifier, arg);
        self.veryl_token(&arg.dollar_identifier_token);
        after!(self, dollar_identifier, arg);
    }

    /// Semantic action for non-terminal 'Identifier'
    fn identifier(&mut self, arg: &Identifier) {
        before!(self, identifier, arg);
        self.veryl_token(&arg.identifier_token);
        after!(self, identifier, arg);
    }

    /// Semantic action for non-terminal 'Number'
    fn number(&mut self, arg: &Number) {
        before!(self, number, arg);
        match arg {
            Number::IntegralNumber(x) => self.integral_number(&x.integral_number),
            Number::RealNumber(x) => self.real_number(&x.real_number),
        };
        after!(self, number, arg);
    }

    /// Semantic action for non-terminal 'IntegralNumber'
    fn integral_number(&mut self, arg: &IntegralNumber) {
        before!(self, integral_number, arg);
        match arg {
            IntegralNumber::Based(x) => self.based(&x.based),
            IntegralNumber::BaseLess(x) => self.base_less(&x.base_less),
            IntegralNumber::AllBit(x) => self.all_bit(&x.all_bit),
        };
        after!(self, integral_number, arg);
    }

    /// Semantic action for non-terminal 'RealNumber'
    fn real_number(&mut self, arg: &RealNumber) {
        before!(self, real_number, arg);
        match arg {
            RealNumber::FixedPoint(x) => self.fixed_point(&x.fixed_point),
            RealNumber::Exponent(x) => self.exponent(&x.exponent),
        };
        after!(self, real_number, arg);
    }

    /// Semantic action for non-terminal 'HierarchicalIdentifier'
    fn hierarchical_identifier(&mut self, arg: &HierarchicalIdentifier) {
        before!(self, hierarchical_identifier, arg);
        self.identifier(&arg.identifier);
        for x in &arg.hierarchical_identifier_list {
            self.select(&x.select);
        }
        for x in &arg.hierarchical_identifier_list0 {
            self.dot(&x.dot);
            self.identifier(&x.identifier);
            for x in &x.hierarchical_identifier_list0_list {
                self.select(&x.select);
            }
        }
        after!(self, hierarchical_identifier, arg);
    }

    /// Semantic action for non-terminal 'ScopedIdentifier'
    fn scoped_identifier(&mut self, arg: &ScopedIdentifier) {
        before!(self, scoped_identifier, arg);
        match &*arg.scoped_identifier_group {
            ScopedIdentifierGroup::IdentifierScopedIdentifierOpt(x) => {
                self.identifier(&x.identifier);
                if let Some(ref x) = x.scoped_identifier_opt {
                    self.with_generic_argument(&x.with_generic_argument);
                }
            }
            ScopedIdentifierGroup::DollarIdentifier(x) => {
                self.dollar_identifier(&x.dollar_identifier)
            }
        }
        for x in &arg.scoped_identifier_list {
            self.colon_colon(&x.colon_colon);
            self.identifier(&x.identifier);
            if let Some(ref x) = x.scoped_identifier_opt0 {
                self.with_generic_argument(&x.with_generic_argument);
            }
        }
        after!(self, scoped_identifier, arg);
    }

    /// Semantic action for non-terminal 'ExpressionIdentifier'
    fn expression_identifier(&mut self, arg: &ExpressionIdentifier) {
        before!(self, expression_identifier, arg);
        self.scoped_identifier(&arg.scoped_identifier);
        if let Some(ref x) = arg.expression_identifier_opt {
            self.width(&x.width);
        }
        for x in &arg.expression_identifier_list {
            self.select(&x.select);
        }
        for x in &arg.expression_identifier_list0 {
            self.dot(&x.dot);
            self.identifier(&x.identifier);
            for x in &x.expression_identifier_list0_list {
                self.select(&x.select);
            }
        }
        after!(self, expression_identifier, arg);
    }

    /// Semantic action for non-terminal 'Expression'
    fn expression(&mut self, arg: &Expression) {
        before!(self, expression, arg);
        self.expression01(&arg.expression01);
        for x in &arg.expression_list {
            self.operator01(&x.operator01);
            self.expression01(&x.expression01);
        }
        after!(self, expression, arg);
    }

    /// Semantic action for non-terminal 'Expression01'
    fn expression01(&mut self, arg: &Expression01) {
        before!(self, expression01, arg);
        self.expression02(&arg.expression02);
        for x in &arg.expression01_list {
            self.operator02(&x.operator02);
            self.expression02(&x.expression02);
        }
        after!(self, expression01, arg);
    }

    /// Semantic action for non-terminal 'Expression02'
    fn expression02(&mut self, arg: &Expression02) {
        before!(self, expression02, arg);
        self.expression03(&arg.expression03);
        for x in &arg.expression02_list {
            self.operator03(&x.operator03);
            self.expression03(&x.expression03);
        }
        after!(self, expression02, arg);
    }

    /// Semantic action for non-terminal 'Expression03'
    fn expression03(&mut self, arg: &Expression03) {
        before!(self, expression03, arg);
        self.expression04(&arg.expression04);
        for x in &arg.expression03_list {
            self.operator04(&x.operator04);
            self.expression04(&x.expression04);
        }
        after!(self, expression03, arg);
    }

    /// Semantic action for non-terminal 'Expression04'
    fn expression04(&mut self, arg: &Expression04) {
        before!(self, expression04, arg);
        self.expression05(&arg.expression05);
        for x in &arg.expression04_list {
            self.operator05(&x.operator05);
            self.expression05(&x.expression05);
        }
        after!(self, expression04, arg);
    }

    /// Semantic action for non-terminal 'Expression05'
    fn expression05(&mut self, arg: &Expression05) {
        before!(self, expression05, arg);
        self.expression06(&arg.expression06);
        for x in &arg.expression05_list {
            self.operator06(&x.operator06);
            self.expression06(&x.expression06);
        }
        after!(self, expression05, arg);
    }

    /// Semantic action for non-terminal 'Expression06'
    fn expression06(&mut self, arg: &Expression06) {
        before!(self, expression06, arg);
        self.expression07(&arg.expression07);
        for x in &arg.expression06_list {
            self.operator07(&x.operator07);
            self.expression07(&x.expression07);
        }
        after!(self, expression06, arg);
    }

    /// Semantic action for non-terminal 'Expression07'
    fn expression07(&mut self, arg: &Expression07) {
        before!(self, expression07, arg);
        self.expression08(&arg.expression08);
        for x in &arg.expression07_list {
            self.operator08(&x.operator08);
            self.expression08(&x.expression08);
        }
        after!(self, expression07, arg);
    }

    /// Semantic action for non-terminal 'Expression08'
    fn expression08(&mut self, arg: &Expression08) {
        before!(self, expression08, arg);
        self.expression09(&arg.expression09);
        for x in &arg.expression08_list {
            self.operator09(&x.operator09);
            self.expression09(&x.expression09);
        }
        after!(self, expression08, arg);
    }

    /// Semantic action for non-terminal 'Expression09'
    fn expression09(&mut self, arg: &Expression09) {
        before!(self, expression09, arg);
        self.expression10(&arg.expression10);
        for x in &arg.expression09_list {
            match &*x.expression09_list_group {
                Expression09ListGroup::Operator10(x) => self.operator10(&x.operator10),
                Expression09ListGroup::Star(x) => self.star(&x.star),
            }
            self.expression10(&x.expression10);
        }
        after!(self, expression09, arg);
    }

    /// Semantic action for non-terminal 'Expression10'
    fn expression10(&mut self, arg: &Expression10) {
        before!(self, expression10, arg);
        self.expression11(&arg.expression11);
        for x in &arg.expression10_list {
            self.operator11(&x.operator11);
            self.expression11(&x.expression11);
        }
        after!(self, expression10, arg);
    }

    /// Semantic action for non-terminal 'Expression11'
    fn expression11(&mut self, arg: &Expression11) {
        before!(self, expression11, arg);
        self.expression12(&arg.expression12);
        if let Some(x) = &arg.expression11_opt {
            self.r#as(&x.r#as);
            self.casting_type(&x.casting_type);
        }
        after!(self, expression11, arg);
    }

    /// Semantic action for non-terminal 'Expression12'
    fn expression12(&mut self, arg: &Expression12) {
        before!(self, expression12, arg);
        for x in &arg.expression12_list {
            match &*x.expression12_list_group {
                Expression12ListGroup::UnaryOperator(x) => self.unary_operator(&x.unary_operator),
                Expression12ListGroup::Operator03(x) => self.operator03(&x.operator03),
                Expression12ListGroup::Operator04(x) => self.operator04(&x.operator04),
                Expression12ListGroup::Operator05(x) => self.operator05(&x.operator05),
                Expression12ListGroup::Operator09(x) => self.operator09(&x.operator09),
            }
        }
        self.factor(&arg.factor);
        after!(self, expression12, arg);
    }

    /// Semantic action for non-terminal 'Factor'
    fn factor(&mut self, arg: &Factor) {
        before!(self, factor, arg);
        match arg {
            Factor::Number(x) => self.number(&x.number),
            Factor::IdentifierFactor(x) => self.identifier_factor(&x.identifier_factor),
            Factor::LParenExpressionRParen(x) => {
                self.l_paren(&x.l_paren);
                self.expression(&x.expression);
                self.r_paren(&x.r_paren);
            }
            Factor::LBraceConcatenationListRBrace(x) => {
                self.l_brace(&x.l_brace);
                self.concatenation_list(&x.concatenation_list);
                self.r_brace(&x.r_brace);
            }
            Factor::QuoteLBraceArrayLiteralListRBrace(x) => {
                self.quote_l_brace(&x.quote_l_brace);
                self.array_literal_list(&x.array_literal_list);
                self.r_brace(&x.r_brace);
            }
            Factor::IfExpression(x) => {
                self.if_expression(&x.if_expression);
            }
            Factor::CaseExpression(x) => {
                self.case_expression(&x.case_expression);
            }
            Factor::SwitchExpression(x) => {
                self.switch_expression(&x.switch_expression);
            }
            Factor::StringLiteral(x) => {
                self.string_literal(&x.string_literal);
            }
            Factor::FactorGroup(x) => match &*x.factor_group {
                FactorGroup::Msb(x) => self.msb(&x.msb),
                FactorGroup::Lsb(x) => self.lsb(&x.lsb),
            },
            Factor::InsideExpression(x) => {
                self.inside_expression(&x.inside_expression);
            }
            Factor::OutsideExpression(x) => {
                self.outside_expression(&x.outside_expression);
            }
            Factor::TypeExpression(x) => {
                self.type_expression(&x.type_expression);
            }
            Factor::FactorType(x) => {
                self.factor_type(&x.factor_type);
            }
        }
        after!(self, factor, arg);
    }

    /// Semantic action for non-terminal 'IdentifierFactor'
    fn identifier_factor(&mut self, arg: &IdentifierFactor) {
        before!(self, identifier_factor, arg);
        self.expression_identifier(&arg.expression_identifier);
        if let Some(ref x) = arg.identifier_factor_opt {
            self.function_call(&x.function_call);
        }
        after!(self, identifier_factor, arg);
    }

    /// Semantic action for non-terminal 'FunctionCall'
    fn function_call(&mut self, arg: &FunctionCall) {
        before!(self, function_call, arg);
        self.l_paren(&arg.l_paren);
        if let Some(ref x) = arg.function_call_opt {
            self.argument_list(&x.argument_list);
        }
        self.r_paren(&arg.r_paren);
        after!(self, function_call, arg);
    }

    /// Semantic action for non-terminal 'ArgumentList'
    fn argument_list(&mut self, arg: &ArgumentList) {
        before!(self, argument_list, arg);
        self.argument_item(&arg.argument_item);
        for x in &arg.argument_list_list {
            self.comma(&x.comma);
            self.argument_item(&x.argument_item);
        }
        if let Some(ref x) = arg.argument_list_opt {
            self.comma(&x.comma);
        }
        after!(self, argument_list, arg);
    }

    /// Semantic action for non-terminal 'ArgumentItem'
    fn argument_item(&mut self, arg: &ArgumentItem) {
        before!(self, argument_item, arg);
        self.expression(&arg.expression);
        after!(self, argument_item, arg);
    }

    /// Semantic action for non-terminal 'ConcatenationList'
    fn concatenation_list(&mut self, arg: &ConcatenationList) {
        before!(self, concatenation_list, arg);
        self.concatenation_item(&arg.concatenation_item);
        for x in &arg.concatenation_list_list {
            self.comma(&x.comma);
            self.concatenation_item(&x.concatenation_item);
        }
        if let Some(ref x) = arg.concatenation_list_opt {
            self.comma(&x.comma);
        }
        after!(self, concatenation_list, arg);
    }

    /// Semantic action for non-terminal 'ConcatenationItem'
    fn concatenation_item(&mut self, arg: &ConcatenationItem) {
        before!(self, concatenation_item, arg);
        self.expression(&arg.expression);
        if let Some(ref x) = arg.concatenation_item_opt {
            self.repeat(&x.repeat);
            self.expression(&x.expression);
        }
        after!(self, concatenation_item, arg);
    }

    /// Semantic action for non-terminal 'ArrayLiteralList'
    fn array_literal_list(&mut self, arg: &ArrayLiteralList) {
        before!(self, array_literal_list, arg);
        self.array_literal_item(&arg.array_literal_item);
        for x in &arg.array_literal_list_list {
            self.comma(&x.comma);
            self.array_literal_item(&x.array_literal_item);
        }
        if let Some(ref x) = arg.array_literal_list_opt {
            self.comma(&x.comma);
        }
        after!(self, array_literal_list, arg);
    }

    /// Semantic action for non-terminal 'ArrayLiteralItem'
    fn array_literal_item(&mut self, arg: &ArrayLiteralItem) {
        before!(self, array_literal_item, arg);
        match &*arg.array_literal_item_group {
            ArrayLiteralItemGroup::ExpressionArrayLiteralItemOpt(x) => {
                self.expression(&x.expression);
                if let Some(ref x) = x.array_literal_item_opt {
                    self.repeat(&x.repeat);
                    self.expression(&x.expression);
                }
            }
            ArrayLiteralItemGroup::DefaulColonExpression(x) => {
                self.defaul(&x.defaul);
                self.colon(&x.colon);
                self.expression(&x.expression);
            }
        }
        after!(self, array_literal_item, arg);
    }

    /// Semantic action for non-terminal 'IfExpression'
    fn if_expression(&mut self, arg: &IfExpression) {
        before!(self, if_expression, arg);
        self.r#if(&arg.r#if);
        self.expression(&arg.expression);
        self.l_brace(&arg.l_brace);
        self.expression(&arg.expression0);
        self.r_brace(&arg.r_brace);
        for x in &arg.if_expression_list {
            self.r#else(&x.r#else);
            self.r#if(&x.r#if);
            self.expression(&x.expression);
            self.l_brace(&x.l_brace);
            self.expression(&x.expression0);
            self.r_brace(&x.r_brace);
        }
        self.r#else(&arg.r#else);
        self.l_brace(&arg.l_brace0);
        self.expression(&arg.expression1);
        self.r_brace(&arg.r_brace0);
        after!(self, if_expression, arg);
    }

    /// Semantic action for non-terminal 'CaseExpression'
    fn case_expression(&mut self, arg: &CaseExpression) {
        before!(self, case_expression, arg);
        self.case(&arg.case);
        self.expression(&arg.expression);
        self.l_brace(&arg.l_brace);
        self.case_condition(&arg.case_condition);
        self.colon(&arg.colon);
        self.expression(&arg.expression0);
        self.comma(&arg.comma);
        for x in &arg.case_expression_list {
            self.case_condition(&x.case_condition);
            self.colon(&x.colon);
            self.expression(&x.expression);
            self.comma(&x.comma);
        }
        self.defaul(&arg.defaul);
        self.colon(&arg.colon0);
        self.expression(&arg.expression1);
        if let Some(ref x) = arg.case_expression_opt {
            self.comma(&x.comma);
        }
        self.r_brace(&arg.r_brace);
        after!(self, case_expression, arg);
    }

    /// Semantic action for non-terminal 'SwitchExpression'
    fn switch_expression(&mut self, arg: &SwitchExpression) {
        before!(self, switch_expression, arg);
        self.switch(&arg.switch);
        self.l_brace(&arg.l_brace);
        self.switch_condition(&arg.switch_condition);
        self.colon(&arg.colon);
        self.expression(&arg.expression);
        self.comma(&arg.comma);
        for x in &arg.switch_expression_list {
            self.switch_condition(&x.switch_condition);
            self.colon(&x.colon);
            self.expression(&x.expression);
            self.comma(&x.comma);
        }
        self.defaul(&arg.defaul);
        self.colon(&arg.colon0);
        self.expression(&arg.expression0);
        if let Some(ref x) = arg.switch_expression_opt {
            self.comma(&x.comma);
        }
        self.r_brace(&arg.r_brace);
        after!(self, switch_expression, arg);
    }

    /// Semantic action for non-terminal 'TypeExpression'
    fn type_expression(&mut self, arg: &TypeExpression) {
        before!(self, type_expression, arg);
        self.r#type(&arg.r#type);
        self.l_paren(&arg.l_paren);
        self.expression(&arg.expression);
        self.r_paren(&arg.r_paren);
        after!(self, type_expression, arg);
    }

    /// Semantic action for non-terminal 'InsideExpression'
    fn inside_expression(&mut self, arg: &InsideExpression) {
        before!(self, inside_expression, arg);
        self.inside(&arg.inside);
        self.expression(&arg.expression);
        self.l_brace(&arg.l_brace);
        self.range_list(&arg.range_list);
        self.r_brace(&arg.r_brace);
        after!(self, inside_expression, arg);
    }

    /// Semantic action for non-terminal 'OutsideExpression'
    fn outside_expression(&mut self, arg: &OutsideExpression) {
        before!(self, outside_expression, arg);
        self.outside(&arg.outside);
        self.expression(&arg.expression);
        self.l_brace(&arg.l_brace);
        self.range_list(&arg.range_list);
        self.r_brace(&arg.r_brace);
        after!(self, outside_expression, arg);
    }

    /// Semantic action for non-terminal 'RangeList'
    fn range_list(&mut self, arg: &RangeList) {
        before!(self, range_list, arg);
        self.range_item(&arg.range_item);
        for x in &arg.range_list_list {
            self.comma(&x.comma);
            self.range_item(&x.range_item);
        }
        if let Some(ref x) = arg.range_list_opt {
            self.comma(&x.comma);
        }
        after!(self, range_list, arg);
    }

    /// Semantic action for non-terminal 'RangeItem'
    fn range_item(&mut self, arg: &RangeItem) {
        before!(self, range_item, arg);
        self.range(&arg.range);
        after!(self, range_item, arg);
    }

    /// Semantic action for non-terminal 'Select'
    fn select(&mut self, arg: &Select) {
        before!(self, select, arg);
        self.l_bracket(&arg.l_bracket);
        self.expression(&arg.expression);
        if let Some(ref x) = arg.select_opt {
            self.select_operator(&x.select_operator);
            self.expression(&x.expression);
        }
        self.r_bracket(&arg.r_bracket);
        after!(self, select, arg);
    }

    /// Semantic action for non-terminal 'SelectOperator'
    fn select_operator(&mut self, arg: &SelectOperator) {
        before!(self, select_operator, arg);
        match arg {
            SelectOperator::Colon(x) => self.colon(&x.colon),
            SelectOperator::PlusColon(x) => self.plus_colon(&x.plus_colon),
            SelectOperator::MinusColon(x) => self.minus_colon(&x.minus_colon),
            SelectOperator::Step(x) => self.step(&x.step),
        }
        after!(self, select_operator, arg);
    }

    /// Semantic action for non-terminal 'Width'
    fn width(&mut self, arg: &Width) {
        before!(self, width, arg);
        self.l_angle(&arg.l_angle);
        self.expression(&arg.expression);
        for x in &arg.width_list {
            self.comma(&x.comma);
            self.expression(&x.expression);
        }
        self.r_angle(&arg.r_angle);
        after!(self, width, arg);
    }

    /// Semantic action for non-terminal 'Array'
    fn array(&mut self, arg: &Array) {
        before!(self, array, arg);
        self.l_bracket(&arg.l_bracket);
        self.expression(&arg.expression);
        for x in &arg.array_list {
            self.comma(&x.comma);
            self.expression(&x.expression);
        }
        self.r_bracket(&arg.r_bracket);
        after!(self, array, arg);
    }

    /// Semantic action for non-terminal 'Range'
    fn range(&mut self, arg: &Range) {
        before!(self, range, arg);
        self.expression(&arg.expression);
        if let Some(ref x) = arg.range_opt {
            self.range_operator(&x.range_operator);
            self.expression(&x.expression);
        }
        after!(self, range, arg);
    }

    /// Semantic action for non-terminal 'RangeOperator'
    fn range_operator(&mut self, arg: &RangeOperator) {
        before!(self, range_operator, arg);
        match arg {
            RangeOperator::DotDot(x) => self.dot_dot(&x.dot_dot),
            RangeOperator::DotDotEqu(x) => self.dot_dot_equ(&x.dot_dot_equ),
        }
        after!(self, range_operator, arg);
    }

    /// Semantic action for non-terminal 'FixedType'
    fn fixed_type(&mut self, arg: &FixedType) {
        before!(self, fixed_type, arg);
        match arg {
            FixedType::U32(x) => self.u32(&x.u32),
            FixedType::U64(x) => self.u64(&x.u64),
            FixedType::I32(x) => self.i32(&x.i32),
            FixedType::I64(x) => self.i64(&x.i64),
            FixedType::F32(x) => self.f32(&x.f32),
            FixedType::F64(x) => self.f64(&x.f64),
            FixedType::Strin(x) => self.strin(&x.strin),
        };
        after!(self, fixed_type, arg);
    }

    /// Semantic action for non-terminal 'VariableType'
    fn variable_type(&mut self, arg: &VariableType) {
        before!(self, variable_type, arg);
        match arg {
            VariableType::Clock(x) => self.clock(&x.clock),
            VariableType::ClockPosedge(x) => self.clock_posedge(&x.clock_posedge),
            VariableType::ClockNegedge(x) => self.clock_negedge(&x.clock_negedge),
            VariableType::Reset(x) => self.reset(&x.reset),
            VariableType::ResetAsyncHigh(x) => self.reset_async_high(&x.reset_async_high),
            VariableType::ResetAsyncLow(x) => self.reset_async_low(&x.reset_async_low),
            VariableType::ResetSyncHigh(x) => self.reset_sync_high(&x.reset_sync_high),
            VariableType::ResetSyncLow(x) => self.reset_sync_low(&x.reset_sync_low),
            VariableType::Logic(x) => self.logic(&x.logic),
            VariableType::Bit(x) => self.bit(&x.bit),
        };
        after!(self, variable_type, arg);
    }

    /// Semantic action for non-terminal 'UserDefinedType'
    fn user_defined_type(&mut self, arg: &UserDefinedType) {
        before!(self, user_defined_type, arg);
        self.scoped_identifier(&arg.scoped_identifier);
        after!(self, user_defined_type, arg);
    }

    /// Semantic action for non-terminal 'TypeModifier'
    fn type_modifier(&mut self, arg: &TypeModifier) {
        before!(self, type_modifier, arg);
        match arg {
            TypeModifier::Tri(x) => self.tri(&x.tri),
            TypeModifier::Signed(x) => self.signed(&x.signed),
        }
        after!(self, type_modifier, arg);
    }

    /// Semantic action for non-terminal 'FactorType'
    fn factor_type(&mut self, arg: &FactorType) {
        before!(self, factor_type, arg);
        match arg.factor_type_group.as_ref() {
            FactorTypeGroup::VariableTypeFactorTypeOpt(x) => {
                self.variable_type(&x.variable_type);
                if let Some(ref x) = x.factor_type_opt {
                    self.width(&x.width);
                }
            }
            FactorTypeGroup::FixedType(x) => self.fixed_type(&x.fixed_type),
        }
        after!(self, factor_type, arg);
    }

    /// Semantic action for non-terminal 'ScalarType'
    fn scalar_type(&mut self, arg: &ScalarType) {
        before!(self, scalar_type, arg);
        for x in &arg.scalar_type_list {
            self.type_modifier(&x.type_modifier);
        }
        match &*arg.scalar_type_group {
            ScalarTypeGroup::UserDefinedTypeScalarTypeOpt(x) => {
                self.user_defined_type(&x.user_defined_type);
                if let Some(ref x) = x.scalar_type_opt {
                    self.width(&x.width);
                }
            }
            ScalarTypeGroup::FactorType(x) => {
                self.factor_type(&x.factor_type);
            }
        }
        after!(self, scalar_type, arg);
    }

    /// Semantic action for non-terminal 'ArrayType'
    fn array_type(&mut self, arg: &ArrayType) {
        before!(self, array_type, arg);
        self.scalar_type(&arg.scalar_type);
        if let Some(ref x) = arg.array_type_opt {
            self.array(&x.array);
        }
        after!(self, array_type, arg);
    }

    /// Semantic action for non-terminal 'CastingType'
    fn casting_type(&mut self, arg: &CastingType) {
        before!(self, casting_type, arg);
        match arg {
            CastingType::U32(x) => self.u32(&x.u32),
            CastingType::U64(x) => self.u64(&x.u64),
            CastingType::I32(x) => self.i32(&x.i32),
            CastingType::I64(x) => self.i64(&x.i64),
            CastingType::F32(x) => self.f32(&x.f32),
            CastingType::F64(x) => self.f64(&x.f64),
            CastingType::Clock(x) => self.clock(&x.clock),
            CastingType::ClockPosedge(x) => self.clock_posedge(&x.clock_posedge),
            CastingType::ClockNegedge(x) => self.clock_negedge(&x.clock_negedge),
            CastingType::Reset(x) => self.reset(&x.reset),
            CastingType::ResetAsyncHigh(x) => self.reset_async_high(&x.reset_async_high),
            CastingType::ResetAsyncLow(x) => self.reset_async_low(&x.reset_async_low),
            CastingType::ResetSyncHigh(x) => self.reset_sync_high(&x.reset_sync_high),
            CastingType::ResetSyncLow(x) => self.reset_sync_low(&x.reset_sync_low),
            CastingType::UserDefinedType(x) => self.user_defined_type(&x.user_defined_type),
            CastingType::Based(x) => self.based(&x.based),
            CastingType::BaseLess(x) => self.base_less(&x.base_less),
        }
        after!(self, casting_type, arg);
    }

    /// Semantic action for non-terminal 'ClockDomain'
    fn clock_domain(&mut self, arg: &ClockDomain) {
        before!(self, clock_domain, arg);
        self.back_quote(&arg.back_quote);
        self.identifier(&arg.identifier);
        after!(self, clock_domain, arg);
    }

    /// Semantic action for non-terminal 'StatementBlock'
    fn statement_block(&mut self, arg: &StatementBlock) {
        before!(self, statement_block, arg);
        self.l_brace(&arg.l_brace);
        for x in &arg.statement_block_list {
            self.statement_block_group(&x.statement_block_group);
        }
        self.r_brace(&arg.r_brace);
        after!(self, statement_block, arg);
    }

    /// Semantic action for non-terminal 'StatementBlockGroup'
    fn statement_block_group(&mut self, arg: &StatementBlockGroup) {
        before!(self, statement_block_group, arg);
        for x in &arg.statement_block_group_list {
            self.attribute(&x.attribute);
        }
        match arg.statement_block_group_group.as_ref() {
            StatementBlockGroupGroup::LBraceStatementBlockGroupGroupListRBrace(x) => {
                self.l_brace(&x.l_brace);
                for x in &x.statement_block_group_group_list {
                    self.statement_block_group(&x.statement_block_group);
                }
                self.r_brace(&x.r_brace);
            }
            StatementBlockGroupGroup::StatementBlockItem(x) => {
                self.statement_block_item(&x.statement_block_item);
            }
        }
        after!(self, statement_block_group, arg);
    }

    /// Semantic action for non-terminal 'StatementOrVarDeclaration'
    fn statement_block_item(&mut self, arg: &StatementBlockItem) {
        before!(self, statement_block_item, arg);
        match arg {
            StatementBlockItem::VarDeclaration(x) => self.var_declaration(&x.var_declaration),
            StatementBlockItem::LetStatement(x) => self.let_statement(&x.let_statement),
            StatementBlockItem::Statement(x) => self.statement(&x.statement),
        }
        after!(self, statement_block_item, arg);
    }

    /// Semantic action for non-terminal 'Statement'
    fn statement(&mut self, arg: &Statement) {
        before!(self, statement, arg);
        match arg {
            Statement::IdentifierStatement(x) => self.identifier_statement(&x.identifier_statement),
            Statement::IfStatement(x) => self.if_statement(&x.if_statement),
            Statement::IfResetStatement(x) => self.if_reset_statement(&x.if_reset_statement),
            Statement::ReturnStatement(x) => self.return_statement(&x.return_statement),
            Statement::BreakStatement(x) => self.break_statement(&x.break_statement),
            Statement::ForStatement(x) => self.for_statement(&x.for_statement),
            Statement::CaseStatement(x) => self.case_statement(&x.case_statement),
            Statement::SwitchStatement(x) => self.switch_statement(&x.switch_statement),
        };
        after!(self, statement, arg);
    }

    /// Semantic action for non-terminal 'LetStatement'
    fn let_statement(&mut self, arg: &LetStatement) {
        before!(self, let_statement, arg);
        self.r#let(&arg.r#let);
        self.identifier(&arg.identifier);
        self.colon(&arg.colon);
        if let Some(ref x) = arg.let_statement_opt {
            self.clock_domain(&x.clock_domain);
        }
        self.array_type(&arg.array_type);
        self.equ(&arg.equ);
        self.expression(&arg.expression);
        self.semicolon(&arg.semicolon);
        after!(self, let_statement, arg);
    }

    /// Semantic action for non-terminal 'IdentifierStatement'
    fn identifier_statement(&mut self, arg: &IdentifierStatement) {
        before!(self, identifier_statement, arg);
        self.expression_identifier(&arg.expression_identifier);
        match &*arg.identifier_statement_group {
            IdentifierStatementGroup::FunctionCall(x) => {
                self.function_call(&x.function_call);
            }
            IdentifierStatementGroup::Assignment(x) => {
                self.assignment(&x.assignment);
            }
        }
        self.semicolon(&arg.semicolon);
        after!(self, identifier_statement, arg);
    }

    /// Semantic action for non-terminal 'Assignment'
    fn assignment(&mut self, arg: &Assignment) {
        before!(self, assignment, arg);
        match &*arg.assignment_group {
            AssignmentGroup::Equ(x) => self.equ(&x.equ),
            AssignmentGroup::AssignmentOperator(x) => {
                self.assignment_operator(&x.assignment_operator)
            }
        }
        self.expression(&arg.expression);
        after!(self, assignment, arg);
    }

    /// Semantic action for non-terminal 'IfStatement'
    fn if_statement(&mut self, arg: &IfStatement) {
        before!(self, if_statement, arg);
        self.r#if(&arg.r#if);
        self.expression(&arg.expression);
        self.statement_block(&arg.statement_block);
        for x in &arg.if_statement_list {
            self.r#else(&x.r#else);
            self.r#if(&x.r#if);
            self.expression(&x.expression);
            self.statement_block(&x.statement_block);
        }
        if let Some(ref x) = arg.if_statement_opt {
            self.r#else(&x.r#else);
            self.statement_block(&x.statement_block);
        }
        after!(self, if_statement, arg);
    }

    /// Semantic action for non-terminal 'IfResetStatement'
    fn if_reset_statement(&mut self, arg: &IfResetStatement) {
        before!(self, if_reset_statement, arg);
        self.if_reset(&arg.if_reset);
        self.statement_block(&arg.statement_block);
        for x in &arg.if_reset_statement_list {
            self.r#else(&x.r#else);
            self.r#if(&x.r#if);
            self.expression(&x.expression);
            self.statement_block(&x.statement_block);
        }
        if let Some(ref x) = arg.if_reset_statement_opt {
            self.r#else(&x.r#else);
            self.statement_block(&x.statement_block);
        }
        after!(self, if_reset_statement, arg);
    }

    /// Semantic action for non-terminal 'ReturnStatement'
    fn return_statement(&mut self, arg: &ReturnStatement) {
        before!(self, return_statement, arg);
        self.r#return(&arg.r#return);
        self.expression(&arg.expression);
        self.semicolon(&arg.semicolon);
        after!(self, return_statement, arg);
    }

    /// Semantic action for non-terminal 'BreakStatement'
    fn break_statement(&mut self, arg: &BreakStatement) {
        before!(self, break_statement, arg);
        self.r#break(&arg.r#break);
        self.semicolon(&arg.semicolon);
        after!(self, break_statement, arg);
    }

    /// Semantic action for non-terminal 'ForStatement'
    fn for_statement(&mut self, arg: &ForStatement) {
        before!(self, for_statement, arg);
        self.r#for(&arg.r#for);
        self.identifier(&arg.identifier);
        self.colon(&arg.colon);
        self.scalar_type(&arg.scalar_type);
        self.r#in(&arg.r#in);
        self.range(&arg.range);
        if let Some(ref x) = arg.for_statement_opt {
            self.step(&x.step);
            self.assignment_operator(&x.assignment_operator);
            self.expression(&x.expression);
        }
        self.statement_block(&arg.statement_block);
        after!(self, for_statement, arg);
    }

    /// Semantic action for non-terminal 'CaseStatement'
    fn case_statement(&mut self, arg: &CaseStatement) {
        before!(self, case_statement, arg);
        self.case(&arg.case);
        self.expression(&arg.expression);
        self.l_brace(&arg.l_brace);
        for x in &arg.case_statement_list {
            self.case_item(&x.case_item);
        }
        self.r_brace(&arg.r_brace);
        after!(self, case_statement, arg);
    }

    /// Semantic action for non-terminal 'CaseItem'
    fn case_item(&mut self, arg: &CaseItem) {
        before!(self, case_item, arg);
        match &*arg.case_item_group {
            CaseItemGroup::CaseCondition(x) => self.case_condition(&x.case_condition),
            CaseItemGroup::Defaul(x) => self.defaul(&x.defaul),
        }
        self.colon(&arg.colon);
        match &*arg.case_item_group0 {
            CaseItemGroup0::Statement(x) => self.statement(&x.statement),
            CaseItemGroup0::StatementBlock(x) => self.statement_block(&x.statement_block),
        }
        after!(self, case_item, arg);
    }

    /// Semantic action for non-terminal 'CaseCondition'
    fn case_condition(&mut self, arg: &CaseCondition) {
        before!(self, case_condition, arg);
        self.range_item(&arg.range_item);
        for x in &arg.case_condition_list {
            self.comma(&x.comma);
            self.range_item(&x.range_item);
        }
        after!(self, case_condition, arg);
    }

    /// Semantic action for non-terminal 'SwitchStatement'
    fn switch_statement(&mut self, arg: &SwitchStatement) {
        before!(self, switch_statement, arg);
        self.switch(&arg.switch);
        self.l_brace(&arg.l_brace);
        for x in &arg.switch_statement_list {
            self.switch_item(&x.switch_item);
        }
        self.r_brace(&arg.r_brace);
        after!(self, switch_statement, arg);
    }

    /// Semantic action for non-terminal 'SwitchItem'
    fn switch_item(&mut self, arg: &SwitchItem) {
        before!(self, switch_item, arg);
        match &*arg.switch_item_group {
            SwitchItemGroup::SwitchCondition(x) => self.switch_condition(&x.switch_condition),
            SwitchItemGroup::Defaul(x) => self.defaul(&x.defaul),
        }
        self.colon(&arg.colon);
        match &*arg.switch_item_group0 {
            SwitchItemGroup0::Statement(x) => self.statement(&x.statement),
            SwitchItemGroup0::StatementBlock(x) => self.statement_block(&x.statement_block),
        }
        after!(self, switch_item, arg);
    }

    /// Semantic action for non-terminal 'SwitchCondition'
    fn switch_condition(&mut self, arg: &SwitchCondition) {
        before!(self, switch_condition, arg);
        self.expression(&arg.expression);
        for x in &arg.switch_condition_list {
            self.comma(&x.comma);
            self.expression(&x.expression);
        }
        after!(self, switch_condition, arg);
    }

    /// Semantic action for non-terminal 'Attribute'
    fn attribute(&mut self, arg: &Attribute) {
        before!(self, attribute, arg);
        self.hash(&arg.hash);
        self.l_bracket(&arg.l_bracket);
        self.identifier(&arg.identifier);
        if let Some(ref x) = arg.attribute_opt {
            self.l_paren(&x.l_paren);
            self.attribute_list(&x.attribute_list);
            self.r_paren(&x.r_paren);
        }
        self.r_bracket(&arg.r_bracket);
        after!(self, attribute, arg);
    }

    /// Semantic action for non-terminal 'AttributeList'
    fn attribute_list(&mut self, arg: &AttributeList) {
        before!(self, attribute_list, arg);
        self.attribute_item(&arg.attribute_item);
        for x in &arg.attribute_list_list {
            self.comma(&x.comma);
            self.attribute_item(&x.attribute_item);
        }
        if let Some(ref x) = arg.attribute_list_opt {
            self.comma(&x.comma);
        }
        after!(self, attribute_list, arg);
    }

    /// Semantic action for non-terminal 'AttributeItem'
    fn attribute_item(&mut self, arg: &AttributeItem) {
        before!(self, attribute_item, arg);
        match arg {
            AttributeItem::Identifier(x) => self.identifier(&x.identifier),
            AttributeItem::StringLiteral(x) => self.string_literal(&x.string_literal),
        }
        after!(self, attribute_item, arg);
    }

    /// Semantic action for non-terminal 'LetDeclaration'
    fn let_declaration(&mut self, arg: &LetDeclaration) {
        before!(self, let_declaration, arg);
        self.r#let(&arg.r#let);
        self.identifier(&arg.identifier);
        self.colon(&arg.colon);
        if let Some(ref x) = arg.let_declaration_opt {
            self.clock_domain(&x.clock_domain);
        }
        self.array_type(&arg.array_type);
        self.equ(&arg.equ);
        self.expression(&arg.expression);
        self.semicolon(&arg.semicolon);
        after!(self, let_declaration, arg);
    }

    /// Semantic action for non-terminal 'VarDeclaration'
    fn var_declaration(&mut self, arg: &VarDeclaration) {
        before!(self, var_declaration, arg);
        self.var(&arg.var);
        self.identifier(&arg.identifier);
        self.colon(&arg.colon);
        if let Some(ref x) = arg.var_declaration_opt {
            self.clock_domain(&x.clock_domain);
        }
        self.array_type(&arg.array_type);
        self.semicolon(&arg.semicolon);
        after!(self, var_declaration, arg);
    }

    /// Semantic action for non-terminal 'ConstDeclaration'
    fn const_declaration(&mut self, arg: &ConstDeclaration) {
        before!(self, const_declaration, arg);
        self.r#const(&arg.r#const);
        self.identifier(&arg.identifier);
        self.colon(&arg.colon);
        match &*arg.const_declaration_group {
            ConstDeclarationGroup::ArrayType(x) => {
                self.array_type(&x.array_type);
            }
            ConstDeclarationGroup::Type(x) => {
                self.r#type(&x.r#type);
            }
        }
        self.equ(&arg.equ);
        self.expression(&arg.expression);
        self.semicolon(&arg.semicolon);
        after!(self, const_declaration, arg);
    }

    /// Semantic action for non-terminal 'TypeDefDeclaration'
    fn type_def_declaration(&mut self, arg: &TypeDefDeclaration) {
        before!(self, type_def_declaration, arg);
        self.r#type(&arg.r#type);
        self.identifier(&arg.identifier);
        self.equ(&arg.equ);
        self.array_type(&arg.array_type);
        self.semicolon(&arg.semicolon);
        after!(self, type_def_declaration, arg);
    }

    /// Semantic action for non-terminal 'AlwaysFfDeclaration'
    fn always_ff_declaration(&mut self, arg: &AlwaysFfDeclaration) {
        before!(self, always_ff_declaration, arg);
        self.always_ff(&arg.always_ff);
        if let Some(ref x) = arg.always_ff_declaration_opt {
            self.always_ff_event_list(&x.always_ff_event_list);
        }
        self.statement_block(&arg.statement_block);
        after!(self, always_ff_declaration, arg);
    }

    /// Semantic action for non-terminal 'AlwaysFfEventList'
    fn always_ff_event_list(&mut self, arg: &AlwaysFfEventList) {
        before!(self, always_ff_event_list, arg);
        self.l_paren(&arg.l_paren);
        self.always_ff_clock(&arg.always_ff_clock);
        if let Some(ref x) = arg.always_ff_event_list_opt {
            self.comma(&x.comma);
            self.always_ff_reset(&x.always_ff_reset);
        }
        self.r_paren(&arg.r_paren);
        after!(self, always_ff_event_list, arg);
    }

    /// Semantic action for non-terminal 'AlwaysFfClock'
    fn always_ff_clock(&mut self, arg: &AlwaysFfClock) {
        before!(self, always_ff_clock, arg);
        self.hierarchical_identifier(&arg.hierarchical_identifier);
        after!(self, always_ff_clock, arg);
    }

    /// Semantic action for non-terminal 'AlwaysFfReset'
    fn always_ff_reset(&mut self, arg: &AlwaysFfReset) {
        before!(self, always_ff_reset, arg);
        self.hierarchical_identifier(&arg.hierarchical_identifier);
        after!(self, always_ff_reset, arg);
    }

    /// Semantic action for non-terminal 'AlwaysCombDeclaration'
    fn always_comb_declaration(&mut self, arg: &AlwaysCombDeclaration) {
        before!(self, always_comb_declaration, arg);
        self.always_comb(&arg.always_comb);
        self.statement_block(&arg.statement_block);
        after!(self, always_comb_declaration, arg);
    }

    /// Semantic action for non-terminal 'AssignDeclaration'
    fn assign_declaration(&mut self, arg: &AssignDeclaration) {
        before!(self, assign_declaration, arg);
        self.assign(&arg.assign);
        self.hierarchical_identifier(&arg.hierarchical_identifier);
        self.equ(&arg.equ);
        self.expression(&arg.expression);
        self.semicolon(&arg.semicolon);
        after!(self, assign_declaration, arg);
    }

    /// Semantic action for non-terminal 'ModportDeclaration'
    fn modport_declaration(&mut self, arg: &ModportDeclaration) {
        before!(self, modport_declaration, arg);
        self.modport(&arg.modport);
        self.identifier(&arg.identifier);
        self.l_brace(&arg.l_brace);
        self.modport_list(&arg.modport_list);
        self.r_brace(&arg.r_brace);
        after!(self, modport_declaration, arg);
    }

    /// Semantic action for non-terminal 'ModportList'
    fn modport_list(&mut self, arg: &ModportList) {
        before!(self, modport_list, arg);
        self.modport_group(&arg.modport_group);
        for x in &arg.modport_list_list {
            self.comma(&x.comma);
            self.modport_group(&x.modport_group);
        }
        if let Some(ref x) = arg.modport_list_opt {
            self.comma(&x.comma);
        }
        after!(self, modport_list, arg);
    }

    /// Semantic action for non-terminal 'ModportGroup'
    fn modport_group(&mut self, arg: &ModportGroup) {
        before!(self, modport_group, arg);
        for x in &arg.modport_group_list {
            self.attribute(&x.attribute);
        }
        match &*arg.modport_group_group {
            ModportGroupGroup::LBraceModportListRBrace(x) => {
                self.l_brace(&x.l_brace);
                self.modport_list(&x.modport_list);
                self.r_brace(&x.r_brace);
            }
            ModportGroupGroup::ModportItem(x) => self.modport_item(&x.modport_item),
        }
        after!(self, modport_group, arg);
    }

    /// Semantic action for non-terminal 'ModportItem'
    fn modport_item(&mut self, arg: &ModportItem) {
        before!(self, modport_item, arg);
        self.identifier(&arg.identifier);
        self.colon(&arg.colon);
        self.direction(&arg.direction);
        after!(self, modport_item, arg);
    }

    /// Semantic action for non-terminal 'EnumDeclaration'
    fn enum_declaration(&mut self, arg: &EnumDeclaration) {
        before!(self, enum_declaration, arg);
        self.r#enum(&arg.r#enum);
        self.identifier(&arg.identifier);
        if let Some(ref x) = arg.enum_declaration_opt {
            self.colon(&x.colon);
            self.scalar_type(&x.scalar_type);
        }
        self.l_brace(&arg.l_brace);
        self.enum_list(&arg.enum_list);
        self.r_brace(&arg.r_brace);
        after!(self, enum_declaration, arg);
    }

    /// Semantic action for non-terminal 'EnumList'
    fn enum_list(&mut self, arg: &EnumList) {
        before!(self, enum_list, arg);
        self.enum_group(&arg.enum_group);
        for x in &arg.enum_list_list {
            self.comma(&x.comma);
            self.enum_group(&x.enum_group);
        }
        if let Some(ref x) = arg.enum_list_opt {
            self.comma(&x.comma);
        }
        after!(self, enum_list, arg);
    }

    /// Semantic action for non-terminal 'EnumGroup'
    fn enum_group(&mut self, arg: &EnumGroup) {
        before!(self, enum_group, arg);
        for x in &arg.enum_group_list {
            self.attribute(&x.attribute);
        }
        match &*arg.enum_group_group {
            EnumGroupGroup::LBraceEnumListRBrace(x) => {
                self.l_brace(&x.l_brace);
                self.enum_list(&x.enum_list);
                self.r_brace(&x.r_brace);
            }
            EnumGroupGroup::EnumItem(x) => self.enum_item(&x.enum_item),
        }
        after!(self, enum_group, arg);
    }

    /// Semantic action for non-terminal 'EnumItem'
    fn enum_item(&mut self, arg: &EnumItem) {
        before!(self, enum_item, arg);
        self.identifier(&arg.identifier);
        if let Some(ref x) = arg.enum_item_opt {
            self.equ(&x.equ);
            self.expression(&x.expression);
        }
        after!(self, enum_item, arg);
    }

    /// Semantic action for non-terminal 'StructUnionDeclaration'
    fn struct_union_declaration(&mut self, arg: &StructUnionDeclaration) {
        before!(self, struct_union_declaration, arg);
        self.struct_union(&arg.struct_union);
        self.identifier(&arg.identifier);
        if let Some(ref x) = arg.struct_union_declaration_opt {
            self.with_generic_parameter(&x.with_generic_parameter);
        }
        self.l_brace(&arg.l_brace);
        self.struct_union_list(&arg.struct_union_list);
        self.r_brace(&arg.r_brace);
        after!(self, struct_union_declaration, arg);
    }

    fn struct_union(&mut self, arg: &StructUnion) {
        before!(self, struct_union, arg);
        match arg {
            StructUnion::Struct(x) => {
                self.r#struct(&x.r#struct);
            }
            StructUnion::Union(x) => {
                self.union(&x.union);
            }
        }
        after!(self, struct_union, arg);
    }

    /// Semantic action for non-terminal 'StructList'
    fn struct_union_list(&mut self, arg: &StructUnionList) {
        before!(self, struct_union_list, arg);
        self.struct_union_group(&arg.struct_union_group);
        for x in &arg.struct_union_list_list {
            self.comma(&x.comma);
            self.struct_union_group(&x.struct_union_group);
        }
        if let Some(ref x) = arg.struct_union_list_opt {
            self.comma(&x.comma);
        }
        after!(self, struct_union_list, arg);
    }

    /// Semantic action for non-terminal 'struct_unionUnionGroup'
    fn struct_union_group(&mut self, arg: &StructUnionGroup) {
        before!(self, struct_union_group, arg);
        for x in &arg.struct_union_group_list {
            self.attribute(&x.attribute);
        }
        match &*arg.struct_union_group_group {
            StructUnionGroupGroup::LBraceStructUnionListRBrace(x) => {
                self.l_brace(&x.l_brace);
                self.struct_union_list(&x.struct_union_list);
                self.r_brace(&x.r_brace);
            }
            StructUnionGroupGroup::StructUnionItem(x) => {
                self.struct_union_item(&x.struct_union_item)
            }
        }
        after!(self, struct_union_group, arg);
    }

    /// Semantic action for non-terminal 'struct_unionUnionItem'
    fn struct_union_item(&mut self, arg: &StructUnionItem) {
        before!(self, struct_union_item, arg);
        self.identifier(&arg.identifier);
        self.colon(&arg.colon);
        self.scalar_type(&arg.scalar_type);
        after!(self, struct_union_item, arg);
    }

    /// Semantic action for non-terminal 'InitialDeclaration'
    fn initial_declaration(&mut self, arg: &InitialDeclaration) {
        before!(self, initial_declaration, arg);
        self.initial(&arg.initial);
        self.statement_block(&arg.statement_block);
        after!(self, initial_declaration, arg);
    }

    /// Semantic action for non-terminal 'FinalDeclaration'
    fn final_declaration(&mut self, arg: &FinalDeclaration) {
        before!(self, final_declaration, arg);
        self.r#final(&arg.r#final);
        self.statement_block(&arg.statement_block);
        after!(self, final_declaration, arg);
    }

    /// Semantic action for non-terminal 'InstDeclaration'
    fn inst_declaration(&mut self, arg: &InstDeclaration) {
        before!(self, inst_declaration, arg);
        self.inst(&arg.inst);
        self.identifier(&arg.identifier);
        self.colon(&arg.colon);
        self.scoped_identifier(&arg.scoped_identifier);
        if let Some(ref x) = arg.inst_declaration_opt {
            self.array(&x.array);
        }
        if let Some(ref x) = arg.inst_declaration_opt0 {
            self.inst_parameter(&x.inst_parameter);
        }
        if let Some(ref x) = arg.inst_declaration_opt1 {
            self.l_paren(&x.l_paren);
            if let Some(ref x) = x.inst_declaration_opt2 {
                self.inst_port_list(&x.inst_port_list);
            }
            self.r_paren(&x.r_paren);
        }
        self.semicolon(&arg.semicolon);
        after!(self, inst_declaration, arg);
    }

    /// Semantic action for non-terminal 'InstParameter'
    fn inst_parameter(&mut self, arg: &InstParameter) {
        before!(self, inst_parameter, arg);
        self.hash(&arg.hash);
        self.l_paren(&arg.l_paren);
        if let Some(ref x) = arg.inst_parameter_opt {
            self.inst_parameter_list(&x.inst_parameter_list);
        }
        self.r_paren(&arg.r_paren);
        after!(self, inst_parameter, arg);
    }

    /// Semantic action for non-terminal 'InstParameterList'
    fn inst_parameter_list(&mut self, arg: &InstParameterList) {
        before!(self, inst_parameter_list, arg);
        self.inst_parameter_group(&arg.inst_parameter_group);
        for x in &arg.inst_parameter_list_list {
            self.comma(&x.comma);
            self.inst_parameter_group(&x.inst_parameter_group);
        }
        if let Some(ref x) = arg.inst_parameter_list_opt {
            self.comma(&x.comma);
        }
        after!(self, inst_parameter_list, arg);
    }

    /// Semantic action for non-terminal 'InstParameterGroup'
    fn inst_parameter_group(&mut self, arg: &InstParameterGroup) {
        before!(self, inst_parameter_group, arg);
        for x in &arg.inst_parameter_group_list {
            self.attribute(&x.attribute);
        }
        match &*arg.inst_parameter_group_group {
            InstParameterGroupGroup::LBraceInstParameterListRBrace(x) => {
                self.l_brace(&x.l_brace);
                self.inst_parameter_list(&x.inst_parameter_list);
                self.r_brace(&x.r_brace);
            }
            InstParameterGroupGroup::InstParameterItem(x) => {
                self.inst_parameter_item(&x.inst_parameter_item)
            }
        }
        after!(self, inst_parameter_group, arg);
    }

    /// Semantic action for non-terminal 'InstParameterItem'
    fn inst_parameter_item(&mut self, arg: &InstParameterItem) {
        before!(self, inst_parameter_item, arg);
        self.identifier(&arg.identifier);
        if let Some(ref x) = arg.inst_parameter_item_opt {
            self.colon(&x.colon);
            self.expression(&x.expression);
        }
        after!(self, inst_parameter_item, arg);
    }

    /// Semantic action for non-terminal 'InstPortList'
    fn inst_port_list(&mut self, arg: &InstPortList) {
        before!(self, inst_port_list, arg);
        self.inst_port_group(&arg.inst_port_group);
        for x in &arg.inst_port_list_list {
            self.comma(&x.comma);
            self.inst_port_group(&x.inst_port_group);
        }
        if let Some(ref x) = arg.inst_port_list_opt {
            self.comma(&x.comma);
        }
        after!(self, inst_port_list, arg);
    }

    /// Semantic action for non-terminal 'InstPortGroup'
    fn inst_port_group(&mut self, arg: &InstPortGroup) {
        before!(self, inst_port_group, arg);
        for x in &arg.inst_port_group_list {
            self.attribute(&x.attribute);
        }
        match &*arg.inst_port_group_group {
            InstPortGroupGroup::LBraceInstPortListRBrace(x) => {
                self.l_brace(&x.l_brace);
                self.inst_port_list(&x.inst_port_list);
                self.r_brace(&x.r_brace);
            }
            InstPortGroupGroup::InstPortItem(x) => self.inst_port_item(&x.inst_port_item),
        }
        after!(self, inst_port_group, arg);
    }

    /// Semantic action for non-terminal 'InstPortItem'
    fn inst_port_item(&mut self, arg: &InstPortItem) {
        before!(self, inst_port_item, arg);
        self.identifier(&arg.identifier);
        if let Some(ref x) = arg.inst_port_item_opt {
            self.colon(&x.colon);
            self.expression(&x.expression);
        }
        after!(self, inst_port_item, arg);
    }

    /// Semantic action for non-terminal 'WithParameter'
    fn with_parameter(&mut self, arg: &WithParameter) {
        before!(self, with_parameter, arg);
        self.hash(&arg.hash);
        self.l_paren(&arg.l_paren);
        if let Some(ref x) = arg.with_parameter_opt {
            self.with_parameter_list(&x.with_parameter_list);
        }
        self.r_paren(&arg.r_paren);
        after!(self, with_parameter, arg);
    }

    /// Semantic action for non-terminal 'WithParameterList'
    fn with_parameter_list(&mut self, arg: &WithParameterList) {
        before!(self, with_parameter_list, arg);
        self.with_parameter_group(&arg.with_parameter_group);
        for x in &arg.with_parameter_list_list {
            self.comma(&x.comma);
            self.with_parameter_group(&x.with_parameter_group);
        }
        if let Some(ref x) = arg.with_parameter_list_opt {
            self.comma(&x.comma);
        }
        after!(self, with_parameter_list, arg);
    }

    /// Semantic action for non-terminal 'WithParameterGroup'
    fn with_parameter_group(&mut self, arg: &WithParameterGroup) {
        before!(self, with_parameter_group, arg);
        for x in &arg.with_parameter_group_list {
            self.attribute(&x.attribute);
        }
        match &*arg.with_parameter_group_group {
            WithParameterGroupGroup::LBraceWithParameterListRBrace(x) => {
                self.l_brace(&x.l_brace);
                self.with_parameter_list(&x.with_parameter_list);
                self.r_brace(&x.r_brace);
            }
            WithParameterGroupGroup::WithParameterItem(x) => {
                self.with_parameter_item(&x.with_parameter_item)
            }
        }
        after!(self, with_parameter_group, arg);
    }

    /// Semantic action for non-terminal 'WithParameterItem'
    fn with_parameter_item(&mut self, arg: &WithParameterItem) {
        before!(self, with_parameter_item, arg);
        match &*arg.with_parameter_item_group {
            WithParameterItemGroup::Param(x) => self.param(&x.param),
            WithParameterItemGroup::Const(x) => self.r#const(&x.r#const),
        };
        self.identifier(&arg.identifier);
        self.colon(&arg.colon);
        match &*arg.with_parameter_item_group0 {
            WithParameterItemGroup0::ArrayType(x) => {
                self.array_type(&x.array_type);
            }
            WithParameterItemGroup0::Type(x) => {
                self.r#type(&x.r#type);
            }
        }
        self.equ(&arg.equ);
        self.expression(&arg.expression);
        after!(self, with_parameter_item, arg);
    }

    /// Semantic action for non-terminal 'GenericBound'
    fn generic_bound(&mut self, arg: &GenericBound) {
        before!(self, generic_bound, arg);
        match arg {
            GenericBound::Const(x) => self.r#const(&x.r#const),
            GenericBound::Type(x) => self.r#type(&x.r#type),
            GenericBound::ScopedIdentifier(x) => self.scoped_identifier(&x.scoped_identifier),
        }
        after!(self, generic_bound, arg);
    }

    /// Semantic action for non-terminal 'WithGenericParameter'
    fn with_generic_parameter(&mut self, arg: &WithGenericParameter) {
        before!(self, with_generic_parameter, arg);
        self.colon_colon_l_angle(&arg.colon_colon_l_angle);
        self.with_generic_parameter_list(&arg.with_generic_parameter_list);
        self.r_angle(&arg.r_angle);
        after!(self, with_generic_parameter, arg);
    }

    /// Semantic action for non-terminal 'WithGenericParameterList'
    fn with_generic_parameter_list(&mut self, arg: &WithGenericParameterList) {
        before!(self, with_generic_parameter_list, arg);
        self.with_generic_parameter_item(&arg.with_generic_parameter_item);
        for x in &arg.with_generic_parameter_list_list {
            self.comma(&x.comma);
            self.with_generic_parameter_item(&x.with_generic_parameter_item);
        }
        if let Some(ref x) = arg.with_generic_parameter_list_opt {
            self.comma(&x.comma);
        }
        after!(self, with_generic_parameter_list, arg);
    }

    /// Semantic action for non-terminal 'WithGenericParameterItem'
    fn with_generic_parameter_item(&mut self, arg: &WithGenericParameterItem) {
        before!(self, with_generic_parameter_item, arg);
        self.identifier(&arg.identifier);
        self.colon(&arg.colon);
        self.generic_bound(&arg.generic_bound);
        if let Some(ref x) = arg.with_generic_parameter_item_opt {
            self.equ(&x.equ);
            self.with_generic_argument_item(&x.with_generic_argument_item);
        }
        after!(self, with_generic_parameter_item, arg);
    }

    /// Semantic action for non-terminal 'WithGenericArgument'
    fn with_generic_argument(&mut self, arg: &WithGenericArgument) {
        before!(self, with_generic_argument, arg);
        self.colon_colon_l_angle(&arg.colon_colon_l_angle);
        if let Some(x) = &arg.with_generic_argument_opt {
            self.with_generic_argument_list(&x.with_generic_argument_list);
        }
        self.r_angle(&arg.r_angle);
        after!(self, with_generic_argument, arg);
    }

    /// Semantic action for non-terminal 'WithGenericArgumentList'
    fn with_generic_argument_list(&mut self, arg: &WithGenericArgumentList) {
        before!(self, with_generic_argument_list, arg);
        self.with_generic_argument_item(&arg.with_generic_argument_item);
        for x in &arg.with_generic_argument_list_list {
            self.comma(&x.comma);
            self.with_generic_argument_item(&x.with_generic_argument_item);
        }
        if let Some(ref x) = arg.with_generic_argument_list_opt {
            self.comma(&x.comma);
        }
        after!(self, with_generic_argument_list, arg);
    }

    /// Semantic action for non-terminal 'WithGenericArgumentItem'
    fn with_generic_argument_item(&mut self, arg: &WithGenericArgumentItem) {
        before!(self, with_generic_argument_item, arg);
        match arg {
            WithGenericArgumentItem::ScopedIdentifier(x) => {
                self.scoped_identifier(&x.scoped_identifier);
            }
            WithGenericArgumentItem::Number(x) => {
                self.number(&x.number);
            }
        }
        after!(self, with_generic_argument_item, arg);
    }

    /// Semantic action for non-terminal 'PortDeclaration'
    fn port_declaration(&mut self, arg: &PortDeclaration) {
        before!(self, port_declaration, arg);
        self.l_paren(&arg.l_paren);
        if let Some(ref x) = arg.port_declaration_opt {
            self.port_declaration_list(&x.port_declaration_list);
        }
        self.r_paren(&arg.r_paren);
        after!(self, port_declaration, arg);
    }

    /// Semantic action for non-terminal 'PortDeclarationList'
    fn port_declaration_list(&mut self, arg: &PortDeclarationList) {
        before!(self, port_declaration_list, arg);
        self.port_declaration_group(&arg.port_declaration_group);
        for x in &arg.port_declaration_list_list {
            self.comma(&x.comma);
            self.port_declaration_group(&x.port_declaration_group);
        }
        if let Some(ref x) = arg.port_declaration_list_opt {
            self.comma(&x.comma);
        }
        after!(self, port_declaration_list, arg);
    }

    /// Semantic action for non-terminal 'PortDeclarationGroup'
    fn port_declaration_group(&mut self, arg: &PortDeclarationGroup) {
        before!(self, port_declaration_group, arg);
        for x in &arg.port_declaration_group_list {
            self.attribute(&x.attribute);
        }
        match &*arg.port_declaration_group_group {
            PortDeclarationGroupGroup::LBracePortDeclarationListRBrace(x) => {
                self.l_brace(&x.l_brace);
                self.port_declaration_list(&x.port_declaration_list);
                self.r_brace(&x.r_brace);
            }
            PortDeclarationGroupGroup::PortDeclarationItem(x) => {
                self.port_declaration_item(&x.port_declaration_item)
            }
        }
        after!(self, port_declaration_group, arg);
    }

    /// Semantic action for non-terminal 'PortDeclarationItem'
    fn port_declaration_item(&mut self, arg: &PortDeclarationItem) {
        before!(self, port_declaration_item, arg);
        self.identifier(&arg.identifier);
        self.colon(&arg.colon);
        match &*arg.port_declaration_item_group {
            PortDeclarationItemGroup::PortTypeConcrete(x) => {
                self.port_type_concrete(&x.port_type_concrete);
            }
            PortDeclarationItemGroup::PortTypeAbstract(x) => {
                self.port_type_abstract(&x.port_type_abstract);
            }
        }
        after!(self, port_declaration_item, arg);
    }

    /// Semantic action for non-terminal 'PortTypeConcrete'
    fn port_type_concrete(&mut self, arg: &PortTypeConcrete) {
        before!(self, port_type_concrete, arg);
        if let Some(ref x) = arg.port_type_concrete_opt {
            self.clock_domain(&x.clock_domain);
        }
        self.direction(&arg.direction);
        self.array_type(&arg.array_type);
        if let Some(ref x) = arg.port_type_concrete_opt0 {
            self.equ(&x.equ);
            self.port_default_value(&x.port_default_value);
        }
        after!(self, port_type_concrete, arg);
    }

    /// Semantic action for non-terminal 'PortDefaultValue'
    fn port_default_value(&mut self, arg: &PortDefaultValue) {
        before!(self, port_default_value, arg);
        self.expression(&arg.expression);
        after!(self, port_default_value, arg);
    }

    /// Semantic action for non-terminal 'PortTypeAbstract'
    fn port_type_abstract(&mut self, arg: &PortTypeAbstract) {
        if let Some(ref x) = arg.port_type_abstract_opt {
            self.clock_domain(&x.clock_domain);
        }
        self.interface(&arg.interface);
        if let Some(ref x) = arg.port_type_abstract_opt0 {
            self.colon_colon(&x.colon_colon);
            self.identifier(&x.identifier);
        }
        if let Some(ref x) = arg.port_type_abstract_opt1 {
            self.array(&x.array);
        }
    }

    /// Semantic action for non-terminal 'Direction'
    fn direction(&mut self, arg: &Direction) {
        before!(self, direction, arg);
        match arg {
            Direction::Input(x) => self.input(&x.input),
            Direction::Output(x) => self.output(&x.output),
            Direction::Inout(x) => self.inout(&x.inout),
            Direction::Ref(x) => self.r#ref(&x.r#ref),
            Direction::Modport(x) => self.modport(&x.modport),
            Direction::Import(x) => self.import(&x.import),
        };
        after!(self, direction, arg);
    }

    /// Semantic action for non-terminal 'FunctionDeclaration'
    fn function_declaration(&mut self, arg: &FunctionDeclaration) {
        before!(self, function_declaration, arg);
        self.function(&arg.function);
        self.identifier(&arg.identifier);
        if let Some(ref x) = arg.function_declaration_opt {
            self.with_generic_parameter(&x.with_generic_parameter);
        }
        if let Some(ref x) = arg.function_declaration_opt0 {
            self.port_declaration(&x.port_declaration);
        }
        if let Some(ref x) = arg.function_declaration_opt1 {
            self.minus_g_t(&x.minus_g_t);
            self.scalar_type(&x.scalar_type);
        }
        self.statement_block(&arg.statement_block);
        after!(self, function_declaration, arg);
    }

    /// Semantic action for non-terminal 'ImportDeclaration'
    fn import_declaration(&mut self, arg: &ImportDeclaration) {
        before!(self, import_declaration, arg);
        self.import(&arg.import);
        self.scoped_identifier(&arg.scoped_identifier);
        if let Some(ref x) = arg.import_declaration_opt {
            self.colon_colon(&x.colon_colon);
            self.star(&x.star);
        }
        self.semicolon(&arg.semicolon);
        after!(self, import_declaration, arg);
    }

    /// Semantic action for non-terminal 'ExportDeclaration'
    fn export_declaration(&mut self, arg: &ExportDeclaration) {
        before!(self, export_declaration, arg);
        self.export(&arg.export);
        match &*arg.export_declaration_group {
            ExportDeclarationGroup::Star(x) => self.star(&x.star),
            ExportDeclarationGroup::ScopedIdentifierExportDeclarationOpt(x) => {
                self.scoped_identifier(&x.scoped_identifier);
                if let Some(ref x) = x.export_declaration_opt {
                    self.colon_colon(&x.colon_colon);
                    self.star(&x.star);
                }
            }
        }
        self.semicolon(&arg.semicolon);
        after!(self, export_declaration, arg);
    }

    /// Semantic action for non-terminal 'UnsafeBlock'
    fn unsafe_block(&mut self, arg: &UnsafeBlock) {
        before!(self, unsafe_block, arg);
        self.r#unsafe(&arg.r#unsafe);
        self.l_paren(&arg.l_paren);
        self.identifier(&arg.identifier);
        self.r_paren(&arg.r_paren);
        self.l_brace(&arg.l_brace);
        for x in &arg.unsafe_block_list {
            self.generate_group(&x.generate_group);
        }
        self.r_brace(&arg.r_brace);
        after!(self, unsafe_block, arg);
    }

    /// Semantic action for non-terminal 'ModuleDeclaration'
    fn module_declaration(&mut self, arg: &ModuleDeclaration) {
        before!(self, module_declaration, arg);
        if let Some(ref x) = arg.module_declaration_opt {
            self.r#pub(&x.r#pub);
        }
        self.module(&arg.module);
        self.identifier(&arg.identifier);
        if let Some(ref x) = arg.module_declaration_opt0 {
            self.with_generic_parameter(&x.with_generic_parameter);
        }
        if let Some(ref x) = arg.module_declaration_opt1 {
            self.r#for(&x.r#for);
            self.scoped_identifier(&x.scoped_identifier);
        }
        if let Some(ref x) = arg.module_declaration_opt2 {
            self.with_parameter(&x.with_parameter);
        }
        if let Some(ref x) = arg.module_declaration_opt3 {
            self.port_declaration(&x.port_declaration);
        }
        self.l_brace(&arg.l_brace);
        for x in &arg.module_declaration_list {
            self.module_group(&x.module_group);
        }
        self.r_brace(&arg.r_brace);
        after!(self, module_declaration, arg);
    }

    /// Semantic action for non-terminal 'ModuleGroup'
    fn module_group(&mut self, arg: &ModuleGroup) {
        before!(self, module_group, arg);
        for x in &arg.module_group_list {
            self.attribute(&x.attribute);
        }
        match &*arg.module_group_group {
            ModuleGroupGroup::LBraceModuleGroupGroupListRBrace(x) => {
                self.l_brace(&x.l_brace);
                for x in &x.module_group_group_list {
                    self.module_group(&x.module_group);
                }
                self.r_brace(&x.r_brace);
            }
            ModuleGroupGroup::ModuleItem(x) => {
                self.module_item(&x.module_item);
            }
        }
        after!(self, module_group, arg);
    }

    /// Semantic action for non-terminal 'ModuleItem'
    fn module_item(&mut self, arg: &ModuleItem) {
        before!(self, module_item, arg);
        self.generate_item(&arg.generate_item);
        after!(self, module_item, arg);
    }

    /// Semantic action for non-terminal 'InterfaceDeclaration'
    fn interface_declaration(&mut self, arg: &InterfaceDeclaration) {
        before!(self, interface_declaration, arg);
        if let Some(ref x) = arg.interface_declaration_opt {
            self.r#pub(&x.r#pub);
        }
        self.interface(&arg.interface);
        self.identifier(&arg.identifier);
        if let Some(ref x) = arg.interface_declaration_opt0 {
            self.with_generic_parameter(&x.with_generic_parameter);
        }
        if let Some(ref x) = arg.interface_declaration_opt1 {
            self.with_parameter(&x.with_parameter);
        }
        self.l_brace(&arg.l_brace);
        for x in &arg.interface_declaration_list {
            self.interface_group(&x.interface_group);
        }
        self.r_brace(&arg.r_brace);
        after!(self, interface_declaration, arg);
    }

    /// Semantic action for non-terminal 'InterfaceGroup'
    fn interface_group(&mut self, arg: &InterfaceGroup) {
        before!(self, interface_group, arg);
        for x in &arg.interface_group_list {
            self.attribute(&x.attribute);
        }
        match &*arg.interface_group_group {
            InterfaceGroupGroup::LBraceInterfaceGroupGroupListRBrace(x) => {
                self.l_brace(&x.l_brace);
                for x in &x.interface_group_group_list {
                    self.interface_group(&x.interface_group);
                }
                self.r_brace(&x.r_brace);
            }
            InterfaceGroupGroup::InterfaceItem(x) => {
                self.interface_item(&x.interface_item);
            }
        }
        after!(self, interface_group, arg);
    }

    /// Semantic action for non-terminal 'InterfaceItem'
    fn interface_item(&mut self, arg: &InterfaceItem) {
        before!(self, interface_item, arg);
        match arg {
            InterfaceItem::GenerateItem(x) => self.generate_item(&x.generate_item),
            InterfaceItem::ModportDeclaration(x) => {
                self.modport_declaration(&x.modport_declaration)
            }
        }
        after!(self, interface_item, arg);
    }

    /// Semantic action for non-terminal 'GenerateIfDeclaration'
    fn generate_if_declaration(&mut self, arg: &GenerateIfDeclaration) {
        before!(self, generate_if_declaration, arg);
        self.r#if(&arg.r#if);
        self.expression(&arg.expression);
        self.generate_named_block(&arg.generate_named_block);
        for x in &arg.generate_if_declaration_list {
            self.r#else(&x.r#else);
            self.r#if(&x.r#if);
            self.expression(&x.expression);
            self.generate_optional_named_block(&x.generate_optional_named_block);
        }
        if let Some(ref x) = arg.generate_if_declaration_opt {
            self.r#else(&x.r#else);
            self.generate_optional_named_block(&x.generate_optional_named_block);
        }
        after!(self, generate_if_declaration, arg);
    }

    /// Semantic action for non-terminal 'GenerateForDeclaration'
    fn generate_for_declaration(&mut self, arg: &GenerateForDeclaration) {
        before!(self, generate_for_declaration, arg);
        self.r#for(&arg.r#for);
        self.identifier(&arg.identifier);
        self.r#in(&arg.r#in);
        self.range(&arg.range);
        if let Some(ref x) = arg.generate_for_declaration_opt {
            self.step(&x.step);
            self.assignment_operator(&x.assignment_operator);
            self.expression(&x.expression);
        }
        self.generate_named_block(&arg.generate_named_block);
        after!(self, generate_for_declaration, arg);
    }

    /// Semantic action for non-terminal 'GenerateBlockDeclaration'
    fn generate_block_declaration(&mut self, arg: &GenerateBlockDeclaration) {
        before!(self, generate_block_declaration, arg);
        self.generate_named_block(&arg.generate_named_block);
        after!(self, generate_block_declaration, arg);
    }

    /// Semantic action for non-terminal 'GenerateNamedBlock'
    fn generate_named_block(&mut self, arg: &GenerateNamedBlock) {
        before!(self, generate_named_block, arg);
        self.colon(&arg.colon);
        self.identifier(&arg.identifier);
        self.l_brace(&arg.l_brace);
        for x in &arg.generate_named_block_list {
            self.generate_group(&x.generate_group);
        }
        self.r_brace(&arg.r_brace);
        after!(self, generate_named_block, arg);
    }

    /// Semantic action for non-terminal 'GenerateOptionalNamedBlock'
    fn generate_optional_named_block(&mut self, arg: &GenerateOptionalNamedBlock) {
        before!(self, generate_optional_named_block, arg);
        if let Some(ref x) = arg.generate_optional_named_block_opt {
            self.colon(&x.colon);
            self.identifier(&x.identifier);
        }
        self.l_brace(&arg.l_brace);
        for x in &arg.generate_optional_named_block_list {
            self.generate_group(&x.generate_group);
        }
        self.r_brace(&arg.r_brace);
        after!(self, generate_optional_named_block, arg);
    }

    /// Semantic action for non-terminal 'GenerateGroup'
    fn generate_group(&mut self, arg: &GenerateGroup) {
        before!(self, generate_group, arg);
        for x in &arg.generate_group_list {
            self.attribute(&x.attribute);
        }
        match &*arg.generate_group_group {
            GenerateGroupGroup::LBraceGenerateGroupGroupListRBrace(x) => {
                self.l_brace(&x.l_brace);
                for x in &x.generate_group_group_list {
                    self.generate_group(&x.generate_group);
                }
                self.r_brace(&x.r_brace);
            }
            GenerateGroupGroup::GenerateItem(x) => self.generate_item(&x.generate_item),
        }
        after!(self, generate_group, arg);
    }

    /// Semantic action for non-terminal 'GenerateItem'
    fn generate_item(&mut self, arg: &GenerateItem) {
        before!(self, generate_item, arg);
        match arg {
            GenerateItem::LetDeclaration(x) => self.let_declaration(&x.let_declaration),
            GenerateItem::VarDeclaration(x) => self.var_declaration(&x.var_declaration),
            GenerateItem::InstDeclaration(x) => self.inst_declaration(&x.inst_declaration),
            GenerateItem::ConstDeclaration(x) => self.const_declaration(&x.const_declaration),
            GenerateItem::AlwaysFfDeclaration(x) => {
                self.always_ff_declaration(&x.always_ff_declaration)
            }
            GenerateItem::AlwaysCombDeclaration(x) => {
                self.always_comb_declaration(&x.always_comb_declaration)
            }
            GenerateItem::AssignDeclaration(x) => self.assign_declaration(&x.assign_declaration),
            GenerateItem::FunctionDeclaration(x) => {
                self.function_declaration(&x.function_declaration)
            }
            GenerateItem::GenerateIfDeclaration(x) => {
                self.generate_if_declaration(&x.generate_if_declaration)
            }
            GenerateItem::GenerateForDeclaration(x) => {
                self.generate_for_declaration(&x.generate_for_declaration)
            }
            GenerateItem::GenerateBlockDeclaration(x) => {
                self.generate_block_declaration(&x.generate_block_declaration)
            }
            GenerateItem::TypeDefDeclaration(x) => {
                self.type_def_declaration(&x.type_def_declaration)
            }
            GenerateItem::EnumDeclaration(x) => self.enum_declaration(&x.enum_declaration),
            GenerateItem::StructUnionDeclaration(x) => {
                self.struct_union_declaration(&x.struct_union_declaration)
            }
            GenerateItem::ImportDeclaration(x) => self.import_declaration(&x.import_declaration),
            GenerateItem::InitialDeclaration(x) => self.initial_declaration(&x.initial_declaration),
            GenerateItem::FinalDeclaration(x) => self.final_declaration(&x.final_declaration),
            GenerateItem::UnsafeBlock(x) => self.unsafe_block(&x.unsafe_block),
        };
        after!(self, generate_item, arg);
    }

    /// Semantic action for non-terminal 'PackageDeclaration'
    fn package_declaration(&mut self, arg: &PackageDeclaration) {
        before!(self, package_declaration, arg);
        if let Some(ref x) = arg.package_declaration_opt {
            self.r#pub(&x.r#pub);
        }
        self.package(&arg.package);
        self.identifier(&arg.identifier);
        if let Some(ref x) = arg.package_declaration_opt0 {
            self.with_generic_parameter(&x.with_generic_parameter);
        }
        self.l_brace(&arg.l_brace);
        for x in &arg.package_declaration_list {
            self.package_group(&x.package_group);
        }
        self.r_brace(&arg.r_brace);
        after!(self, package_declaration, arg);
    }

    /// Semantic action for non-terminal 'PackageGroup'
    fn package_group(&mut self, arg: &PackageGroup) {
        before!(self, package_group, arg);
        for x in &arg.package_group_list {
            self.attribute(&x.attribute);
        }
        match &*arg.package_group_group {
            PackageGroupGroup::LBracePackageGroupGroupListRBrace(x) => {
                self.l_brace(&x.l_brace);
                for x in &x.package_group_group_list {
                    self.package_group(&x.package_group);
                }
                self.r_brace(&x.r_brace);
            }
            PackageGroupGroup::PackageItem(x) => self.package_item(&x.package_item),
        }
        after!(self, package_group, arg);
    }

    /// Semantic action for non-terminal 'PackageItem'
    fn package_item(&mut self, arg: &PackageItem) {
        before!(self, package_item, arg);
        match arg {
            PackageItem::VarDeclaration(x) => self.var_declaration(&x.var_declaration),
            PackageItem::ConstDeclaration(x) => self.const_declaration(&x.const_declaration),
            PackageItem::TypeDefDeclaration(x) => {
                self.type_def_declaration(&x.type_def_declaration)
            }
            PackageItem::EnumDeclaration(x) => self.enum_declaration(&x.enum_declaration),
            PackageItem::StructUnionDeclaration(x) => {
                self.struct_union_declaration(&x.struct_union_declaration)
            }
            PackageItem::FunctionDeclaration(x) => {
                self.function_declaration(&x.function_declaration)
            }
            PackageItem::ImportDeclaration(x) => self.import_declaration(&x.import_declaration),
            PackageItem::ExportDeclaration(x) => self.export_declaration(&x.export_declaration),
        }
        after!(self, package_item, arg);
    }

    /// Semantic action for non-terminal 'ProtoModuleDeclaration'
    fn proto_module_declaration(&mut self, arg: &ProtoModuleDeclaration) {
        before!(self, proto_module_declaration, arg);
        if let Some(ref x) = arg.proto_module_declaration_opt {
            self.r#pub(&x.r#pub);
        }
        self.module(&arg.module);
        self.identifier(&arg.identifier);
        if let Some(ref x) = arg.proto_module_declaration_opt0 {
            self.with_parameter(&x.with_parameter);
        }
        if let Some(ref x) = arg.proto_module_declaration_opt1 {
            self.port_declaration(&x.port_declaration);
        }
        self.semicolon(&arg.semicolon);
        after!(self, proto_module_declaration, arg);
    }

    /// Semantic action for non-terminal 'EmbedDeclaration'
    fn embed_declaration(&mut self, arg: &EmbedDeclaration) {
        before!(self, embed_declaration, arg);
        self.embed(&arg.embed);
        self.l_paren(&arg.l_paren);
        self.identifier(&arg.identifier);
        self.r_paren(&arg.r_paren);
        self.identifier(&arg.identifier0);
        self.embed_content(&arg.embed_content);
        after!(self, embed_declaration, arg);
    }

    /// Semantic action for non-terminal 'EmbedContent'
    fn embed_content(&mut self, arg: &EmbedContent) {
        before!(self, embed_content, arg);
        self.veryl_token(&arg.embed_content_token);
        after!(self, embed_content, arg);
    }

    /// Semantic action for non-terminal 'IncludeDeclaration'
    fn include_declaration(&mut self, arg: &IncludeDeclaration) {
        before!(self, include_declaration, arg);
        self.include(&arg.include);
        self.l_paren(&arg.l_paren);
        self.identifier(&arg.identifier);
        self.comma(&arg.comma);
        self.string_literal(&arg.string_literal);
        self.r_paren(&arg.r_paren);
        self.semicolon(&arg.semicolon);
        after!(self, include_declaration, arg);
    }

    /// Semantic action for non-terminal 'DescriptionGroup'
    fn description_group(&mut self, arg: &DescriptionGroup) {
        before!(self, description_group, arg);
        for x in &arg.description_group_list {
            self.attribute(&x.attribute);
        }
        match &*arg.description_group_group {
            DescriptionGroupGroup::LBraceDescriptionGroupGroupListRBrace(x) => {
                self.l_brace(&x.l_brace);
                for x in &x.description_group_group_list {
                    self.description_group(&x.description_group);
                }
                self.r_brace(&x.r_brace);
            }
            DescriptionGroupGroup::DescriptionItem(x) => self.description_item(&x.description_item),
        }
        after!(self, description_group, arg);
    }

    /// Semantic action for non-terminal 'DescriptionItem'
    fn description_item(&mut self, arg: &DescriptionItem) {
        before!(self, description_item, arg);
        match arg {
            DescriptionItem::ModuleDeclaration(x) => self.module_declaration(&x.module_declaration),
            DescriptionItem::InterfaceDeclaration(x) => {
                self.interface_declaration(&x.interface_declaration)
            }
            DescriptionItem::PackageDeclaration(x) => {
                self.package_declaration(&x.package_declaration)
            }
            DescriptionItem::ProtoModuleDeclaration(x) => {
                self.proto_module_declaration(&x.proto_module_declaration)
            }
            DescriptionItem::ImportDeclaration(x) => self.import_declaration(&x.import_declaration),
            DescriptionItem::EmbedDeclaration(x) => self.embed_declaration(&x.embed_declaration),
            DescriptionItem::IncludeDeclaration(x) => {
                self.include_declaration(&x.include_declaration)
            }
        };
        after!(self, description_item, arg);
    }

    /// Semantic action for non-terminal 'Veryl'
    fn veryl(&mut self, arg: &Veryl) {
        before!(self, veryl, arg);
        self.start(&arg.start);
        for x in &arg.veryl_list {
            self.description_group(&x.description_group);
        }
        after!(self, veryl, arg);
    }

    fn get_handlers(&mut self) -> Option<Vec<&mut dyn Handler>> {
        None
    }
}

#[derive(Default)]
pub enum HandlerPoint {
    #[default]
    Before,
    After,
}

pub trait Handler: VerylGrammarTrait {
    fn set_point(&mut self, p: HandlerPoint);
}
