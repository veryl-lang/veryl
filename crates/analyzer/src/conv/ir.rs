use crate::conv::checker::proto::check_proto;
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
                let emit_ir_error = context.emit_ir_error;
                if item.is_generic() {
                    context.emit_ir_error = false;
                }

                if let DescriptionItem::DescriptionItemOptPublicDescriptionItem(x) = item {
                    match x.public_description_item.as_ref() {
                        PublicDescriptionItem::ModuleDeclaration(x) => {
                            let ret: IrResult<ir::Module> =
                                Conv::conv(context, x.module_declaration.as_ref());
                            context.insert_ir_error(&ret);

                            if let Ok(component) = ret {
                                // ignore generic module as top-level components
                                if x.module_declaration.module_declaration_opt.is_none() {
                                    components.push(ir::Component::Module(component));
                                }
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
                            let ret: IrResult<()> =
                                match x.proto_declaration.proto_declaration_group.as_ref() {
                                    ProtoDeclarationGroup::ProtoModuleDeclaration(x) => {
                                        Conv::conv(context, x.proto_module_declaration.as_ref())
                                    }
                                    ProtoDeclarationGroup::ProtoInterfaceDeclaration(x) => {
                                        Conv::conv(context, x.proto_interface_declaration.as_ref())
                                    }
                                    ProtoDeclarationGroup::ProtoPackageDeclaration(x) => {
                                        Conv::conv(context, x.proto_package_declaration.as_ref())
                                    }
                                };
                            context.insert_ir_error(&ret);
                        }
                        _ => (),
                    }
                }

                if item.is_generic() {
                    context.emit_ir_error = emit_ir_error;
                }
            }
        }

        Ok(ir::Ir { components })
    }
}

impl Conv<&ModuleDeclaration> for ir::Module {
    fn conv(context: &mut Context, value: &ModuleDeclaration) -> IrResult<Self> {
        let mut declarations = vec![];

        // each module has independent context
        let upper_context = context;
        let mut context = Context::default();
        context.inherit(upper_context);
        context.affiliation.push(Affiliation::Module);

        if let Ok(symbol) = symbol_table::resolve(value.identifier.as_ref())
            && let SymbolKind::Module(x) = symbol.found.kind
        {
            if let Some(x) = x.default_clock {
                context.set_default_clock(VarPath::new(symbol_table::get(x).unwrap().token.text));
            }
            if let Some(x) = x.default_reset {
                context.set_default_reset(VarPath::new(symbol_table::get(x).unwrap().token.text));
            }
        } else {
            let token: TokenRange = value.identifier.as_ref().into();
            return Err(ir_error!(token));
        }

        if let Some(x) = &value.module_declaration_opt {
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
        })
    }
}

impl Conv<&InterfaceDeclaration> for ir::Interface {
    fn conv(context: &mut Context, value: &InterfaceDeclaration) -> IrResult<Self> {
        // each interface has independent context
        let upper_context = context;
        let mut context = Context::default();
        context.inherit(upper_context);
        context.affiliation.push(Affiliation::Interface);

        if let Some(x) = &value.interface_declaration_opt {
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
        })
    }
}

impl Conv<&PackageDeclaration> for () {
    fn conv(context: &mut Context, value: &PackageDeclaration) -> IrResult<Self> {
        context.affiliation.push(Affiliation::Package);

        if let Some(x) = &value.package_declaration_opt0 {
            check_proto(context, &value.identifier, &x.scoped_identifier);
        }

        for x in &value.package_declaration_list {
            let items: Vec<_> = x.package_group.as_ref().into();
            for item in items {
                match item {
                    PackageItem::ConstDeclaration(x) => {
                        let ret: IrResult<ir::Declaration> =
                            Conv::conv(context, x.const_declaration.as_ref());
                        context.insert_ir_error(&ret);
                    }
                    PackageItem::FunctionDeclaration(x) => {
                        let ret: IrResult<()> =
                            Conv::conv(context, x.function_declaration.as_ref());
                        context.insert_ir_error(&ret);
                    }
                    _ => (),
                }
            }
        }

        context.affiliation.pop();
        Ok(())
    }
}

impl Conv<&ProtoModuleDeclaration> for () {
    fn conv(context: &mut Context, _value: &ProtoModuleDeclaration) -> IrResult<Self> {
        context.affiliation.push(Affiliation::ProtoModule);
        context.affiliation.pop();
        Ok(())
    }
}

impl Conv<&ProtoInterfaceDeclaration> for () {
    fn conv(context: &mut Context, _value: &ProtoInterfaceDeclaration) -> IrResult<Self> {
        context.affiliation.push(Affiliation::ProtoInterface);
        context.affiliation.pop();
        Ok(())
    }
}

impl Conv<&ProtoPackageDeclaration> for () {
    fn conv(context: &mut Context, _value: &ProtoPackageDeclaration) -> IrResult<Self> {
        context.affiliation.push(Affiliation::ProtoPackage);
        context.affiliation.pop();
        Ok(())
    }
}
