use crate::conv::checker::alias::{AliasType, check_alias_target};
use crate::conv::checker::clock_domain::check_clock_domain;
use crate::conv::checker::generic::check_generic_bound;
use crate::conv::checker::proto::check_proto;
use crate::conv::utils::check_module_with_unevaluable_generic_parameters;
use crate::conv::{Affiliation, Context, Conv};
use crate::ir::{self, IrResult, VarPath};
use crate::symbol::SymbolKind;
use crate::symbol_table;
use crate::{HashMap, ir_error};
use veryl_parser::token_range::TokenRange;
use veryl_parser::veryl_grammar_trait::*;

impl Conv<&Veryl> for ir::Ir {
    fn conv(context: &mut Context, value: &Veryl) -> IrResult<Self> {
        let mut components = vec![];

        for x in &value.veryl_list {
            let items: Vec<_> = x.description_group.as_ref().into();
            for item in &items {
                // ignore IrError of generic top-level components
                let use_ir = context.config.use_ir;
                if item.is_generic() {
                    context.config.use_ir = false;
                }

                match item {
                    DescriptionItem::DescriptionItemOptPublicDescriptionItem(x) => {
                        match x.public_description_item.as_ref() {
                            PublicDescriptionItem::ModuleDeclaration(x) => {
                                let ret: IrResult<ir::Module> =
                                    Conv::conv(context, x.module_declaration.as_ref());
                                context.insert_ir_error(&ret);

                                if let Ok(mut component) = ret {
                                    // suppress unassigned check for modules with unevaluable generic parameters
                                    if check_module_with_unevaluable_generic_parameters(
                                        &x.module_declaration.identifier,
                                    ) {
                                        component.suppress_unassigned = true;
                                    }

                                    components.push(ir::Component::Module(component));
                                }
                            }
                            PublicDescriptionItem::InterfaceDeclaration(x) => {
                                let ret: IrResult<ir::Interface> =
                                    Conv::conv(context, x.interface_declaration.as_ref());
                                context.insert_ir_error(&ret);
                            }
                            PublicDescriptionItem::PackageDeclaration(x) => {
                                let ret: IrResult<()> =
                                    Conv::conv(context, x.package_declaration.as_ref());
                                context.insert_ir_error(&ret);
                            }
                            PublicDescriptionItem::ProtoDeclaration(x) => {
                                match x.proto_declaration.proto_declaration_group.as_ref() {
                                    ProtoDeclarationGroup::ProtoModuleDeclaration(x) => {
                                        let ret: IrResult<ir::Module> = Conv::conv(
                                            context,
                                            x.proto_module_declaration.as_ref(),
                                        );
                                        context.insert_ir_error(&ret);
                                    }
                                    ProtoDeclarationGroup::ProtoInterfaceDeclaration(x) => {
                                        let ret: IrResult<()> = Conv::conv(
                                            context,
                                            x.proto_interface_declaration.as_ref(),
                                        );
                                        context.insert_ir_error(&ret);
                                    }
                                    ProtoDeclarationGroup::ProtoPackageDeclaration(x) => {
                                        let ret: IrResult<()> = Conv::conv(
                                            context,
                                            x.proto_package_declaration.as_ref(),
                                        );
                                        context.insert_ir_error(&ret);
                                    }
                                }
                            }
                            PublicDescriptionItem::AliasDeclaration(x) => {
                                let ret: IrResult<()> =
                                    Conv::conv(context, x.alias_declaration.as_ref());
                                context.insert_ir_error(&ret);
                            }
                        }
                    }
                    DescriptionItem::BindDeclaration(x) => {
                        let ret: IrResult<()> = Conv::conv(context, x.bind_declaration.as_ref());
                        context.insert_ir_error(&ret);
                    }
                    DescriptionItem::EmbedDeclaration(x) => {
                        let ret: IrResult<()> = Conv::conv(context, x.embed_declaration.as_ref());
                        context.insert_ir_error(&ret);
                    }
                    DescriptionItem::ImportDeclaration(x) => {
                        let ret: IrResult<ir::DeclarationBlock> =
                            Conv::conv(context, x.import_declaration.as_ref());
                        context.insert_ir_error(&ret);
                    }
                    _ => (),
                }

                if item.is_generic() {
                    context.config.use_ir = use_ir;
                }
            }
        }

        Ok(ir::Ir { components })
    }
}

impl Conv<&ModuleDeclaration> for ir::Module {
    fn conv(context: &mut Context, value: &ModuleDeclaration) -> IrResult<Self> {
        let mut declarations = vec![];

        // each top-level component has independent context
        let upper_context = context;
        let mut context = Context::default();
        context.inherit(upper_context);

        // pop_affiliation is not necessary because the local `context` will be dropped
        context.push_affiliation(Affiliation::Module);

        if let Ok(symbol) = symbol_table::resolve(value.identifier.as_ref())
            && let SymbolKind::Module(x) = symbol.found.kind
        {
            if let Some(x) = x.default_clock {
                let path = VarPath::new(symbol_table::get(x).unwrap().token.text);
                context.set_default_clock(path, x);
            }
            if let Some(x) = x.default_reset {
                let path = VarPath::new(symbol_table::get(x).unwrap().token.text);
                context.set_default_reset(path, x);
            }
        } else {
            let token: TokenRange = value.identifier.as_ref().into();
            return Err(ir_error!(token));
        }

        if let Some(x) = &value.module_declaration_opt {
            check_generic_bound(&mut context, &x.with_generic_parameter);
            let items: Vec<_> = x
                .with_generic_parameter
                .with_generic_parameter_list
                .as_ref()
                .into();
            for item in items {
                let ret: IrResult<()> = Conv::conv(&mut context, item);
                context.insert_ir_error(&ret);
            }
        }

        if let Some(x) = &value.module_declaration_opt0 {
            check_proto(&mut context, &value.identifier, &x.scoped_identifier);
        }

        if let Some(x) = &value.module_declaration_opt1
            && let Some(x) = &x.with_parameter.with_parameter_opt
        {
            let items: Vec<_> = x.with_parameter_list.as_ref().into();
            for item in items {
                let ret: IrResult<()> = Conv::conv(&mut context, item);
                context.insert_ir_error(&ret);
            }
        }

        if let Some(x) = &value.module_declaration_opt2
            && let Some(x) = &x.port_declaration.port_declaration_opt
        {
            let items: Vec<_> = x.port_declaration_list.as_ref().into();
            for item in items {
                let ret: IrResult<()> = Conv::conv(&mut context, item);
                context.insert_ir_error(&ret);
            }
        }

        for x in &value.module_declaration_list {
            let items: Vec<_> = x.module_group.as_ref().into();
            for item in &items {
                let ret: IrResult<ir::DeclarationBlock> =
                    Conv::conv(&mut context, item.generate_item.as_ref());
                context.insert_ir_error(&ret);

                if let Ok(mut block) = ret {
                    declarations.append(&mut block.0);
                }
            }
        }

        // This check must be after default clock/reset are registered in context
        if let (Some(clock), Some(reset)) =
            (context.get_default_clock(), context.get_default_reset())
        {
            check_clock_domain(
                &mut context,
                &clock.0.comptime,
                &reset.0.comptime,
                &value.module.module_token.token,
            );
        }

        declarations.retain(|x| !x.is_null());
        let port_types = context.drain_port_types();
        let variables = context.drain_variables();
        let functions = context.drain_functions();

        let mut ports = HashMap::default();

        for (id, var) in &variables {
            if var.kind.is_port() {
                ports.insert(var.path.clone(), *id);
            }
        }

        upper_context.inherit(&mut context);

        Ok(ir::Module {
            name: value.identifier.text(),
            ports,
            port_types,
            variables,
            functions,
            declarations,
            suppress_unassigned: false,
        })
    }
}

impl Conv<&InterfaceDeclaration> for ir::Interface {
    fn conv(context: &mut Context, value: &InterfaceDeclaration) -> IrResult<Self> {
        // each top-level component has independent context
        let upper_context = context;
        let mut context = Context::default();
        context.inherit(upper_context);

        // pop_affiliation is not necessary because the local `context` will be dropped
        context.push_affiliation(Affiliation::Interface);

        if let Some(x) = &value.interface_declaration_opt {
            check_generic_bound(&mut context, &x.with_generic_parameter);
            let items: Vec<_> = x
                .with_generic_parameter
                .with_generic_parameter_list
                .as_ref()
                .into();
            for item in items {
                let ret: IrResult<()> = Conv::conv(&mut context, item);
                context.insert_ir_error(&ret);
            }
        }

        if let Some(x) = &value.interface_declaration_opt0 {
            check_proto(&mut context, &value.identifier, &x.scoped_identifier);
        }

        if let Some(x) = &value.interface_declaration_opt1
            && let Some(x) = &x.with_parameter.with_parameter_opt
        {
            let items: Vec<_> = x.with_parameter_list.as_ref().into();
            for item in items {
                let ret: IrResult<()> = Conv::conv(&mut context, item);
                context.insert_ir_error(&ret);
            }
        }

        for x in &value.interface_declaration_list {
            let items: Vec<_> = x.interface_group.as_ref().into();
            for item in items {
                match item {
                    InterfaceItem::GenerateItem(x) => {
                        let ret: IrResult<ir::DeclarationBlock> =
                            Conv::conv(&mut context, x.generate_item.as_ref());
                        context.insert_ir_error(&ret);
                    }
                    InterfaceItem::ModportDeclaration(x) => {
                        let ret: IrResult<()> =
                            Conv::conv(&mut context, x.modport_declaration.as_ref());
                        context.insert_ir_error(&ret);
                    }
                }
            }
        }

        let var_paths = context.drain_var_paths();
        let func_paths = context.drain_func_paths();
        let mut variables = context.drain_variables();
        let functions = context.drain_functions();
        let modports = context.drain_modports();

        let variables = variables
            .extract_if(|_, v| v.affiliation != Affiliation::Function)
            .collect();

        upper_context.inherit(&mut context);

        Ok(ir::Interface {
            name: value.identifier.text(),
            var_paths,
            func_paths,
            variables,
            functions,
            modports,
        })
    }
}

impl Conv<&PackageDeclaration> for () {
    fn conv(context: &mut Context, value: &PackageDeclaration) -> IrResult<Self> {
        // each top-level component has independent context
        let upper_context = context;
        let mut context = Context::default();
        context.inherit(upper_context);

        // pop_affiliation is not necessary because the local `context` will be dropped
        context.push_affiliation(Affiliation::Package);

        if let Some(x) = &value.package_declaration_opt {
            check_generic_bound(&mut context, &x.with_generic_parameter);
        }

        if let Some(x) = &value.package_declaration_opt0 {
            check_proto(&mut context, &value.identifier, &x.scoped_identifier);
        }

        for x in &value.package_declaration_list {
            let items: Vec<_> = x.package_group.as_ref().into();
            for item in items {
                match item {
                    PackageItem::ConstDeclaration(x) => {
                        let ret: IrResult<ir::Declaration> =
                            Conv::conv(&mut context, x.const_declaration.as_ref());
                        context.insert_ir_error(&ret);
                    }
                    PackageItem::FunctionDeclaration(x) => {
                        let ret: IrResult<()> =
                            Conv::conv(&mut context, x.function_declaration.as_ref());
                        context.insert_ir_error(&ret);
                    }
                    _ => (),
                }
            }
        }

        upper_context.inherit(&mut context);

        Ok(())
    }
}

impl Conv<&ProtoModuleDeclaration> for ir::Module {
    fn conv(context: &mut Context, value: &ProtoModuleDeclaration) -> IrResult<Self> {
        // each top-level component has independent context
        let upper_context = context;
        let mut context = Context::default();
        context.inherit(upper_context);

        // pop_affiliation is not necessary because the local `context` will be dropped
        context.push_affiliation(Affiliation::ProtoModule);

        if let Some(x) = &value.proto_module_declaration_opt
            && let Some(x) = &x.with_parameter.with_parameter_opt
        {
            let items: Vec<_> = x.with_parameter_list.as_ref().into();
            for item in items {
                let ret: IrResult<()> = Conv::conv(&mut context, item);
                context.insert_ir_error(&ret);
            }
        }

        if let Some(x) = &value.proto_module_declaration_opt0
            && let Some(x) = &x.port_declaration.port_declaration_opt
        {
            let items: Vec<_> = x.port_declaration_list.as_ref().into();
            for item in items {
                let ret: IrResult<()> = Conv::conv(&mut context, item);
                context.insert_ir_error(&ret);
            }
        }

        let port_types = context.drain_port_types();
        let variables = context.drain_variables();

        let mut ports = HashMap::default();

        for (id, var) in &variables {
            if var.kind.is_port() {
                ports.insert(var.path.clone(), *id);
            }
        }

        upper_context.inherit(&mut context);

        Ok(ir::Module {
            name: value.identifier.text(),
            ports,
            port_types,
            variables,
            functions: HashMap::default(),
            declarations: vec![],
            suppress_unassigned: false,
        })
    }
}

impl Conv<&ProtoInterfaceDeclaration> for () {
    fn conv(context: &mut Context, value: &ProtoInterfaceDeclaration) -> IrResult<Self> {
        context.push_affiliation(Affiliation::ProtoInterface);

        for x in &value.proto_interface_declaration_list {
            if let ProtoInterfaceItem::ProtoAliasDeclaration(x) = x.proto_interface_item.as_ref() {
                let r#type = match x
                    .proto_alias_declaration
                    .proto_alias_declaration_group
                    .as_ref()
                {
                    ProtoAliasDeclarationGroup::Module(_) => AliasType::ProtoModule,
                    ProtoAliasDeclarationGroup::Interface(_) => AliasType::ProtoInterface,
                    ProtoAliasDeclarationGroup::Package(_) => AliasType::ProtoPackage,
                };
                check_alias_target(
                    context,
                    &x.proto_alias_declaration.scoped_identifier,
                    r#type,
                );
            }
        }

        context.pop_affiliation();
        Ok(())
    }
}

impl Conv<&ProtoPackageDeclaration> for () {
    fn conv(context: &mut Context, value: &ProtoPackageDeclaration) -> IrResult<Self> {
        context.push_affiliation(Affiliation::ProtoPackage);

        for x in &value.proto_package_declaration_list {
            if let ProtoPacakgeItem::ProtoAliasDeclaration(x) = x.proto_pacakge_item.as_ref() {
                let r#type = match x
                    .proto_alias_declaration
                    .proto_alias_declaration_group
                    .as_ref()
                {
                    ProtoAliasDeclarationGroup::Module(_) => AliasType::ProtoModule,
                    ProtoAliasDeclarationGroup::Interface(_) => AliasType::ProtoInterface,
                    ProtoAliasDeclarationGroup::Package(_) => AliasType::ProtoPackage,
                };
                check_alias_target(
                    context,
                    &x.proto_alias_declaration.scoped_identifier,
                    r#type,
                );
            }
        }

        context.pop_affiliation();
        Ok(())
    }
}
