use crate::{
    analyzer_error::AnalyzerError,
    symbol_table::SymbolPathNamespace,
    type_dag::{self, Context, DagError},
};
use veryl_parser::{
    resource_table,
    veryl_grammar_trait::{
        DescriptionItem, EnumDeclaration, InterfaceDeclaration, ModuleDeclaration,
        PackageDeclaration, ScopedIdentifier, StructUnion, StructUnionDeclaration,
        TypeDefDeclaration, Veryl, VerylGrammarTrait,
    },
    veryl_token::Token,
    ParolError,
};
use veryl_parser::{
    veryl_grammar_trait::Identifier,
    veryl_walker::{Handler, HandlerPoint},
};

#[derive(Default)]
pub struct CreateTypeDag<'a> {
    text: &'a str,
    pub errors: Vec<AnalyzerError>,
    parent: Vec<u32>,
    point: HandlerPoint,
    ctx: Vec<Context>,
    file_scope_import: Vec<Option<u32>>,
}

impl<'a> CreateTypeDag<'a> {
    pub fn new(text: &'a str) -> Self {
        Self {
            text,
            ..Default::default()
        }
    }

    fn insert_node(
        &mut self,
        path: &SymbolPathNamespace,
        name: &str,
        token: &Token,
    ) -> Option<u32> {
        match type_dag::insert_node(path, name, token) {
            Ok(n) => Some(n),
            Err(e) => {
                self.errors.push(self.to_analyzer_error(e));
                None
            }
        }
    }

    fn to_analyzer_error(&self, de: DagError) -> AnalyzerError {
        match de {
            DagError::Cyclic(s, e) => {
                let start = match resource_table::get_str_value(s.token.text) {
                    Some(s) => s,
                    None => "<unknown StrId>".into(),
                };
                let end = match resource_table::get_str_value(e.token.text) {
                    Some(s) => s,
                    None => "<unknown StrId>".into(),
                };
                AnalyzerError::cyclic_type_dependency(self.text, &start, &end, &e.token.into())
            }
            DagError::UnableToResolve(b) => {
                let t = b.as_ref();
                AnalyzerError::undefined_identifier(&t.name, self.text, &t.token.into())
            }
        }
    }

    fn insert_edge(&mut self, s: u32, e: u32, edge: Context) {
        // Reversing this order to make traversal work
        match type_dag::insert_edge(e, s, edge) {
            Ok(_) => {}
            Err(er) => {
                self.errors.push(self.to_analyzer_error(er));
            }
        }
    }
}

impl<'a> Handler for CreateTypeDag<'a> {
    fn set_point(&mut self, p: HandlerPoint) {
        self.point = p;
    }
}

impl<'a> VerylGrammarTrait for CreateTypeDag<'a> {
    fn struct_union_declaration(&mut self, arg: &StructUnionDeclaration) -> Result<(), ParolError> {
        match self.point {
            HandlerPoint::Before => {
                let path: SymbolPathNamespace = arg.identifier.as_ref().into();
                let name = arg.identifier.identifier_token.to_string();
                let token = arg.identifier.identifier_token.token;
                if let Some(x) = self.insert_node(&path, &name, &token) {
                    self.parent.push(x)
                }
                // Unused for now, but will be useful in the future
                // to do this struct vs union chec
                match &*arg.struct_union {
                    StructUnion::Struct(_) => self.ctx.push(Context::Struct),
                    StructUnion::Union(_) => self.ctx.push(Context::Union),
                }
            }
            HandlerPoint::After => {
                self.parent.pop();
                self.ctx.pop();
            }
        }
        Ok(())
    }

    fn type_def_declaration(&mut self, arg: &TypeDefDeclaration) -> Result<(), ParolError> {
        match self.point {
            HandlerPoint::Before => {
                let path: SymbolPathNamespace = arg.identifier.as_ref().into();
                let name = arg.identifier.identifier_token.to_string();
                let token = arg.identifier.identifier_token.token;
                if let Some(x) = self.insert_node(&path, &name, &token) {
                    self.parent.push(x)
                }
                self.ctx.push(Context::TypeDef);
            }
            HandlerPoint::After => {
                self.parent.pop();
                self.ctx.pop();
            }
        }
        Ok(())
    }

    fn scoped_identifier(&mut self, arg: &ScopedIdentifier) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            if !self.ctx.is_empty() {
                let path: SymbolPathNamespace = arg.into();
                let name = to_string(arg);
                let token = arg.identifier.identifier_token.token;
                let child = self.insert_node(&path, &name, &token);
                if let (Some(parent), Some(child)) = (self.parent.last(), child) {
                    self.insert_edge(*parent, child, *self.ctx.last().unwrap());
                }
            }
        }
        Ok(())
    }

    fn enum_declaration(&mut self, arg: &EnumDeclaration) -> Result<(), ParolError> {
        match self.point {
            HandlerPoint::Before => {
                let path: SymbolPathNamespace = arg.identifier.as_ref().into();
                let name = arg.identifier.identifier_token.to_string();
                let token = arg.identifier.identifier_token.token;
                if let Some(x) = self.insert_node(&path, &name, &token) {
                    self.parent.push(x)
                }
                self.ctx.push(Context::Enum);
            }
            HandlerPoint::After => {
                self.parent.pop();
                self.ctx.pop();
            }
        }
        Ok(())
    }

    fn module_declaration(&mut self, arg: &ModuleDeclaration) -> Result<(), ParolError> {
        match self.point {
            HandlerPoint::Before => {
                let path: SymbolPathNamespace = arg.identifier.as_ref().into();
                let name = arg.identifier.identifier_token.to_string();
                let token = arg.identifier.identifier_token.token;
                if let Some(x) = self.insert_node(&path, &name, &token) {
                    self.parent.push(x)
                }
                self.ctx.push(Context::Module);

                for child in &self.file_scope_import.clone() {
                    if let (Some(parent), Some(child)) = (self.parent.last(), child) {
                        self.insert_edge(*parent, *child, *self.ctx.last().unwrap());
                    }
                }
            }
            HandlerPoint::After => {
                self.parent.pop();
                self.ctx.pop();
            }
        }
        Ok(())
    }

    fn interface_declaration(&mut self, arg: &InterfaceDeclaration) -> Result<(), ParolError> {
        match self.point {
            HandlerPoint::Before => {
                let path: SymbolPathNamespace = arg.identifier.as_ref().into();
                let name = arg.identifier.identifier_token.to_string();
                let token = arg.identifier.identifier_token.token;
                if let Some(x) = self.insert_node(&path, &name, &token) {
                    self.parent.push(x)
                }
                self.ctx.push(Context::Interface);

                for child in &self.file_scope_import.clone() {
                    if let (Some(parent), Some(child)) = (self.parent.last(), child) {
                        self.insert_edge(*parent, *child, *self.ctx.last().unwrap());
                    }
                }
            }
            HandlerPoint::After => {
                self.parent.pop();
                self.ctx.pop();
            }
        }
        Ok(())
    }

    fn package_declaration(&mut self, arg: &PackageDeclaration) -> Result<(), ParolError> {
        match self.point {
            HandlerPoint::Before => {
                let path: SymbolPathNamespace = arg.identifier.as_ref().into();
                let name = arg.identifier.identifier_token.to_string();
                let token = arg.identifier.identifier_token.token;
                if let Some(x) = self.insert_node(&path, &name, &token) {
                    self.parent.push(x)
                }
                self.ctx.push(Context::Package);

                for child in &self.file_scope_import.clone() {
                    if let (Some(parent), Some(child)) = (self.parent.last(), child) {
                        self.insert_edge(*parent, *child, *self.ctx.last().unwrap());
                    }
                }
            }
            HandlerPoint::After => {
                self.parent.pop();
                self.ctx.pop();
            }
        }
        Ok(())
    }

    fn veryl(&mut self, arg: &Veryl) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            for x in &arg.veryl_list {
                let items: Vec<DescriptionItem> = x.description_group.as_ref().into();
                for item in items {
                    if let DescriptionItem::ImportDeclaration(x) = item {
                        let x = &x.import_declaration.scoped_identifier;

                        let path: SymbolPathNamespace = x.as_ref().into();
                        let name = to_string(x);
                        let token = x.identifier.identifier_token.token;
                        let child = self.insert_node(&path, &name, &token);
                        self.file_scope_import.push(child);
                    }
                }
            }
        }
        Ok(())
    }
}

fn to_string(sid: &ScopedIdentifier) -> String {
    let mut rv: String = "".into();

    let f = |id: &Identifier, scope: bool| -> String {
        let mut s: String = (if scope { "::" } else { "" }).into();
        s.push_str(&id.identifier_token.to_string());
        s
    };
    rv.push_str(&f(&sid.identifier, false));

    for sidl in sid.scoped_identifier_list.iter() {
        let id = sidl.identifier.as_ref();
        rv.push_str(&f(id, true));
    }

    rv
}
