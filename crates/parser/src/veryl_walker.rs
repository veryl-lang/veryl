use crate::veryl_grammar_trait::*;
use crate::veryl_token::VerylToken;

pub trait VerylWalker {
    /// Semantic action for non-terminal 'VerylToken'
    fn veryl_token(&mut self, _arg: &VerylToken) {}

    /// Semantic action for non-terminal 'Start'
    fn start(&mut self, arg: &Start) {
        self.veryl_token(&arg.start_token);
    }

    /// Semantic action for non-terminal 'Exponent'
    fn exponent(&mut self, arg: &Exponent) {
        self.veryl_token(&arg.exponent_token);
    }

    /// Semantic action for non-terminal 'FixedPoint'
    fn fixed_point(&mut self, arg: &FixedPoint) {
        self.veryl_token(&arg.fixed_point_token);
    }

    /// Semantic action for non-terminal 'Based'
    fn based(&mut self, arg: &Based) {
        self.veryl_token(&arg.based_token);
    }

    /// Semantic action for non-terminal 'BaseLess'
    fn base_less(&mut self, arg: &BaseLess) {
        self.veryl_token(&arg.base_less_token);
    }

    /// Semantic action for non-terminal 'AllBit'
    fn all_bit(&mut self, arg: &AllBit) {
        self.veryl_token(&arg.all_bit_token);
    }

    /// Semantic action for non-terminal 'CommonOperator'
    fn common_operator(&mut self, arg: &CommonOperator) {
        self.veryl_token(&arg.common_operator_token);
    }

    /// Semantic action for non-terminal 'BinaryOperator'
    fn binary_operator(&mut self, arg: &BinaryOperator) {
        self.veryl_token(&arg.binary_operator_token);
    }

    /// Semantic action for non-terminal 'UnaryOperator'
    fn unary_operator(&mut self, arg: &UnaryOperator) {
        self.veryl_token(&arg.unary_operator_token);
    }

    /// Semantic action for non-terminal 'ColonColonToken'
    fn colon_colon(&mut self, arg: &ColonColon) {
        self.veryl_token(&arg.colon_colon_token);
    }

    /// Semantic action for non-terminal 'Colon'
    fn colon(&mut self, arg: &Colon) {
        self.veryl_token(&arg.colon_token);
    }

    /// Semantic action for non-terminal 'Comma'
    fn comma(&mut self, arg: &Comma) {
        self.veryl_token(&arg.comma_token);
    }

    /// Semantic action for non-terminal 'Equ'
    fn equ(&mut self, arg: &Equ) {
        self.veryl_token(&arg.equ_token);
    }

    /// Semantic action for non-terminal 'Hash'
    fn hash(&mut self, arg: &Hash) {
        self.veryl_token(&arg.hash_token);
    }

    /// Semantic action for non-terminal 'LBrace'
    fn l_brace(&mut self, arg: &LBrace) {
        self.veryl_token(&arg.l_brace_token);
    }

    /// Semantic action for non-terminal 'LBracket'
    fn l_bracket(&mut self, arg: &LBracket) {
        self.veryl_token(&arg.l_bracket_token);
    }

    /// Semantic action for non-terminal 'LParen'
    fn l_paren(&mut self, arg: &LParen) {
        self.veryl_token(&arg.l_paren_token);
    }

    /// Semantic action for non-terminal 'RBrace'
    fn r_brace(&mut self, arg: &RBrace) {
        self.veryl_token(&arg.r_brace_token);
    }

    /// Semantic action for non-terminal 'RBracket'
    fn r_bracket(&mut self, arg: &RBracket) {
        self.veryl_token(&arg.r_bracket_token);
    }

    /// Semantic action for non-terminal 'RParen'
    fn r_paren(&mut self, arg: &RParen) {
        self.veryl_token(&arg.r_paren_token);
    }

    /// Semantic action for non-terminal 'Semicolon'
    fn semicolon(&mut self, arg: &Semicolon) {
        self.veryl_token(&arg.semicolon_token);
    }

    /// Semantic action for non-terminal 'AlwaysComb'
    fn always_comb(&mut self, arg: &AlwaysComb) {
        self.veryl_token(&arg.always_comb_token);
    }

    /// Semantic action for non-terminal 'AlwaysFf'
    fn always_ff(&mut self, arg: &AlwaysFf) {
        self.veryl_token(&arg.always_ff_token);
    }

    /// Semantic action for non-terminal 'Assign'
    fn assign(&mut self, arg: &Assign) {
        self.veryl_token(&arg.assign_token);
    }

    /// Semantic action for non-terminal 'AsyncHigh'
    fn async_high(&mut self, arg: &AsyncHigh) {
        self.veryl_token(&arg.async_high_token);
    }

    /// Semantic action for non-terminal 'AsyncLow'
    fn async_low(&mut self, arg: &AsyncLow) {
        self.veryl_token(&arg.async_low_token);
    }

    /// Semantic action for non-terminal 'Bit'
    fn bit(&mut self, arg: &Bit) {
        self.veryl_token(&arg.bit_token);
    }

    /// Semantic action for non-terminal 'Else'
    fn r#else(&mut self, arg: &Else) {
        self.veryl_token(&arg.else_token);
    }

    /// Semantic action for non-terminal 'F32'
    fn f32(&mut self, arg: &F32) {
        self.veryl_token(&arg.f32_token);
    }

    /// Semantic action for non-terminal 'F64'
    fn f64(&mut self, arg: &F64) {
        self.veryl_token(&arg.f64_token);
    }

    /// Semantic action for non-terminal 'I32'
    fn i32(&mut self, arg: &I32) {
        self.veryl_token(&arg.i32_token);
    }

    /// Semantic action for non-terminal 'I64'
    fn i64(&mut self, arg: &I64) {
        self.veryl_token(&arg.i64_token);
    }

    /// Semantic action for non-terminal 'If'
    fn r#if(&mut self, arg: &If) {
        self.veryl_token(&arg.if_token);
    }

    /// Semantic action for non-terminal 'IfReset'
    fn if_reset(&mut self, arg: &IfReset) {
        self.veryl_token(&arg.if_reset_token);
    }

    /// Semantic action for non-terminal 'Inout'
    fn inout(&mut self, arg: &Inout) {
        self.veryl_token(&arg.inout_token);
    }

    /// Semantic action for non-terminal 'Input'
    fn input(&mut self, arg: &Input) {
        self.veryl_token(&arg.input_token);
    }

    /// Semantic action for non-terminal 'Interface'
    fn interface(&mut self, arg: &Interface) {
        self.veryl_token(&arg.interface_token);
    }

    /// Semantic action for non-terminal 'Localparam'
    fn localparam(&mut self, arg: &Localparam) {
        self.veryl_token(&arg.localparam_token);
    }

    /// Semantic action for non-terminal 'Logic'
    fn logic(&mut self, arg: &Logic) {
        self.veryl_token(&arg.logic_token);
    }

    /// Semantic action for non-terminal 'Modport'
    fn modport(&mut self, arg: &Modport) {
        self.veryl_token(&arg.modport_token);
    }

    /// Semantic action for non-terminal 'Module'
    fn module(&mut self, arg: &Module) {
        self.veryl_token(&arg.module_token);
    }

    /// Semantic action for non-terminal 'Negedge'
    fn negedge(&mut self, arg: &Negedge) {
        self.veryl_token(&arg.negedge_token);
    }

    /// Semantic action for non-terminal 'Output'
    fn output(&mut self, arg: &Output) {
        self.veryl_token(&arg.output_token);
    }

    /// Semantic action for non-terminal 'Parameter'
    fn parameter(&mut self, arg: &Parameter) {
        self.veryl_token(&arg.parameter_token);
    }

    /// Semantic action for non-terminal 'Posedge'
    fn posedge(&mut self, arg: &Posedge) {
        self.veryl_token(&arg.posedge_token);
    }

    /// Semantic action for non-terminal 'SyncHigh'
    fn sync_high(&mut self, arg: &SyncHigh) {
        self.veryl_token(&arg.sync_high_token);
    }

    /// Semantic action for non-terminal 'SyncLow'
    fn sync_low(&mut self, arg: &SyncLow) {
        self.veryl_token(&arg.sync_low_token);
    }

    /// Semantic action for non-terminal 'U32'
    fn u32(&mut self, arg: &U32) {
        self.veryl_token(&arg.u32_token);
    }

    /// Semantic action for non-terminal 'U64'
    fn u64(&mut self, arg: &U64) {
        self.veryl_token(&arg.u64_token);
    }

    /// Semantic action for non-terminal 'Identifier'
    fn identifier(&mut self, arg: &Identifier) {
        self.veryl_token(&arg.identifier_token);
    }

    /// Semantic action for non-terminal 'Number'
    fn number(&mut self, arg: &Number) {
        match arg {
            Number::Number0(x) => self.integral_number(&x.integral_number),
            Number::Number1(x) => self.real_number(&x.real_number),
        };
    }

    /// Semantic action for non-terminal 'IntegralNumber'
    fn integral_number(&mut self, arg: &IntegralNumber) {
        match arg {
            IntegralNumber::IntegralNumber0(x) => self.based(&x.based),
            IntegralNumber::IntegralNumber1(x) => self.base_less(&x.base_less),
            IntegralNumber::IntegralNumber2(x) => self.all_bit(&x.all_bit),
        };
    }

    /// Semantic action for non-terminal 'RealNumber'
    fn real_number(&mut self, arg: &RealNumber) {
        match arg {
            RealNumber::RealNumber0(x) => self.fixed_point(&x.fixed_point),
            RealNumber::RealNumber1(x) => self.exponent(&x.exponent),
        };
    }

    /// Semantic action for non-terminal 'Expression'
    fn expression(&mut self, arg: &Expression) {
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
    }

    /// Semantic action for non-terminal 'Expression1'
    fn expression1(&mut self, arg: &Expression1) {
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
    }

    /// Semantic action for non-terminal 'Factor'
    fn factor(&mut self, arg: &Factor) {
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
    }

    /// Semantic action for non-terminal 'Range'
    fn range(&mut self, arg: &Range) {
        self.l_bracket(&arg.l_bracket);
        self.expression(&arg.expression);
        if let Some(ref x) = arg.range_opt {
            self.colon(&x.colon);
            self.expression(&x.expression);
        }
        self.r_bracket(&arg.r_bracket);
    }

    /// Semantic action for non-terminal 'Width'
    fn width(&mut self, arg: &Width) {
        self.l_bracket(&arg.l_bracket);
        self.expression(&arg.expression);
        self.r_bracket(&arg.r_bracket);
    }

    /// Semantic action for non-terminal 'BuiltinType'
    fn builtin_type(&mut self, arg: &BuiltinType) {
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
    }

    /// Semantic action for non-terminal 'Type'
    fn r#type(&mut self, arg: &Type) {
        match &*arg.type_group {
            TypeGroup::TypeGroup0(x) => self.builtin_type(&x.builtin_type),
            TypeGroup::TypeGroup1(x) => self.identifier(&x.identifier),
        };
        for x in &arg.type_list {
            self.width(&x.width);
        }
    }

    /// Semantic action for non-terminal 'Statement'
    fn statement(&mut self, arg: &Statement) {
        match arg {
            Statement::Statement0(x) => self.assignment_statement(&x.assignment_statement),
            Statement::Statement1(x) => self.if_statement(&x.if_statement),
            Statement::Statement2(x) => self.if_reset_statement(&x.if_reset_statement),
        };
    }

    /// Semantic action for non-terminal 'AssignmentStatement'
    fn assignment_statement(&mut self, arg: &AssignmentStatement) {
        self.identifier(&arg.identifier);
        self.equ(&arg.equ);
        self.expression(&arg.expression);
        self.semicolon(&arg.semicolon);
    }

    /// Semantic action for non-terminal 'IfStatement'
    fn if_statement(&mut self, arg: &IfStatement) {
        self.r#if(&arg.r#if);
        self.expression(&arg.expression);
        self.l_brace(&arg.l_brace);
        self.statement(&arg.statement);
        self.r_brace(&arg.r_brace);
        for x in &arg.if_statement_list {
            self.r#else(&x.r#else);
            self.r#if(&x.r#if);
            self.expression(&x.expression);
            self.l_brace(&x.l_brace);
            self.statement(&x.statement);
            self.r_brace(&x.r_brace);
        }
        if let Some(ref x) = arg.if_statement_opt {
            self.r#else(&x.r#else);
            self.l_brace(&x.l_brace);
            self.statement(&x.statement);
            self.r_brace(&x.r_brace);
        }
    }

    /// Semantic action for non-terminal 'IfResetStatement'
    fn if_reset_statement(&mut self, arg: &IfResetStatement) {
        self.if_reset(&arg.if_reset);
        self.l_brace(&arg.l_brace);
        self.statement(&arg.statement);
        self.r_brace(&arg.r_brace);
        for x in &arg.if_reset_statement_list {
            self.r#else(&x.r#else);
            self.r#if(&x.r#if);
            self.expression(&x.expression);
            self.l_brace(&x.l_brace);
            self.statement(&x.statement);
            self.r_brace(&x.r_brace);
        }
        if let Some(ref x) = arg.if_reset_statement_opt {
            self.r#else(&x.r#else);
            self.l_brace(&x.l_brace);
            self.statement(&x.statement);
            self.r_brace(&x.r_brace);
        }
    }

    /// Semantic action for non-terminal 'VariableDeclaration'
    fn variable_declaration(&mut self, arg: &VariableDeclaration) {
        self.identifier(&arg.identifier);
        self.colon(&arg.colon);
        self.r#type(&arg.r#type);
        self.semicolon(&arg.semicolon);
    }

    /// Semantic action for non-terminal 'ParameterDeclaration'
    fn parameter_declaration(&mut self, arg: &ParameterDeclaration) {
        self.parameter(&arg.parameter);
        self.identifier(&arg.identifier);
        self.colon(&arg.colon);
        self.r#type(&arg.r#type);
        self.equ(&arg.equ);
        self.expression(&arg.expression);
        self.semicolon(&arg.semicolon);
    }

    /// Semantic action for non-terminal 'LocalparamDeclaration'
    fn localparam_declaration(&mut self, arg: &LocalparamDeclaration) {
        self.localparam(&arg.localparam);
        self.identifier(&arg.identifier);
        self.colon(&arg.colon);
        self.r#type(&arg.r#type);
        self.equ(&arg.equ);
        self.expression(&arg.expression);
        self.semicolon(&arg.semicolon);
    }

    /// Semantic action for non-terminal 'AlwaysFfDeclaration'
    fn always_ff_declaration(&mut self, arg: &AlwaysFfDeclaration) {
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
    }

    /// Semantic action for non-terminal 'AlwaysFfClock'
    fn always_ff_clock(&mut self, arg: &AlwaysFfClock) {
        if let Some(ref x) = arg.always_ff_clock_opt {
            match &*x.always_ff_clock_opt_group {
                AlwaysFfClockOptGroup::AlwaysFfClockOptGroup0(x) => self.posedge(&x.posedge),
                AlwaysFfClockOptGroup::AlwaysFfClockOptGroup1(x) => self.negedge(&x.negedge),
            }
        }
        self.identifier(&arg.identifier);
    }

    /// Semantic action for non-terminal 'AlwaysFfReset'
    fn always_ff_reset(&mut self, arg: &AlwaysFfReset) {
        if let Some(ref x) = arg.always_ff_reset_opt {
            match &*x.always_ff_reset_opt_group {
                AlwaysFfResetOptGroup::AlwaysFfResetOptGroup0(x) => self.async_low(&x.async_low),
                AlwaysFfResetOptGroup::AlwaysFfResetOptGroup1(x) => self.async_high(&x.async_high),
                AlwaysFfResetOptGroup::AlwaysFfResetOptGroup2(x) => self.sync_low(&x.sync_low),
                AlwaysFfResetOptGroup::AlwaysFfResetOptGroup3(x) => self.sync_high(&x.sync_high),
            }
        }
        self.identifier(&arg.identifier);
    }

    /// Semantic action for non-terminal 'AlwaysCombDeclaration'
    fn always_comb_declaration(&mut self, arg: &AlwaysCombDeclaration) {
        self.always_comb(&arg.always_comb);
        self.l_brace(&arg.l_brace);
        for x in &arg.always_comb_declaration_list {
            self.statement(&x.statement);
        }
        self.r_brace(&arg.r_brace);
    }

    /// Semantic action for non-terminal 'AssignDeclaration'
    fn assign_declaration(&mut self, arg: &AssignDeclaration) {
        self.assign(&arg.assign);
        self.identifier(&arg.identifier);
        if let Some(ref x) = arg.assign_declaration_opt {
            self.colon(&x.colon);
            self.r#type(&x.r#type);
        }
        self.equ(&arg.equ);
        self.expression(&arg.expression);
        self.semicolon(&arg.semicolon);
    }

    /// Semantic action for non-terminal 'ModportDeclaration'
    fn modport_declaration(&mut self, arg: &ModportDeclaration) {
        self.modport(&arg.modport);
        self.identifier(&arg.identifier);
        self.l_brace(&arg.l_brace);
        self.modport_list(&arg.modport_list);
        self.r_brace(&arg.r_brace);
    }

    /// Semantic action for non-terminal 'ModportList'
    fn modport_list(&mut self, arg: &ModportList) {
        self.modport_item(&arg.modport_item);
        for x in &arg.modport_list_list {
            self.comma(&x.comma);
            self.modport_item(&x.modport_item);
        }
        if let Some(ref x) = arg.modport_list_opt {
            self.comma(&x.comma);
        }
    }

    /// Semantic action for non-terminal 'ModportItem'
    fn modport_item(&mut self, arg: &ModportItem) {
        self.identifier(&arg.identifier);
        self.colon(&arg.colon);
        self.direction(&arg.direction);
    }

    /// Semantic action for non-terminal 'Instantiation'
    fn instantiation(&mut self, arg: &Instantiation) {
        self.identifier(&arg.identifier);
        self.colon_colon(&arg.colon_colon);
        self.identifier(&arg.identifier0);
        if let Some(ref x) = arg.instantiation_opt {
            self.instance_parameter(&x.instance_parameter);
        }
        self.l_brace(&arg.l_brace);
        if let Some(ref x) = arg.instantiation_opt0 {
            self.instance_port_list(&x.instance_port_list);
        }
        self.r_brace(&arg.r_brace);
    }

    /// Semantic action for non-terminal 'InstanceParameter'
    fn instance_parameter(&mut self, arg: &InstanceParameter) {
        self.hash(&arg.hash);
        self.l_paren(&arg.l_paren);
        if let Some(ref x) = arg.instance_parameter_opt {
            self.instance_parameter_list(&x.instance_parameter_list);
        }
        self.r_paren(&arg.r_paren);
    }

    /// Semantic action for non-terminal 'InstanceParameterList'
    fn instance_parameter_list(&mut self, arg: &InstanceParameterList) {
        self.instance_parameter_item(&arg.instance_parameter_item);
        for x in &arg.instance_parameter_list_list {
            self.comma(&x.comma);
            self.instance_parameter_item(&x.instance_parameter_item);
        }
        if let Some(ref x) = arg.instance_parameter_list_opt {
            self.comma(&x.comma);
        }
    }

    /// Semantic action for non-terminal 'InstanceParameterItem'
    fn instance_parameter_item(&mut self, arg: &InstanceParameterItem) {
        self.identifier(&arg.identifier);
        if let Some(ref x) = arg.instance_parameter_item_opt {
            self.colon(&x.colon);
            self.expression(&x.expression);
        }
    }

    /// Semantic action for non-terminal 'InstancePortList'
    fn instance_port_list(&mut self, arg: &InstancePortList) {
        self.instance_port_item(&arg.instance_port_item);
        for x in &arg.instance_port_list_list {
            self.comma(&x.comma);
            self.instance_port_item(&x.instance_port_item);
        }
        if let Some(ref x) = arg.instance_port_list_opt {
            self.comma(&x.comma);
        }
    }

    /// Semantic action for non-terminal 'InstancePortItem'
    fn instance_port_item(&mut self, arg: &InstancePortItem) {
        self.identifier(&arg.identifier);
        if let Some(ref x) = arg.instance_port_item_opt {
            self.colon(&x.colon);
            self.expression(&x.expression);
        }
    }

    /// Semantic action for non-terminal 'WithParameter'
    fn with_parameter(&mut self, arg: &WithParameter) {
        self.hash(&arg.hash);
        self.l_paren(&arg.l_paren);
        if let Some(ref x) = arg.with_parameter_opt {
            self.with_parameter_list(&x.with_parameter_list);
        }
        self.r_paren(&arg.r_paren);
    }

    /// Semantic action for non-terminal 'WithParameterList'
    fn with_parameter_list(&mut self, arg: &WithParameterList) {
        self.with_parameter_item(&arg.with_parameter_item);
        for x in &arg.with_parameter_list_list {
            self.comma(&x.comma);
            self.with_parameter_item(&x.with_parameter_item);
        }
        if let Some(ref x) = arg.with_parameter_list_opt {
            self.comma(&x.comma);
        }
    }

    /// Semantic action for non-terminal 'WithParameterItem'
    fn with_parameter_item(&mut self, arg: &WithParameterItem) {
        match &*arg.with_parameter_item_group {
            WithParameterItemGroup::WithParameterItemGroup0(x) => self.parameter(&x.parameter),
            WithParameterItemGroup::WithParameterItemGroup1(x) => self.localparam(&x.localparam),
        };
        self.identifier(&arg.identifier);
        self.colon(&arg.colon);
        self.r#type(&arg.r#type);
        self.equ(&arg.equ);
        self.expression(&arg.expression);
    }

    /// Semantic action for non-terminal 'ModuleDeclaration'
    fn module_declaration(&mut self, arg: &ModuleDeclaration) {
        self.module(&arg.module);
        self.identifier(&arg.identifier);
        if let Some(ref x) = arg.module_declaration_opt {
            self.with_parameter(&x.with_parameter);
        }
        if let Some(ref x) = arg.module_declaration_opt0 {
            self.module_port(&x.module_port);
        }
        self.l_brace(&arg.l_brace);
        for x in &arg.module_declaration_list {
            self.module_item(&x.module_item);
        }
        self.r_brace(&arg.r_brace);
    }

    /// Semantic action for non-terminal 'ModulePort'
    fn module_port(&mut self, arg: &ModulePort) {
        self.l_paren(&arg.l_paren);
        if let Some(ref x) = arg.module_port_opt {
            self.module_port_list(&x.module_port_list);
        }
        self.r_paren(&arg.r_paren);
    }

    /// Semantic action for non-terminal 'ModulePortList'
    fn module_port_list(&mut self, arg: &ModulePortList) {
        self.module_port_item(&arg.module_port_item);
        for x in &arg.module_port_list_list {
            self.comma(&x.comma);
            self.module_port_item(&x.module_port_item);
        }
        if let Some(ref x) = arg.module_port_list_opt {
            self.comma(&x.comma);
        }
    }

    /// Semantic action for non-terminal 'ModulePortItem'
    fn module_port_item(&mut self, arg: &ModulePortItem) {
        self.identifier(&arg.identifier);
        self.colon(&arg.colon);
        self.direction(&arg.direction);
        self.r#type(&arg.r#type);
    }

    /// Semantic action for non-terminal 'Direction'
    fn direction(&mut self, arg: &Direction) {
        match arg {
            Direction::Direction0(x) => self.input(&x.input),
            Direction::Direction1(x) => self.output(&x.output),
            Direction::Direction2(x) => self.inout(&x.inout),
        };
    }

    /// Semantic action for non-terminal 'ModuleItem'
    fn module_item(&mut self, arg: &ModuleItem) {
        match arg {
            ModuleItem::ModuleItem0(x) => self.variable_declaration(&x.variable_declaration),
            ModuleItem::ModuleItem1(x) => self.parameter_declaration(&x.parameter_declaration),
            ModuleItem::ModuleItem2(x) => self.localparam_declaration(&x.localparam_declaration),
            ModuleItem::ModuleItem3(x) => self.always_ff_declaration(&x.always_ff_declaration),
            ModuleItem::ModuleItem4(x) => self.always_comb_declaration(&x.always_comb_declaration),
            ModuleItem::ModuleItem5(x) => self.assign_declaration(&x.assign_declaration),
            ModuleItem::ModuleItem6(x) => self.instantiation(&x.instantiation),
        };
    }

    /// Semantic action for non-terminal 'InterfaceDeclaration'
    fn interface_declaration(&mut self, arg: &InterfaceDeclaration) {
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
    }

    /// Semantic action for non-terminal 'InterfaceItem'
    fn interface_item(&mut self, arg: &InterfaceItem) {
        match arg {
            InterfaceItem::InterfaceItem0(x) => self.variable_declaration(&x.variable_declaration),
            InterfaceItem::InterfaceItem1(x) => {
                self.parameter_declaration(&x.parameter_declaration)
            }
            InterfaceItem::InterfaceItem2(x) => {
                self.localparam_declaration(&x.localparam_declaration)
            }
            InterfaceItem::InterfaceItem3(x) => self.modport_declaration(&x.modport_declaration),
        };
    }

    /// Semantic action for non-terminal 'Description'
    fn description(&mut self, arg: &Description) {
        match arg {
            Description::Description0(x) => self.module_declaration(&x.module_declaration),
            Description::Description1(x) => self.interface_declaration(&x.interface_declaration),
        };
    }

    /// Semantic action for non-terminal 'Veryl'
    fn veryl(&mut self, arg: &Veryl) {
        self.start(&arg.start);
        for x in &arg.veryl_list {
            self.description(&x.description);
        }
    }
}
