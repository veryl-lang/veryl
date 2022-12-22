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

    /// Semantic action for non-terminal 'CommonOperator'
    fn common_operator(&mut self, arg: &CommonOperator) {
        before!(self, common_operator, arg);
        self.veryl_token(&arg.common_operator_token);
        after!(self, common_operator, arg);
    }

    /// Semantic action for non-terminal 'BinaryOperator'
    fn binary_operator(&mut self, arg: &BinaryOperator) {
        before!(self, binary_operator, arg);
        self.veryl_token(&arg.binary_operator_token);
        after!(self, binary_operator, arg);
    }

    /// Semantic action for non-terminal 'UnaryOperator'
    fn unary_operator(&mut self, arg: &UnaryOperator) {
        before!(self, unary_operator, arg);
        self.veryl_token(&arg.unary_operator_token);
        after!(self, unary_operator, arg);
    }

    /// Semantic action for non-terminal 'Colon'
    fn colon(&mut self, arg: &Colon) {
        before!(self, colon, arg);
        self.veryl_token(&arg.colon_token);
        after!(self, colon, arg);
    }

    /// Semantic action for non-terminal 'ColonColonColon'
    fn colon_colon_colon(&mut self, arg: &ColonColonColon) {
        before!(self, colon_colon_colon, arg);
        self.veryl_token(&arg.colon_colon_colon_token);
        after!(self, colon_colon_colon, arg);
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

    /// Semantic action for non-terminal 'MinusGT'
    fn minus_g_t(&mut self, arg: &MinusGT) {
        before!(self, minus_g_t, arg);
        self.veryl_token(&arg.minus_g_t_token);
        after!(self, minus_g_t, arg);
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

    /// Semantic action for non-terminal 'Assign'
    fn assign(&mut self, arg: &Assign) {
        before!(self, assign, arg);
        self.veryl_token(&arg.assign_token);
        after!(self, assign, arg);
    }

    /// Semantic action for non-terminal 'AsyncHigh'
    fn async_high(&mut self, arg: &AsyncHigh) {
        before!(self, async_high, arg);
        self.veryl_token(&arg.async_high_token);
        after!(self, async_high, arg);
    }

    /// Semantic action for non-terminal 'AsyncLow'
    fn async_low(&mut self, arg: &AsyncLow) {
        before!(self, async_low, arg);
        self.veryl_token(&arg.async_low_token);
        after!(self, async_low, arg);
    }

    /// Semantic action for non-terminal 'Bit'
    fn bit(&mut self, arg: &Bit) {
        before!(self, bit, arg);
        self.veryl_token(&arg.bit_token);
        after!(self, bit, arg);
    }

    /// Semantic action for non-terminal 'Else'
    fn r#else(&mut self, arg: &Else) {
        before!(self, r#else, arg);
        self.veryl_token(&arg.else_token);
        after!(self, r#else, arg);
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

    /// Semantic action for non-terminal 'In'
    fn r#in(&mut self, arg: &In) {
        before!(self, r#in, arg);
        self.veryl_token(&arg.in_token);
        after!(self, r#in, arg);
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

    /// Semantic action for non-terminal 'Interface'
    fn interface(&mut self, arg: &Interface) {
        before!(self, interface, arg);
        self.veryl_token(&arg.interface_token);
        after!(self, interface, arg);
    }

    /// Semantic action for non-terminal 'Localparam'
    fn localparam(&mut self, arg: &Localparam) {
        before!(self, localparam, arg);
        self.veryl_token(&arg.localparam_token);
        after!(self, localparam, arg);
    }

    /// Semantic action for non-terminal 'Logic'
    fn logic(&mut self, arg: &Logic) {
        before!(self, logic, arg);
        self.veryl_token(&arg.logic_token);
        after!(self, logic, arg);
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

    /// Semantic action for non-terminal 'Negedge'
    fn negedge(&mut self, arg: &Negedge) {
        before!(self, negedge, arg);
        self.veryl_token(&arg.negedge_token);
        after!(self, negedge, arg);
    }

    /// Semantic action for non-terminal 'Output'
    fn output(&mut self, arg: &Output) {
        before!(self, output, arg);
        self.veryl_token(&arg.output_token);
        after!(self, output, arg);
    }

    /// Semantic action for non-terminal 'Parameter'
    fn parameter(&mut self, arg: &Parameter) {
        before!(self, parameter, arg);
        self.veryl_token(&arg.parameter_token);
        after!(self, parameter, arg);
    }

    /// Semantic action for non-terminal 'Posedge'
    fn posedge(&mut self, arg: &Posedge) {
        before!(self, posedge, arg);
        self.veryl_token(&arg.posedge_token);
        after!(self, posedge, arg);
    }

    /// Semantic action for non-terminal 'Ref'
    fn r#ref(&mut self, arg: &Ref) {
        before!(self, r#ref, arg);
        self.veryl_token(&arg.ref_token);
        after!(self, r#ref, arg);
    }

    /// Semantic action for non-terminal 'Return'
    fn r#return(&mut self, arg: &Return) {
        before!(self, r#return, arg);
        self.veryl_token(&arg.return_token);
        after!(self, r#return, arg);
    }

    /// Semantic action for non-terminal 'Step'
    fn step(&mut self, arg: &Step) {
        before!(self, step, arg);
        self.veryl_token(&arg.step_token);
        after!(self, step, arg);
    }

    /// Semantic action for non-terminal 'SyncHigh'
    fn sync_high(&mut self, arg: &SyncHigh) {
        before!(self, sync_high, arg);
        self.veryl_token(&arg.sync_high_token);
        after!(self, sync_high, arg);
    }

    /// Semantic action for non-terminal 'SyncLow'
    fn sync_low(&mut self, arg: &SyncLow) {
        before!(self, sync_low, arg);
        self.veryl_token(&arg.sync_low_token);
        after!(self, sync_low, arg);
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
            Number::Number0(x) => self.integral_number(&x.integral_number),
            Number::Number1(x) => self.real_number(&x.real_number),
        };
        after!(self, number, arg);
    }

    /// Semantic action for non-terminal 'IntegralNumber'
    fn integral_number(&mut self, arg: &IntegralNumber) {
        before!(self, integral_number, arg);
        match arg {
            IntegralNumber::IntegralNumber0(x) => self.based(&x.based),
            IntegralNumber::IntegralNumber1(x) => self.base_less(&x.base_less),
            IntegralNumber::IntegralNumber2(x) => self.all_bit(&x.all_bit),
        };
        after!(self, integral_number, arg);
    }

    /// Semantic action for non-terminal 'RealNumber'
    fn real_number(&mut self, arg: &RealNumber) {
        before!(self, real_number, arg);
        match arg {
            RealNumber::RealNumber0(x) => self.fixed_point(&x.fixed_point),
            RealNumber::RealNumber1(x) => self.exponent(&x.exponent),
        };
        after!(self, real_number, arg);
    }

    /// Semantic action for non-terminal 'Expression'
    fn expression(&mut self, arg: &Expression) {
        before!(self, expression, arg);
        self.expression1(&arg.expression1);
        for x in &arg.expression_list {
            match &*x.expression_list_group {
                ExpressionListGroup::ExpressionListGroup0(x) => {
                    self.binary_operator(&x.binary_operator)
                }
                ExpressionListGroup::ExpressionListGroup1(x) => {
                    self.common_operator(&x.common_operator)
                }
            };
            self.expression1(&x.expression1);
        }
        after!(self, expression, arg);
    }

    /// Semantic action for non-terminal 'Expression1'
    fn expression1(&mut self, arg: &Expression1) {
        before!(self, expression1, arg);
        if let Some(ref x) = arg.expression1_opt {
            match &*x.expression1_opt_group {
                Expression1OptGroup::Expression1OptGroup0(x) => {
                    self.unary_operator(&x.unary_operator)
                }
                Expression1OptGroup::Expression1OptGroup1(x) => {
                    self.common_operator(&x.common_operator)
                }
            };
        }
        self.factor(&arg.factor);
        after!(self, expression1, arg);
    }

    /// Semantic action for non-terminal 'Factor'
    fn factor(&mut self, arg: &Factor) {
        before!(self, factor, arg);
        match arg {
            Factor::Factor0(x) => self.number(&x.number),
            Factor::Factor1(x) => {
                self.identifier(&x.identifier);
                for x in &x.factor_list {
                    self.range(&x.range);
                }
            }
            Factor::Factor2(x) => {
                self.l_paren(&x.l_paren);
                self.expression(&x.expression);
                self.r_paren(&x.r_paren);
            }
        }
        after!(self, factor, arg);
    }

    /// Semantic action for non-terminal 'Range'
    fn range(&mut self, arg: &Range) {
        before!(self, range, arg);
        self.l_bracket(&arg.l_bracket);
        self.expression(&arg.expression);
        if let Some(ref x) = arg.range_opt {
            self.colon(&x.colon);
            self.expression(&x.expression);
        }
        self.r_bracket(&arg.r_bracket);
        after!(self, range, arg);
    }

    /// Semantic action for non-terminal 'Width'
    fn width(&mut self, arg: &Width) {
        before!(self, width, arg);
        self.l_bracket(&arg.l_bracket);
        self.expression(&arg.expression);
        self.r_bracket(&arg.r_bracket);
        after!(self, width, arg);
    }

    /// Semantic action for non-terminal 'BuiltinType'
    fn builtin_type(&mut self, arg: &BuiltinType) {
        before!(self, builtin_type, arg);
        match arg {
            BuiltinType::BuiltinType0(x) => self.logic(&x.logic),
            BuiltinType::BuiltinType1(x) => self.bit(&x.bit),
            BuiltinType::BuiltinType2(x) => self.u32(&x.u32),
            BuiltinType::BuiltinType3(x) => self.u64(&x.u64),
            BuiltinType::BuiltinType4(x) => self.i32(&x.i32),
            BuiltinType::BuiltinType5(x) => self.i64(&x.i64),
            BuiltinType::BuiltinType6(x) => self.f32(&x.f32),
            BuiltinType::BuiltinType7(x) => self.f64(&x.f64),
        };
        after!(self, builtin_type, arg);
    }

    /// Semantic action for non-terminal 'Type'
    fn r#type(&mut self, arg: &Type) {
        before!(self, r#type, arg);
        match &*arg.type_group {
            TypeGroup::TypeGroup0(x) => self.builtin_type(&x.builtin_type),
            TypeGroup::TypeGroup1(x) => self.identifier(&x.identifier),
        };
        for x in &arg.type_list {
            self.width(&x.width);
        }
        after!(self, r#type, arg);
    }

    /// Semantic action for non-terminal 'Statement'
    fn statement(&mut self, arg: &Statement) {
        before!(self, statement, arg);
        match arg {
            Statement::Statement0(x) => self.assignment_statement(&x.assignment_statement),
            Statement::Statement1(x) => self.if_statement(&x.if_statement),
            Statement::Statement2(x) => self.if_reset_statement(&x.if_reset_statement),
            Statement::Statement3(x) => self.return_statement(&x.return_statement),
            Statement::Statement4(x) => self.for_statement(&x.for_statement),
        };
        after!(self, statement, arg);
    }

    /// Semantic action for non-terminal 'AssignmentStatement'
    fn assignment_statement(&mut self, arg: &AssignmentStatement) {
        before!(self, assignment_statement, arg);
        self.identifier(&arg.identifier);
        match &*arg.assignment_statement_group {
            AssignmentStatementGroup::AssignmentStatementGroup0(x) => self.equ(&x.equ),
            AssignmentStatementGroup::AssignmentStatementGroup1(x) => {
                self.assignment_operator(&x.assignment_operator)
            }
        }
        self.expression(&arg.expression);
        self.semicolon(&arg.semicolon);
        after!(self, assignment_statement, arg);
    }

    /// Semantic action for non-terminal 'IfStatement'
    fn if_statement(&mut self, arg: &IfStatement) {
        before!(self, if_statement, arg);
        self.r#if(&arg.r#if);
        self.expression(&arg.expression);
        self.l_brace(&arg.l_brace);
        for x in &arg.if_statement_list {
            self.statement(&x.statement);
        }
        self.r_brace(&arg.r_brace);
        for x in &arg.if_statement_list0 {
            self.r#else(&x.r#else);
            self.r#if(&x.r#if);
            self.expression(&x.expression);
            self.l_brace(&x.l_brace);
            for x in &x.if_statement_list0_list {
                self.statement(&x.statement);
            }
            self.r_brace(&x.r_brace);
        }
        if let Some(ref x) = arg.if_statement_opt {
            self.r#else(&x.r#else);
            self.l_brace(&x.l_brace);
            for x in &x.if_statement_opt_list {
                self.statement(&x.statement);
            }
            self.r_brace(&x.r_brace);
        }
        after!(self, if_statement, arg);
    }

    /// Semantic action for non-terminal 'IfResetStatement'
    fn if_reset_statement(&mut self, arg: &IfResetStatement) {
        before!(self, if_reset_statement, arg);
        self.if_reset(&arg.if_reset);
        self.l_brace(&arg.l_brace);
        for x in &arg.if_reset_statement_list {
            self.statement(&x.statement);
        }
        self.r_brace(&arg.r_brace);
        for x in &arg.if_reset_statement_list0 {
            self.r#else(&x.r#else);
            self.r#if(&x.r#if);
            self.expression(&x.expression);
            self.l_brace(&x.l_brace);
            for x in &x.if_reset_statement_list0_list {
                self.statement(&x.statement);
            }
            self.r_brace(&x.r_brace);
        }
        if let Some(ref x) = arg.if_reset_statement_opt {
            self.r#else(&x.r#else);
            self.l_brace(&x.l_brace);
            for x in &x.if_reset_statement_opt_list {
                self.statement(&x.statement);
            }
            self.r_brace(&x.r_brace);
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

    /// Semantic action for non-terminal 'ForStatement'
    fn for_statement(&mut self, arg: &ForStatement) {
        before!(self, for_statement, arg);
        self.r#for(&arg.r#for);
        self.identifier(&arg.identifier);
        self.colon(&arg.colon);
        self.r#type(&arg.r#type);
        self.r#in(&arg.r#in);
        self.expression(&arg.expression);
        self.dot_dot(&arg.dot_dot);
        self.expression(&arg.expression0);
        if let Some(ref x) = arg.for_statement_opt {
            self.step(&x.step);
            self.assignment_operator(&x.assignment_operator);
            self.expression(&x.expression);
        }
        self.l_brace(&arg.l_brace);
        for x in &arg.for_statement_list {
            self.statement(&x.statement);
        }
        self.r_brace(&arg.r_brace);
        after!(self, for_statement, arg);
    }

    /// Semantic action for non-terminal 'VariableDeclaration'
    fn variable_declaration(&mut self, arg: &VariableDeclaration) {
        before!(self, variable_declaration, arg);
        self.identifier(&arg.identifier);
        self.colon(&arg.colon);
        self.r#type(&arg.r#type);
        self.semicolon(&arg.semicolon);
        after!(self, variable_declaration, arg);
    }

    /// Semantic action for non-terminal 'ParameterDeclaration'
    fn parameter_declaration(&mut self, arg: &ParameterDeclaration) {
        before!(self, parameter_declaration, arg);
        self.parameter(&arg.parameter);
        self.identifier(&arg.identifier);
        self.colon(&arg.colon);
        self.r#type(&arg.r#type);
        self.equ(&arg.equ);
        self.expression(&arg.expression);
        self.semicolon(&arg.semicolon);
        after!(self, parameter_declaration, arg);
    }

    /// Semantic action for non-terminal 'LocalparamDeclaration'
    fn localparam_declaration(&mut self, arg: &LocalparamDeclaration) {
        before!(self, localparam_declaration, arg);
        self.localparam(&arg.localparam);
        self.identifier(&arg.identifier);
        self.colon(&arg.colon);
        self.r#type(&arg.r#type);
        self.equ(&arg.equ);
        self.expression(&arg.expression);
        self.semicolon(&arg.semicolon);
        after!(self, localparam_declaration, arg);
    }

    /// Semantic action for non-terminal 'AlwaysFfDeclaration'
    fn always_ff_declaration(&mut self, arg: &AlwaysFfDeclaration) {
        before!(self, always_ff_declaration, arg);
        self.always_ff(&arg.always_ff);
        self.l_paren(&arg.l_paren);
        self.always_ff_clock(&arg.always_ff_clock);
        if let Some(ref x) = arg.always_ff_declaration_opt {
            self.comma(&x.comma);
            self.always_ff_reset(&x.always_ff_reset);
        }
        self.r_paren(&arg.r_paren);
        self.l_brace(&arg.l_brace);
        for x in &arg.always_ff_declaration_list {
            self.statement(&x.statement);
        }
        self.r_brace(&arg.r_brace);
        after!(self, always_ff_declaration, arg);
    }

    /// Semantic action for non-terminal 'AlwaysFfClock'
    fn always_ff_clock(&mut self, arg: &AlwaysFfClock) {
        before!(self, always_ff_clock, arg);
        if let Some(ref x) = arg.always_ff_clock_opt {
            match &*x.always_ff_clock_opt_group {
                AlwaysFfClockOptGroup::AlwaysFfClockOptGroup0(x) => self.posedge(&x.posedge),
                AlwaysFfClockOptGroup::AlwaysFfClockOptGroup1(x) => self.negedge(&x.negedge),
            }
        }
        self.identifier(&arg.identifier);
        after!(self, always_ff_clock, arg);
    }

    /// Semantic action for non-terminal 'AlwaysFfReset'
    fn always_ff_reset(&mut self, arg: &AlwaysFfReset) {
        before!(self, always_ff_reset, arg);
        if let Some(ref x) = arg.always_ff_reset_opt {
            match &*x.always_ff_reset_opt_group {
                AlwaysFfResetOptGroup::AlwaysFfResetOptGroup0(x) => self.async_low(&x.async_low),
                AlwaysFfResetOptGroup::AlwaysFfResetOptGroup1(x) => self.async_high(&x.async_high),
                AlwaysFfResetOptGroup::AlwaysFfResetOptGroup2(x) => self.sync_low(&x.sync_low),
                AlwaysFfResetOptGroup::AlwaysFfResetOptGroup3(x) => self.sync_high(&x.sync_high),
            }
        }
        self.identifier(&arg.identifier);
        after!(self, always_ff_reset, arg);
    }

    /// Semantic action for non-terminal 'AlwaysCombDeclaration'
    fn always_comb_declaration(&mut self, arg: &AlwaysCombDeclaration) {
        before!(self, always_comb_declaration, arg);
        self.always_comb(&arg.always_comb);
        self.l_brace(&arg.l_brace);
        for x in &arg.always_comb_declaration_list {
            self.statement(&x.statement);
        }
        self.r_brace(&arg.r_brace);
        after!(self, always_comb_declaration, arg);
    }

    /// Semantic action for non-terminal 'AssignDeclaration'
    fn assign_declaration(&mut self, arg: &AssignDeclaration) {
        before!(self, assign_declaration, arg);
        self.assign(&arg.assign);
        self.identifier(&arg.identifier);
        if let Some(ref x) = arg.assign_declaration_opt {
            self.colon(&x.colon);
            self.r#type(&x.r#type);
        }
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
        self.modport_item(&arg.modport_item);
        for x in &arg.modport_list_list {
            self.comma(&x.comma);
            self.modport_item(&x.modport_item);
        }
        if let Some(ref x) = arg.modport_list_opt {
            self.comma(&x.comma);
        }
        after!(self, modport_list, arg);
    }

    /// Semantic action for non-terminal 'ModportItem'
    fn modport_item(&mut self, arg: &ModportItem) {
        before!(self, modport_item, arg);
        self.identifier(&arg.identifier);
        self.colon(&arg.colon);
        self.direction(&arg.direction);
        after!(self, modport_item, arg);
    }

    /// Semantic action for non-terminal 'Instantiation'
    fn instantiation(&mut self, arg: &Instantiation) {
        before!(self, instantiation, arg);
        self.identifier(&arg.identifier);
        self.colon_colon_colon(&arg.colon_colon_colon);
        self.identifier(&arg.identifier0);
        if let Some(ref x) = arg.instantiation_opt {
            self.instance_parameter(&x.instance_parameter);
        }
        self.l_brace(&arg.l_brace);
        if let Some(ref x) = arg.instantiation_opt0 {
            self.instance_port_list(&x.instance_port_list);
        }
        self.r_brace(&arg.r_brace);
        after!(self, instantiation, arg);
    }

    /// Semantic action for non-terminal 'InstanceParameter'
    fn instance_parameter(&mut self, arg: &InstanceParameter) {
        before!(self, instance_parameter, arg);
        self.hash(&arg.hash);
        self.l_paren(&arg.l_paren);
        if let Some(ref x) = arg.instance_parameter_opt {
            self.instance_parameter_list(&x.instance_parameter_list);
        }
        self.r_paren(&arg.r_paren);
        after!(self, instance_parameter, arg);
    }

    /// Semantic action for non-terminal 'InstanceParameterList'
    fn instance_parameter_list(&mut self, arg: &InstanceParameterList) {
        before!(self, instance_parameter_list, arg);
        self.instance_parameter_item(&arg.instance_parameter_item);
        for x in &arg.instance_parameter_list_list {
            self.comma(&x.comma);
            self.instance_parameter_item(&x.instance_parameter_item);
        }
        if let Some(ref x) = arg.instance_parameter_list_opt {
            self.comma(&x.comma);
        }
        after!(self, instance_parameter_list, arg);
    }

    /// Semantic action for non-terminal 'InstanceParameterItem'
    fn instance_parameter_item(&mut self, arg: &InstanceParameterItem) {
        before!(self, instance_parameter_item, arg);
        self.identifier(&arg.identifier);
        if let Some(ref x) = arg.instance_parameter_item_opt {
            self.colon(&x.colon);
            self.expression(&x.expression);
        }
        after!(self, instance_parameter_item, arg);
    }

    /// Semantic action for non-terminal 'InstancePortList'
    fn instance_port_list(&mut self, arg: &InstancePortList) {
        before!(self, instance_port_list, arg);
        self.instance_port_item(&arg.instance_port_item);
        for x in &arg.instance_port_list_list {
            self.comma(&x.comma);
            self.instance_port_item(&x.instance_port_item);
        }
        if let Some(ref x) = arg.instance_port_list_opt {
            self.comma(&x.comma);
        }
        after!(self, instance_port_list, arg);
    }

    /// Semantic action for non-terminal 'InstancePortItem'
    fn instance_port_item(&mut self, arg: &InstancePortItem) {
        before!(self, instance_port_item, arg);
        self.identifier(&arg.identifier);
        if let Some(ref x) = arg.instance_port_item_opt {
            self.colon(&x.colon);
            self.expression(&x.expression);
        }
        after!(self, instance_port_item, arg);
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
        self.with_parameter_item(&arg.with_parameter_item);
        for x in &arg.with_parameter_list_list {
            self.comma(&x.comma);
            self.with_parameter_item(&x.with_parameter_item);
        }
        if let Some(ref x) = arg.with_parameter_list_opt {
            self.comma(&x.comma);
        }
        after!(self, with_parameter_list, arg);
    }

    /// Semantic action for non-terminal 'WithParameterItem'
    fn with_parameter_item(&mut self, arg: &WithParameterItem) {
        before!(self, with_parameter_item, arg);
        match &*arg.with_parameter_item_group {
            WithParameterItemGroup::WithParameterItemGroup0(x) => self.parameter(&x.parameter),
            WithParameterItemGroup::WithParameterItemGroup1(x) => self.localparam(&x.localparam),
        };
        self.identifier(&arg.identifier);
        self.colon(&arg.colon);
        self.r#type(&arg.r#type);
        self.equ(&arg.equ);
        self.expression(&arg.expression);
        after!(self, with_parameter_item, arg);
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
        self.port_declaration_item(&arg.port_declaration_item);
        for x in &arg.port_declaration_list_list {
            self.comma(&x.comma);
            self.port_declaration_item(&x.port_declaration_item);
        }
        if let Some(ref x) = arg.port_declaration_list_opt {
            self.comma(&x.comma);
        }
        after!(self, port_declaration_list, arg);
    }

    /// Semantic action for non-terminal 'PortDeclarationItem'
    fn port_declaration_item(&mut self, arg: &PortDeclarationItem) {
        before!(self, port_declaration_item, arg);
        self.identifier(&arg.identifier);
        self.colon(&arg.colon);
        self.direction(&arg.direction);
        self.r#type(&arg.r#type);
        after!(self, port_declaration_item, arg);
    }

    /// Semantic action for non-terminal 'Direction'
    fn direction(&mut self, arg: &Direction) {
        before!(self, direction, arg);
        match arg {
            Direction::Direction0(x) => self.input(&x.input),
            Direction::Direction1(x) => self.output(&x.output),
            Direction::Direction2(x) => self.inout(&x.inout),
            Direction::Direction3(x) => self.r#ref(&x.r#ref),
        };
        after!(self, direction, arg);
    }

    /// Semantic action for non-terminal 'FunctionDeclaration'
    fn function_declaration(&mut self, arg: &FunctionDeclaration) {
        before!(self, function_declaration, arg);
        self.function(&arg.function);
        self.identifier(&arg.identifier);
        if let Some(ref x) = arg.function_declaration_opt {
            self.with_parameter(&x.with_parameter);
        }
        if let Some(ref x) = arg.function_declaration_opt0 {
            self.port_declaration(&x.port_declaration);
        }
        self.minus_g_t(&arg.minus_g_t);
        self.r#type(&arg.r#type);
        self.l_brace(&arg.l_brace);
        for x in &arg.function_declaration_list {
            self.function_item(&x.function_item);
        }
        self.r_brace(&arg.r_brace);
        after!(self, function_declaration, arg);
    }

    /// Semantic action for non-terminal 'FunctionItem'
    fn function_item(&mut self, arg: &FunctionItem) {
        before!(self, function_item, arg);
        match arg {
            FunctionItem::FunctionItem0(x) => self.variable_declaration(&x.variable_declaration),
            FunctionItem::FunctionItem1(x) => self.statement(&x.statement),
        };
        after!(self, function_item, arg);
    }

    /// Semantic action for non-terminal 'ModuleDeclaration'
    fn module_declaration(&mut self, arg: &ModuleDeclaration) {
        before!(self, module_declaration, arg);
        self.module(&arg.module);
        self.identifier(&arg.identifier);
        if let Some(ref x) = arg.module_declaration_opt {
            self.with_parameter(&x.with_parameter);
        }
        if let Some(ref x) = arg.module_declaration_opt0 {
            self.port_declaration(&x.port_declaration);
        }
        self.l_brace(&arg.l_brace);
        for x in &arg.module_declaration_list {
            self.module_item(&x.module_item);
        }
        self.r_brace(&arg.r_brace);
        after!(self, module_declaration, arg);
    }

    /// Semantic action for non-terminal 'ModuleIfDeclaration'
    fn module_if_declaration(&mut self, arg: &ModuleIfDeclaration) {
        before!(self, module_if_declaration, arg);
        self.r#if(&arg.r#if);
        self.expression(&arg.expression);
        self.l_brace(&arg.l_brace);
        for x in &arg.module_if_declaration_list {
            self.module_item(&x.module_item);
        }
        self.r_brace(&arg.r_brace);
        for x in &arg.module_if_declaration_list0 {
            self.r#else(&x.r#else);
            self.r#if(&x.r#if);
            self.expression(&x.expression);
            self.l_brace(&x.l_brace);
            for x in &x.module_if_declaration_list0_list {
                self.module_item(&x.module_item);
            }
            self.r_brace(&x.r_brace);
        }
        if let Some(ref x) = arg.module_if_declaration_opt {
            self.r#else(&x.r#else);
            self.l_brace(&x.l_brace);
            for x in &x.module_if_declaration_opt_list {
                self.module_item(&x.module_item);
            }
            self.r_brace(&x.r_brace);
        }
        after!(self, module_if_declaration, arg);
    }

    /// Semantic action for non-terminal 'ModuleForDeclaration'
    fn module_for_declaration(&mut self, arg: &ModuleForDeclaration) {
        before!(self, module_for_declaration, arg);
        self.r#for(&arg.r#for);
        self.identifier(&arg.identifier);
        self.colon(&arg.colon);
        self.r#type(&arg.r#type);
        self.r#in(&arg.r#in);
        self.expression(&arg.expression);
        self.dot_dot(&arg.dot_dot);
        self.expression(&arg.expression0);
        if let Some(ref x) = arg.module_for_declaration_opt {
            self.step(&x.step);
            self.assignment_operator(&x.assignment_operator);
            self.expression(&x.expression);
        }
        self.l_brace(&arg.l_brace);
        for x in &arg.module_for_declaration_list {
            self.module_item(&x.module_item);
        }
        self.r_brace(&arg.r_brace);
        after!(self, module_for_declaration, arg);
    }

    /// Semantic action for non-terminal 'ModuleItem'
    fn module_item(&mut self, arg: &ModuleItem) {
        before!(self, module_item, arg);
        match arg {
            ModuleItem::ModuleItem0(x) => self.variable_declaration(&x.variable_declaration),
            ModuleItem::ModuleItem1(x) => self.parameter_declaration(&x.parameter_declaration),
            ModuleItem::ModuleItem2(x) => self.localparam_declaration(&x.localparam_declaration),
            ModuleItem::ModuleItem3(x) => self.always_ff_declaration(&x.always_ff_declaration),
            ModuleItem::ModuleItem4(x) => self.always_comb_declaration(&x.always_comb_declaration),
            ModuleItem::ModuleItem5(x) => self.assign_declaration(&x.assign_declaration),
            ModuleItem::ModuleItem6(x) => self.instantiation(&x.instantiation),
            ModuleItem::ModuleItem7(x) => self.function_declaration(&x.function_declaration),
            ModuleItem::ModuleItem8(x) => self.module_if_declaration(&x.module_if_declaration),
            ModuleItem::ModuleItem9(x) => self.module_for_declaration(&x.module_for_declaration),
        };
        after!(self, module_item, arg);
    }

    /// Semantic action for non-terminal 'InterfaceDeclaration'
    fn interface_declaration(&mut self, arg: &InterfaceDeclaration) {
        before!(self, interface_declaration, arg);
        self.interface(&arg.interface);
        self.identifier(&arg.identifier);
        if let Some(ref x) = arg.interface_declaration_opt {
            self.with_parameter(&x.with_parameter);
        }
        self.l_brace(&arg.l_brace);
        for x in &arg.interface_declaration_list {
            self.interface_item(&x.interface_item);
        }
        self.r_brace(&arg.r_brace);
        after!(self, interface_declaration, arg);
    }

    /// Semantic action for non-terminal 'InterfaceIfDeclaration'
    fn interface_if_declaration(&mut self, arg: &InterfaceIfDeclaration) {
        before!(self, interface_if_declaration, arg);
        self.r#if(&arg.r#if);
        self.expression(&arg.expression);
        self.l_brace(&arg.l_brace);
        for x in &arg.interface_if_declaration_list {
            self.interface_item(&x.interface_item);
        }
        self.r_brace(&arg.r_brace);
        for x in &arg.interface_if_declaration_list0 {
            self.r#else(&x.r#else);
            self.r#if(&x.r#if);
            self.expression(&x.expression);
            self.l_brace(&x.l_brace);
            for x in &x.interface_if_declaration_list0_list {
                self.interface_item(&x.interface_item);
            }
            self.r_brace(&x.r_brace);
        }
        if let Some(ref x) = arg.interface_if_declaration_opt {
            self.r#else(&x.r#else);
            self.l_brace(&x.l_brace);
            for x in &x.interface_if_declaration_opt_list {
                self.interface_item(&x.interface_item);
            }
            self.r_brace(&x.r_brace);
        }
        after!(self, interface_if_declaration, arg);
    }

    /// Semantic action for non-terminal 'InterfaceForDeclaration'
    fn interface_for_declaration(&mut self, arg: &InterfaceForDeclaration) {
        before!(self, interface_for_declaration, arg);
        self.r#for(&arg.r#for);
        self.identifier(&arg.identifier);
        self.colon(&arg.colon);
        self.r#type(&arg.r#type);
        self.r#in(&arg.r#in);
        self.expression(&arg.expression);
        self.dot_dot(&arg.dot_dot);
        self.expression(&arg.expression0);
        if let Some(ref x) = arg.interface_for_declaration_opt {
            self.step(&x.step);
            self.assignment_operator(&x.assignment_operator);
            self.expression(&x.expression);
        }
        self.l_brace(&arg.l_brace);
        for x in &arg.interface_for_declaration_list {
            self.interface_item(&x.interface_item);
        }
        self.r_brace(&arg.r_brace);
        after!(self, interface_for_declaration, arg);
    }

    /// Semantic action for non-terminal 'InterfaceItem'
    fn interface_item(&mut self, arg: &InterfaceItem) {
        before!(self, interface_item, arg);
        match arg {
            InterfaceItem::InterfaceItem0(x) => self.variable_declaration(&x.variable_declaration),
            InterfaceItem::InterfaceItem1(x) => {
                self.parameter_declaration(&x.parameter_declaration)
            }
            InterfaceItem::InterfaceItem2(x) => {
                self.localparam_declaration(&x.localparam_declaration)
            }
            InterfaceItem::InterfaceItem3(x) => self.modport_declaration(&x.modport_declaration),
            InterfaceItem::InterfaceItem4(x) => {
                self.interface_if_declaration(&x.interface_if_declaration)
            }
            InterfaceItem::InterfaceItem5(x) => {
                self.interface_for_declaration(&x.interface_for_declaration)
            }
        };
        after!(self, interface_item, arg);
    }

    /// Semantic action for non-terminal 'Description'
    fn description(&mut self, arg: &Description) {
        before!(self, description, arg);
        match arg {
            Description::Description0(x) => self.module_declaration(&x.module_declaration),
            Description::Description1(x) => self.interface_declaration(&x.interface_declaration),
        };
        after!(self, description, arg);
    }

    /// Semantic action for non-terminal 'Veryl'
    fn veryl(&mut self, arg: &Veryl) {
        before!(self, veryl, arg);
        self.start(&arg.start);
        for x in &arg.veryl_list {
            self.description(&x.description);
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
