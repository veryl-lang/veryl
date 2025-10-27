use crate::HashMap;
use crate::conv::checker::proto::check_proto;
use crate::conv::{Affiliation, Context, Conv};
use crate::ir::{self, VarPath};
use crate::symbol::SymbolKind;
use crate::symbol_table;
use veryl_parser::veryl_grammar_trait::*;

impl Conv<&Veryl> for ir::Ir {
    fn conv(context: &mut Context, value: &Veryl) -> Self {
        let mut components = vec![];

        for x in &value.veryl_list {
            let items: Vec<_> = x.description_group.as_ref().into();
            for item in &items {
                if let DescriptionItem::DescriptionItemOptPublicDescriptionItem(x) = item {
                    match x.public_description_item.as_ref() {
                        PublicDescriptionItem::ModuleDeclaration(x) => {
                            let component = ir::Component::Module(Conv::conv(
                                context,
                                x.module_declaration.as_ref(),
                            ));

                            // ignore generic module as top components
                            if x.module_declaration.module_declaration_opt.is_none() {
                                components.push(component);
                            }
                        }
                        PublicDescriptionItem::InterfaceDeclaration(x) => {
                            // ignore Interface as top components
                            let _: ir::Interface =
                                Conv::conv(context, x.interface_declaration.as_ref());
                        }
                        PublicDescriptionItem::PackageDeclaration(x) => {
                            let _: () = Conv::conv(context, x.package_declaration.as_ref());
                        }
                        PublicDescriptionItem::ProtoDeclaration(x) => {
                            match x.proto_declaration.proto_declaration_group.as_ref() {
                                ProtoDeclarationGroup::ProtoModuleDeclaration(x) => {
                                    let _: () =
                                        Conv::conv(context, x.proto_module_declaration.as_ref());
                                }
                                ProtoDeclarationGroup::ProtoInterfaceDeclaration(x) => {
                                    let _: () =
                                        Conv::conv(context, x.proto_interface_declaration.as_ref());
                                }
                                ProtoDeclarationGroup::ProtoPackageDeclaration(x) => {
                                    let _: () =
                                        Conv::conv(context, x.proto_package_declaration.as_ref());
                                }
                            }
                        }
                        _ => (),
                    }
                }
            }
        }

        ir::Ir { components }
    }
}

impl Conv<&ModuleDeclaration> for ir::Module {
    fn conv(context: &mut Context, value: &ModuleDeclaration) -> Self {
        let mut declarations = vec![];

        // each module has independent context
        let mut local_context = Context::default();
        context.inherit(&mut local_context);
        local_context.affiliation.push(Affiliation::Module);

        if let Ok(symbol) = symbol_table::resolve(value.identifier.as_ref())
            && let SymbolKind::Module(x) = symbol.found.kind
        {
            if let Some(x) = x.default_clock {
                local_context
                    .set_default_clock(VarPath::new(symbol_table::get(x).unwrap().token.text));
            }
            if let Some(x) = x.default_reset {
                local_context
                    .set_default_reset(VarPath::new(symbol_table::get(x).unwrap().token.text));
            }
        }

        if let Some(x) = &value.module_declaration_opt {
            let items: Vec<_> = x
                .with_generic_parameter
                .with_generic_parameter_list
                .as_ref()
                .into();
            for item in items {
                let _: () = Conv::conv(&mut local_context, item);
            }
        }

        if let Some(x) = &value.module_declaration_opt0 {
            check_proto(&mut local_context, &value.identifier, &x.scoped_identifier);
        }

        if let Some(x) = &value.module_declaration_opt1
            && let Some(x) = &x.with_parameter.with_parameter_opt
        {
            let items: Vec<_> = x.with_parameter_list.as_ref().into();
            for item in items {
                let _: () = Conv::conv(&mut local_context, item);
            }
        }

        if let Some(x) = &value.module_declaration_opt2
            && let Some(x) = &x.port_declaration.port_declaration_opt
        {
            let items: Vec<_> = x.port_declaration_list.as_ref().into();
            for item in items {
                let _: () = Conv::conv(&mut local_context, item);
            }
        }

        for x in &value.module_declaration_list {
            let items: Vec<_> = x.module_group.as_ref().into();
            for item in &items {
                let mut block: ir::DeclarationBlock =
                    Conv::conv(&mut local_context, item.generate_item.as_ref());
                declarations.append(&mut block.0);
            }
        }

        declarations.retain(|x| !x.is_null());
        let variables = local_context.drain_variables();
        let functions = local_context.drain_functions();

        let mut ports = HashMap::default();

        for (id, var) in &variables {
            if var.kind.is_port() {
                ports.insert(var.path.clone(), *id);
            }
        }

        local_context.inherit(context);

        ir::Module {
            name: value.identifier.text(),
            ports,
            variables,
            functions,
            declarations,
        }
    }
}

impl Conv<&InterfaceDeclaration> for ir::Interface {
    fn conv(context: &mut Context, value: &InterfaceDeclaration) -> Self {
        // each interface has independent context
        let mut local_context = Context::default();
        context.inherit(&mut local_context);
        local_context.affiliation.push(Affiliation::Interface);

        if let Some(x) = &value.interface_declaration_opt {
            let items: Vec<_> = x
                .with_generic_parameter
                .with_generic_parameter_list
                .as_ref()
                .into();
            for item in items {
                let _: () = Conv::conv(&mut local_context, item);
            }
        }

        if let Some(x) = &value.interface_declaration_opt0 {
            check_proto(&mut local_context, &value.identifier, &x.scoped_identifier);
        }

        if let Some(x) = &value.interface_declaration_opt1
            && let Some(x) = &x.with_parameter.with_parameter_opt
        {
            let items: Vec<_> = x.with_parameter_list.as_ref().into();
            for item in items {
                let _: () = Conv::conv(&mut local_context, item);
            }
        }

        for x in &value.interface_declaration_list {
            let items: Vec<_> = x.interface_group.as_ref().into();
            for item in items {
                match item {
                    InterfaceItem::GenerateItem(x) => {
                        let _: ir::DeclarationBlock =
                            Conv::conv(&mut local_context, x.generate_item.as_ref());
                    }
                    InterfaceItem::ModportDeclaration(x) => {
                        let _: () = Conv::conv(&mut local_context, x.modport_declaration.as_ref());
                    }
                }
            }
        }

        let var_paths = local_context.drain_var_paths();
        let func_paths = local_context.drain_func_paths();
        let variables = local_context.drain_variables();
        let functions = local_context.drain_functions();

        local_context.inherit(context);

        ir::Interface {
            name: value.identifier.text(),
            var_paths,
            func_paths,
            variables,
            functions,
        }
    }
}

impl Conv<&PackageDeclaration> for () {
    fn conv(context: &mut Context, value: &PackageDeclaration) -> Self {
        context.affiliation.push(Affiliation::Package);

        if let Some(x) = &value.package_declaration_opt0 {
            check_proto(context, &value.identifier, &x.scoped_identifier);
        }

        for x in &value.package_declaration_list {
            let items: Vec<_> = x.package_group.as_ref().into();
            for item in items {
                match item {
                    PackageItem::ConstDeclaration(x) => {
                        let _: ir::Declaration = Conv::conv(context, x.const_declaration.as_ref());
                    }
                    PackageItem::FunctionDeclaration(x) => {
                        let _: () = Conv::conv(context, x.function_declaration.as_ref());
                    }
                    _ => (),
                }
            }
        }

        context.affiliation.pop();
    }
}

impl Conv<&ProtoModuleDeclaration> for () {
    fn conv(context: &mut Context, _value: &ProtoModuleDeclaration) -> Self {
        context.affiliation.push(Affiliation::ProtoModule);
        context.affiliation.pop();
    }
}

impl Conv<&ProtoInterfaceDeclaration> for () {
    fn conv(context: &mut Context, _value: &ProtoInterfaceDeclaration) -> Self {
        context.affiliation.push(Affiliation::ProtoInterface);
        context.affiliation.pop();
    }
}

impl Conv<&ProtoPackageDeclaration> for () {
    fn conv(context: &mut Context, _value: &ProtoPackageDeclaration) -> Self {
        context.affiliation.push(Affiliation::ProtoPackage);
        context.affiliation.pop();
    }
}
