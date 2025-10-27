pub use crate::generated::veryl_grammar_trait::*;
use crate::resource_table::StrId;
use crate::veryl_token::{VerylToken, is_anonymous_token};
use paste::paste;
use std::fmt;

macro_rules! list_group_to_item {
    ($x:ident) => {
        paste! {
            impl<'a> From<&'a [<$x List>]> for Vec<&'a [<$x Item>]> {
                fn from(x: &'a [<$x List>]) -> Self {
                    let mut ret = Vec::new();
                    {
                        let mut x: Vec<&'a [<$x Item>]> = x.[<$x:snake _group>].as_ref().into();
                        ret.append(&mut x);
                    }
                    for x in &x.[<$x:snake _list_list>] {
                        let mut x: Vec<&'a [<$x Item>]> = x.[<$x:snake _group>].as_ref().into();
                        ret.append(&mut x);
                    }
                    ret
                }
            }

            impl<'a> From<&'a [<$x Group>]> for Vec<&'a [<$x Item>]> {
                fn from(x: &'a [<$x Group>]) -> Self {
                    let mut ret = Vec::new();
                    match &*x.[<$x:snake _group_group>] {
                        [<$x GroupGroup>]::[<LBrace $x ListRBrace>](x) => {
                            let mut x: Vec<&'a [<$x Item>]> = x.[<$x:snake _list>].as_ref().into();
                            ret.append(&mut x);
                        }
                        [<$x GroupGroup>]::[<$x Item>](x) => {
                            ret.push(x.[<$x:snake _item>].as_ref());
                        }
                    }
                    ret
                }
            }
        }
    };
}

macro_rules! list_to_item {
    ($x:ident) => {
        paste! {
            impl<'a> From<&'a [<$x List>]> for Vec<&'a [<$x Item>]> {
                fn from(x: &'a [<$x List>]) -> Self {
                    let mut ret = Vec::new();
                    {
                        ret.push(x.[<$x:snake _item>].as_ref());
                    }
                    for x in &x.[<$x:snake _list_list>] {
                        ret.push(x.[<$x:snake _item>].as_ref());
                    }
                    ret
                }
            }
        }
    };
}

macro_rules! group_to_item {
    ($x:ident) => {
        paste! {
            impl<'a> From<&'a [<$x Group>]> for Vec<&'a [<$x Item>]> {
                fn from(x: &'a [<$x Group>]) -> Self {
                    let mut ret = Vec::new();
                    match &*x.[<$x:snake _group_group>] {
                        [<$x GroupGroup>]::[<LBrace $x GroupGroupListRBrace>](x) => {
                            for x in &x.[<$x:snake _group_group_list>] {
                                let mut x: Vec<&'a [<$x Item>]> = x.[<$x:snake _group>].as_ref().into();
                                ret.append(&mut x);
                            }
                        }
                        [<$x GroupGroup>]::[<$x Item>](x) => {
                            ret.push(x.[<$x:snake _item>].as_ref());
                        }
                    }
                    ret
                }
            }
        }
    };
}

impl<'a> From<&'a ScopedIdentifier> for Vec<Option<&'a WithGenericArgument>> {
    fn from(value: &'a ScopedIdentifier) -> Self {
        let mut ret = Vec::new();
        match value.scoped_identifier_group.as_ref() {
            ScopedIdentifierGroup::IdentifierScopedIdentifierOpt(x) => {
                if let Some(ref x) = x.scoped_identifier_opt {
                    ret.push(Some(x.with_generic_argument.as_ref()));
                } else {
                    ret.push(None);
                }
            }
            ScopedIdentifierGroup::DollarIdentifier(_) => {
                ret.push(None);
            }
        }
        for x in &value.scoped_identifier_list {
            if let Some(ref x) = x.scoped_identifier_opt0 {
                ret.push(Some(x.with_generic_argument.as_ref()));
            } else {
                ret.push(None);
            }
        }
        ret
    }
}

impl<'a> From<&'a CaseCondition> for Vec<&'a RangeItem> {
    fn from(value: &'a CaseCondition) -> Self {
        let mut ret = Vec::new();
        ret.push(value.range_item.as_ref());

        for x in &value.case_condition_list {
            ret.push(x.range_item.as_ref());
        }

        ret
    }
}

impl<'a> From<&'a SwitchCondition> for Vec<&'a Expression> {
    fn from(value: &'a SwitchCondition) -> Self {
        let mut ret = Vec::new();
        ret.push(value.expression.as_ref());

        for x in &value.switch_condition_list {
            ret.push(x.expression.as_ref());
        }

        ret
    }
}

impl<'a> From<&'a Width> for Vec<&'a Expression> {
    fn from(value: &'a Width) -> Self {
        let mut ret = Vec::new();
        ret.push(value.expression.as_ref());

        for x in &value.width_list {
            ret.push(x.expression.as_ref());
        }

        ret
    }
}

impl<'a> From<&'a Array> for Vec<&'a Expression> {
    fn from(value: &'a Array) -> Self {
        let mut ret = Vec::new();
        ret.push(value.expression.as_ref());

        for x in &value.array_list {
            ret.push(x.expression.as_ref());
        }

        ret
    }
}

impl<'a> From<&'a AssignDestination> for Vec<&'a HierarchicalIdentifier> {
    fn from(value: &'a AssignDestination) -> Self {
        match value {
            AssignDestination::HierarchicalIdentifier(x) => {
                vec![x.hierarchical_identifier.as_ref()]
            }
            AssignDestination::LBraceAssignConcatenationListRBrace(x) => {
                let list: Vec<_> = x.assign_concatenation_list.as_ref().into();
                list.iter()
                    .map(|x| x.hierarchical_identifier.as_ref())
                    .collect()
            }
        }
    }
}

impl From<&Identifier> for ScopedIdentifier {
    fn from(value: &Identifier) -> Self {
        let scoped_identifier_group =
            Box::new(ScopedIdentifierGroup::IdentifierScopedIdentifierOpt(
                ScopedIdentifierGroupIdentifierScopedIdentifierOpt {
                    identifier: Box::new(value.clone()),
                    scoped_identifier_opt: None,
                },
            ));
        Self {
            scoped_identifier_group,
            scoped_identifier_list: vec![],
        }
    }
}

impl From<&Identifier> for ExpressionIdentifier {
    fn from(value: &Identifier) -> Self {
        let scoped_identifier: ScopedIdentifier = value.into();
        (&scoped_identifier).into()
    }
}

impl From<&ScopedIdentifier> for ExpressionIdentifier {
    fn from(value: &ScopedIdentifier) -> Self {
        Self {
            scoped_identifier: Box::new(value.clone()),
            expression_identifier_opt: None,
            expression_identifier_list: vec![],
            expression_identifier_list0: vec![],
        }
    }
}

impl From<&GenericArgIdentifier> for ExpressionIdentifier {
    fn from(value: &GenericArgIdentifier) -> Self {
        let exp_identifier_list: Vec<_> = value
            .generic_arg_identifier_list
            .iter()
            .map(|x| ExpressionIdentifierList0 {
                dot: x.dot.clone(),
                identifier: x.identifier.clone(),
                expression_identifier_list0_list: vec![],
            })
            .collect();
        Self {
            scoped_identifier: value.scoped_identifier.clone(),
            expression_identifier_opt: None,
            expression_identifier_list: vec![],
            expression_identifier_list0: exp_identifier_list,
        }
    }
}

impl From<&Number> for Factor {
    fn from(value: &Number) -> Self {
        Factor::Number(FactorNumber {
            number: Box::new(value.clone()),
        })
    }
}

impl From<&BooleanLiteral> for Factor {
    fn from(value: &BooleanLiteral) -> Self {
        Factor::BooleanLiteral(FactorBooleanLiteral {
            boolean_literal: Box::new(value.clone()),
        })
    }
}

impl From<&Identifier> for Factor {
    fn from(value: &Identifier) -> Self {
        let identifier_factor = IdentifierFactor {
            expression_identifier: Box::new(value.into()),
            identifier_factor_opt: None,
        };
        Factor::IdentifierFactor(FactorIdentifierFactor {
            identifier_factor: Box::new(identifier_factor),
        })
    }
}

impl From<&ExpressionIdentifier> for Factor {
    fn from(value: &ExpressionIdentifier) -> Self {
        let identifier_factor = IdentifierFactor {
            expression_identifier: Box::new(value.clone()),
            identifier_factor_opt: None,
        };
        Factor::IdentifierFactor(FactorIdentifierFactor {
            identifier_factor: Box::new(identifier_factor),
        })
    }
}

impl From<&GenericArgIdentifier> for Factor {
    fn from(value: &GenericArgIdentifier) -> Self {
        let identifier_factor = IdentifierFactor {
            expression_identifier: Box::new(value.into()),
            identifier_factor_opt: None,
        };
        Factor::IdentifierFactor(FactorIdentifierFactor {
            identifier_factor: Box::new(identifier_factor),
        })
    }
}

impl From<(&ExpressionIdentifier, &FunctionCall)> for Factor {
    fn from(value: (&ExpressionIdentifier, &FunctionCall)) -> Self {
        let function_call = IdentifierFactorOptGroupFunctionCall {
            function_call: Box::new(value.1.clone()),
        };
        let identifier_factor_opt_group = IdentifierFactorOptGroup::FunctionCall(function_call);
        let identifier_factor_opt = IdentifierFactorOpt {
            identifier_factor_opt_group: Box::new(identifier_factor_opt_group),
        };
        let identifier_factor = IdentifierFactor {
            expression_identifier: Box::new(value.0.clone()),
            identifier_factor_opt: Some(identifier_factor_opt),
        };
        Factor::IdentifierFactor(FactorIdentifierFactor {
            identifier_factor: Box::new(identifier_factor),
        })
    }
}

impl From<&FixedType> for Factor {
    fn from(value: &FixedType) -> Self {
        let fixed_type = FactorTypeGroupFixedType {
            fixed_type: Box::new(value.clone()),
        };
        let factor_type_group = FactorTypeGroup::FixedType(fixed_type);
        let factor_type = FactorType {
            factor_type_group: Box::new(factor_type_group),
        };
        let factor_type_factor = FactorTypeFactor {
            factor_type_factor_list: vec![],
            factor_type: Box::new(factor_type),
        };
        Factor::FactorTypeFactor(FactorFactorTypeFactor {
            factor_type_factor: Box::new(factor_type_factor),
        })
    }
}

impl From<Factor> for Expression {
    fn from(value: Factor) -> Self {
        let expression13 = Box::new(Expression13 {
            expression13_list: vec![],
            factor: Box::new(value),
        });
        let expression12 = Box::new(Expression12 {
            expression13,
            expression12_opt: None,
        });
        let expression11 = Box::new(Expression11 {
            expression12,
            expression11_list: vec![],
        });
        let expression10 = Box::new(Expression10 {
            expression11,
            expression10_list: vec![],
        });
        let expression09 = Box::new(Expression09 {
            expression10,
            expression09_list: vec![],
        });
        let expression08 = Box::new(Expression08 {
            expression09,
            expression08_list: vec![],
        });
        let expression07 = Box::new(Expression07 {
            expression08,
            expression07_list: vec![],
        });
        let expression06 = Box::new(Expression06 {
            expression07,
            expression06_list: vec![],
        });
        let expression05 = Box::new(Expression05 {
            expression06,
            expression05_list: vec![],
        });
        let expression04 = Box::new(Expression04 {
            expression05,
            expression04_list: vec![],
        });
        let expression03 = Box::new(Expression03 {
            expression04,
            expression03_list: vec![],
        });
        let expression02 = Box::new(Expression02 {
            expression03,
            expression02_list: vec![],
        });
        let expression01 = Box::new(Expression01 {
            expression02,
            expression01_list: vec![],
        });
        let if_expression = Box::new(IfExpression {
            if_expression_list: vec![],
            expression01,
        });
        Expression { if_expression }
    }
}

impl From<&Number> for Expression {
    fn from(value: &Number) -> Self {
        let factor: Factor = value.into();
        factor.into()
    }
}

impl From<&BooleanLiteral> for Expression {
    fn from(value: &BooleanLiteral) -> Self {
        let factor: Factor = value.into();
        factor.into()
    }
}

impl From<&Identifier> for Expression {
    fn from(value: &Identifier) -> Self {
        let factor: Factor = value.into();
        factor.into()
    }
}

impl From<&ExpressionIdentifier> for Expression {
    fn from(value: &ExpressionIdentifier) -> Self {
        let factor: Factor = value.into();
        factor.into()
    }
}

impl From<(&ExpressionIdentifier, &FunctionCall)> for Expression {
    fn from(value: (&ExpressionIdentifier, &FunctionCall)) -> Self {
        let factor: Factor = value.into();
        factor.into()
    }
}

impl From<&GenericArgIdentifier> for Expression {
    fn from(value: &GenericArgIdentifier) -> Self {
        let factor: Factor = value.into();
        factor.into()
    }
}

impl From<&FixedType> for Expression {
    fn from(value: &FixedType) -> Self {
        let factor: Factor = value.into();
        factor.into()
    }
}

impl<'a> From<&'a StatementBlock> for Vec<&'a StatementBlockItem> {
    fn from(value: &'a StatementBlock) -> Self {
        let mut ret = vec![];
        for x in &value.statement_block_list {
            let mut x: Vec<_> = x.statement_block_group.as_ref().into();
            ret.append(&mut x);
        }
        ret
    }
}

list_group_to_item!(Modport);
list_group_to_item!(Enum);
list_group_to_item!(StructUnion);
list_group_to_item!(InstParameter);
list_group_to_item!(InstPort);
list_group_to_item!(WithParameter);
list_group_to_item!(PortDeclaration);
list_to_item!(WithGenericParameter);
list_to_item!(WithGenericArgument);
list_to_item!(Attribute);
list_to_item!(Argument);
list_to_item!(Concatenation);
list_to_item!(AssignConcatenation);
list_to_item!(ArrayLiteral);
list_to_item!(StructConstructor);
group_to_item!(Module);
group_to_item!(Interface);
group_to_item!(Generate);
group_to_item!(Package);
group_to_item!(Description);
group_to_item!(StatementBlock);

impl Expression {
    pub fn unwrap_factor(&self) -> Option<&Factor> {
        let exp = &*self.if_expression;
        if !exp.if_expression_list.is_empty() {
            return None;
        }

        let exp = &*exp.expression01;
        if !exp.expression01_list.is_empty() {
            return None;
        }

        let exp = &*exp.expression02;
        if !exp.expression02_list.is_empty() {
            return None;
        }

        let exp = &*exp.expression03;
        if !exp.expression03_list.is_empty() {
            return None;
        }

        let exp = &*exp.expression04;
        if !exp.expression04_list.is_empty() {
            return None;
        }

        let exp = &*exp.expression05;
        if !exp.expression05_list.is_empty() {
            return None;
        }

        let exp = &*exp.expression06;
        if !exp.expression06_list.is_empty() {
            return None;
        }

        let exp = &*exp.expression07;
        if !exp.expression07_list.is_empty() {
            return None;
        }

        let exp = &*exp.expression08;
        if !exp.expression08_list.is_empty() {
            return None;
        }

        let exp = &*exp.expression09;
        if !exp.expression09_list.is_empty() {
            return None;
        }

        let exp = &*exp.expression10;
        if !exp.expression10_list.is_empty() {
            return None;
        }

        let exp = &*exp.expression11;
        if !exp.expression11_list.is_empty() {
            return None;
        }

        let exp = &*exp.expression12;
        if exp.expression12_opt.is_some() {
            return None;
        }

        let exp = &*exp.expression13;
        if !exp.expression13_list.is_empty() {
            return None;
        }

        Some(exp.factor.as_ref())
    }

    pub fn unwrap_identifier(&self) -> Option<&ExpressionIdentifier> {
        if let Some(Factor::IdentifierFactor(x)) = self.unwrap_factor()
            && x.identifier_factor.identifier_factor_opt.is_none()
        {
            return Some(x.identifier_factor.expression_identifier.as_ref());
        }

        None
    }

    pub fn is_assignable(&self) -> bool {
        let Some(factor) = self.unwrap_factor() else {
            return false;
        };

        match factor {
            Factor::IdentifierFactor(x) => {
                // Function call
                if x.identifier_factor.identifier_factor_opt.is_some() {
                    return false;
                }
                true
            }
            Factor::LBraceConcatenationListRBrace(x) => {
                let items: Vec<_> = x.concatenation_list.as_ref().into();
                items
                    .iter()
                    .all(|x| x.concatenation_item_opt.is_none() && x.expression.is_assignable())
            }
            _ => false,
        }
    }

    pub fn is_anonymous_expression(&self) -> bool {
        let Some(factor) = self.unwrap_factor() else {
            return false;
        };

        match factor {
            Factor::IdentifierFactor(x) => {
                let factor = &x.identifier_factor;

                if factor.identifier_factor_opt.is_some() {
                    return false;
                }

                let exp_identifier = &*factor.expression_identifier;
                if exp_identifier.expression_identifier_opt.is_some() {
                    return false;
                }
                if !exp_identifier.expression_identifier_list.is_empty() {
                    return false;
                }
                if !exp_identifier.expression_identifier_list0.is_empty() {
                    return false;
                }

                let scoped_identifier = &*exp_identifier.scoped_identifier;
                if !scoped_identifier.scoped_identifier_list.is_empty() {
                    return false;
                }

                let token = scoped_identifier.identifier().token;
                is_anonymous_token(&token)
            }
            _ => false,
        }
    }
}

impl ModuleDeclaration {
    pub fn collect_import_declarations(&self) -> Vec<ImportDeclaration> {
        let mut ret = Vec::new();
        for x in &self.module_declaration_list {
            ret.append(&mut x.module_group.collect_import_declarations());
        }
        ret
    }
}

impl ModuleGroup {
    pub fn collect_import_declarations(&self) -> Vec<ImportDeclaration> {
        let mut ret = Vec::new();
        match &*self.module_group_group {
            ModuleGroupGroup::LBraceModuleGroupGroupListRBrace(x) => {
                for x in &x.module_group_group_list {
                    ret.append(&mut x.module_group.collect_import_declarations());
                }
            }
            ModuleGroupGroup::ModuleItem(x) => {
                if let GenerateItem::ImportDeclaration(x) = &*x.module_item.generate_item {
                    ret.push(x.import_declaration.as_ref().clone());
                }
            }
        }

        ret
    }
}

impl InterfaceDeclaration {
    pub fn collect_import_declarations(&self) -> Vec<ImportDeclaration> {
        let mut ret = Vec::new();
        for x in &self.interface_declaration_list {
            ret.append(&mut x.interface_group.collect_import_declarations());
        }
        ret
    }
}

impl InterfaceGroup {
    pub fn collect_import_declarations(&self) -> Vec<ImportDeclaration> {
        let mut ret = Vec::new();
        match &*self.interface_group_group {
            InterfaceGroupGroup::LBraceInterfaceGroupGroupListRBrace(x) => {
                for x in &x.interface_group_group_list {
                    ret.append(&mut x.interface_group.collect_import_declarations());
                }
            }
            InterfaceGroupGroup::InterfaceItem(x) => {
                if let InterfaceItem::GenerateItem(x) = &*x.interface_item
                    && let GenerateItem::ImportDeclaration(x) = &*x.generate_item
                {
                    ret.push(x.import_declaration.as_ref().clone());
                }
            }
        }
        ret
    }
}

impl PackageDeclaration {
    pub fn collect_import_declarations(&self) -> Vec<ImportDeclaration> {
        let mut ret = Vec::new();
        for x in &self.package_declaration_list {
            ret.append(&mut x.package_group.collect_import_declarations());
        }
        ret
    }
}

impl PackageGroup {
    pub fn collect_import_declarations(&self) -> Vec<ImportDeclaration> {
        let mut ret = Vec::new();
        match &*self.package_group_group {
            PackageGroupGroup::LBracePackageGroupGroupListRBrace(x) => {
                for x in &x.package_group_group_list {
                    ret.append(&mut x.package_group.collect_import_declarations());
                }
            }
            PackageGroupGroup::PackageItem(x) => {
                if let PackageItem::ImportDeclaration(x) = &*x.package_item {
                    ret.push(x.import_declaration.as_ref().clone());
                }
            }
        }
        ret
    }
}

impl fmt::Display for Direction {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let token = match self {
            Direction::Input(x) => &x.input.input_token,
            Direction::Output(x) => &x.output.output_token,
            Direction::Inout(x) => &x.inout.inout_token,
            Direction::Modport(x) => &x.modport.modport_token,
            Direction::Import(x) => &x.import.import_token,
        };
        token.fmt(f)
    }
}

impl Identifier {
    pub fn text(&self) -> StrId {
        self.identifier_token.token.text
    }
}

impl ScopedIdentifier {
    pub fn get_scope_depth(&self) -> usize {
        self.scoped_identifier_list.len() + 1
    }
}

impl ExpressionIdentifier {
    pub fn last_select(&self) -> Vec<Select> {
        if self.expression_identifier_list0.is_empty() {
            self.expression_identifier_list
                .iter()
                .map(|x| x.select.as_ref().clone())
                .collect()
        } else {
            self.expression_identifier_list0
                .last()
                .unwrap()
                .expression_identifier_list0_list
                .iter()
                .map(|x| x.select.as_ref().clone())
                .collect()
        }
    }
}

impl HierarchicalIdentifier {
    pub fn last_select(&self) -> Vec<Select> {
        if self.hierarchical_identifier_list0.is_empty() {
            self.hierarchical_identifier_list
                .iter()
                .map(|x| x.select.as_ref().clone())
                .collect()
        } else {
            self.hierarchical_identifier_list0
                .last()
                .unwrap()
                .hierarchical_identifier_list0_list
                .iter()
                .map(|x| x.select.as_ref().clone())
                .collect()
        }
    }
}

impl AlwaysFfDeclaration {
    pub fn has_if_reset(&self) -> bool {
        let x = self.statement_block.statement_block_list.first();
        if x.is_none() {
            return false;
        }

        let x: Vec<_> = x.unwrap().statement_block_group.as_ref().into();
        if let Some(StatementBlockItem::Statement(x)) = x.first() {
            return matches!(*x.statement, Statement::IfResetStatement(_));
        }

        false
    }

    pub fn has_explicit_clock(&self) -> bool {
        self.always_ff_declaration_opt.is_some()
    }

    pub fn get_explicit_clock(&self) -> Option<HierarchicalIdentifier> {
        self.always_ff_declaration_opt.as_ref().map(|x| {
            x.always_ff_event_list
                .always_ff_clock
                .hierarchical_identifier
                .as_ref()
                .clone()
        })
    }

    pub fn has_explicit_reset(&self) -> bool {
        if let Some(x) = &self.always_ff_declaration_opt {
            return x.always_ff_event_list.always_ff_event_list_opt.is_some();
        }

        false
    }

    pub fn get_explicit_reset(&self) -> Option<HierarchicalIdentifier> {
        if let Some(x) = &self.always_ff_declaration_opt
            && let Some(x) = &x.always_ff_event_list.always_ff_event_list_opt
        {
            return Some(x.always_ff_reset.hierarchical_identifier.as_ref().clone());
        }

        None
    }
}

impl ProtoDeclaration {
    pub fn identifier_token(&self) -> VerylToken {
        match &*self.proto_declaration_group {
            ProtoDeclarationGroup::ProtoModuleDeclaration(x) => x
                .proto_module_declaration
                .identifier
                .identifier_token
                .clone(),
            ProtoDeclarationGroup::ProtoInterfaceDeclaration(x) => x
                .proto_interface_declaration
                .identifier
                .identifier_token
                .clone(),
            ProtoDeclarationGroup::ProtoPackageDeclaration(x) => x
                .proto_package_declaration
                .identifier
                .identifier_token
                .clone(),
        }
    }
}

impl PublicDescriptionItem {
    pub fn identifier_token(&self) -> VerylToken {
        match self {
            PublicDescriptionItem::ModuleDeclaration(x) => {
                x.module_declaration.identifier.identifier_token.clone()
            }
            PublicDescriptionItem::InterfaceDeclaration(x) => {
                x.interface_declaration.identifier.identifier_token.clone()
            }
            PublicDescriptionItem::PackageDeclaration(x) => {
                x.package_declaration.identifier.identifier_token.clone()
            }
            PublicDescriptionItem::AliasDeclaration(x) => {
                x.alias_declaration.identifier.identifier_token.clone()
            }
            PublicDescriptionItem::ProtoDeclaration(x) => x.proto_declaration.identifier_token(),
        }
    }
}

impl DescriptionItem {
    pub fn is_generic(&self) -> bool {
        match self {
            DescriptionItem::DescriptionItemOptPublicDescriptionItem(x) => {
                match x.public_description_item.as_ref() {
                    PublicDescriptionItem::ModuleDeclaration(x) => {
                        x.module_declaration.module_declaration_opt.is_some()
                    }
                    PublicDescriptionItem::InterfaceDeclaration(x) => {
                        x.interface_declaration.interface_declaration_opt.is_some()
                    }
                    PublicDescriptionItem::PackageDeclaration(x) => {
                        x.package_declaration.package_declaration_opt.is_some()
                    }
                    _ => false,
                }
            }
            _ => false,
        }
    }

    pub fn identifier_token(&self) -> Option<VerylToken> {
        match self {
            DescriptionItem::DescriptionItemOptPublicDescriptionItem(x) => {
                Some(x.public_description_item.identifier_token())
            }
            DescriptionItem::ImportDeclaration(_) => None,
            DescriptionItem::BindDeclaration(x) => Some(
                x.bind_declaration
                    .component_instantiation
                    .identifier
                    .identifier_token
                    .clone(),
            ),
            DescriptionItem::EmbedDeclaration(x) => {
                Some(x.embed_declaration.identifier.identifier_token.clone())
            }
            DescriptionItem::IncludeDeclaration(x) => {
                Some(x.include_declaration.identifier.identifier_token.clone())
            }
        }
    }
}
