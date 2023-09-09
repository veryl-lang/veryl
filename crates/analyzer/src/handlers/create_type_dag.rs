use crate::{
    analyzer_error::AnalyzerError,
    symbol_table::SymbolPathNamespace,
    type_dag::{self, Context, DagError, TypeResolveInfo},
};
use veryl_parser::{veryl_walker::{Handler, HandlerPoint}, veryl_grammar_trait::Identifier};
use veryl_parser::{
    resource_table,
    veryl_grammar_trait::{
        EnumDeclaration, ScopedIdentifier, StructUnion, StructUnionDeclaration, TypeDefDeclaration,
        VerylGrammarTrait,
    },
    veryl_token::VerylToken,
    ParolError,
};

#[derive(Default)]
pub struct CreateTypeDag<'a> {
    text: &'a str,
    pub errors: Vec<AnalyzerError>,
    parent: Option<u32>,
    point: HandlerPoint,
    ctx: Context,
}

impl<'a> CreateTypeDag<'a> {
    pub fn new(text: &'a str) -> Self {
        Self {
            text,
            ..Default::default()
        }
    }

    fn insert_node(&mut self, path: &SymbolPathNamespace, name: &str, token: &VerylToken) -> Option<u32> {
        match type_dag::insert_node(path, &name, &token) {
            Ok(n) => Some(n),
            Err(e) => {
                self.errors.push(self.to_analyzer_error(e));
                None
            },
        }
    }

    fn to_analyzer_error(&self, de: DagError) -> AnalyzerError {
match de {
            DagError::Cyclic(s, e) => {
                let token: VerylToken = VerylToken {
                    token: e.token,
                    comments: vec![],
                };
                let start = match resource_table::get_str_value(s.token.text,) {
                    Some(s) => s,
                    None => "<unknown StrId>".into(),
                };
                let end = match resource_table::get_str_value(e.token.text,) {
                    Some(s) => s,
                    None => "<unknown StrId>".into(),
                };
                AnalyzerError::cyclic_type_dependency(
                    self.text, &start, &end, &token,
                )
            }
            DagError::UnableToResolve(TypeResolveInfo
                                      { path: _, name, token, }) => {
                AnalyzerError::undefined_identifier(&name, self.text, &token)
            }
        }
    }

    fn insert_edge(&mut self, s: u32, e: u32, edge: Context) {
        // Reversing this order to make traversal work
        match type_dag::insert_edge(e, s, edge) {
            Ok(_) => {},
            Err(er) => {
                self.errors.push(self.to_analyzer_error(er));
            },
        }
    }
}

impl<'a> Handler for CreateTypeDag<'a> {
    fn set_point(&mut self, p: HandlerPoint) {
        self.point = p;
    }
}

impl<'a> VerylGrammarTrait for CreateTypeDag<'a> {
    fn veryl(&mut self, _arg: &veryl_parser::veryl_grammar_trait::Veryl) -> Result<(), ParolError> {
        if let HandlerPoint::After = self.point {
            // Evaluate DAG

        }
        Ok(())
    }

    fn struct_union_declaration(&mut self, arg: &StructUnionDeclaration) -> Result<(), ParolError> {
        match self.point {
            HandlerPoint::Before => {
                let path: SymbolPathNamespace = arg.identifier.as_ref().into();
                let name = arg.identifier.identifier_token.text();
                let token = arg.identifier.identifier_token.clone();
                self.parent  = self.insert_node(&path, &name, &token);
                // Unused for now, but will be useful in the future
                // to do this struct vs union chec
                match &*arg.struct_union {
                    StructUnion::Struct(_) => self.ctx = Context::Struct,
                    StructUnion::Union(_) => self.ctx = Context::Union,
                }
            }
            HandlerPoint::After => {
                self.parent = None;
                self.ctx = Context::Irrelevant;
            }
        }
        Ok(())
    }

    fn type_def_declaration(&mut self, arg: &TypeDefDeclaration) -> Result<(), ParolError> {
        match self.point {
            HandlerPoint::Before => {
                let path: SymbolPathNamespace = arg.identifier.as_ref().into();
                let name = arg.identifier.identifier_token.text();
                let token = arg.identifier.identifier_token.clone();
                self.parent = self.insert_node(&path, &name, &token);
                self.ctx = Context::TypeDef;
            }
            HandlerPoint::After => {
                self.parent = None;
                self.ctx = Context::TypeDef;
            }
        }
        Ok(())
    }


    fn scoped_identifier(&mut self, arg: &ScopedIdentifier) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            match self.ctx {
                Context::Irrelevant => {},
                _ => {
                    let path: SymbolPathNamespace = arg.into();
                    let name = to_string(arg);
                    let token = arg.identifier.identifier_token.clone();
                    let child = self.insert_node(&path, &name, &token);
                    match (self.parent, child) {
                        (Some(parent), Some(child)) => {
                            self.insert_edge(parent, child, self.ctx);
                        },
                        _ => {}
                    }
                }
            }
        }
        Ok(())
    }

    fn enum_declaration(&mut self, arg: &EnumDeclaration) -> Result<(), ParolError> {
        match self.point {
            HandlerPoint::Before => {
                let path: SymbolPathNamespace = arg.identifier.as_ref().into();
                let name = arg.identifier.identifier_token.text();
                let token = arg.identifier.identifier_token.clone();
                self.parent = self.insert_node(&path, &name, &token);
                self.ctx = Context::Enum;
            }
            HandlerPoint::After => {
                self.parent = None;
                self.ctx = Context::Irrelevant;
            }
        }
        Ok(())
    }
}

fn to_string(sid: &ScopedIdentifier) -> String {
    let mut rv: String = "".into();

    let f = |id: &Identifier, scope: bool| -> String {
        let mut s: String = (if scope { "::" } else { "" }).into();
        s.push_str(&id.identifier_token.text());
        s
    };
    rv.push_str(&f(&sid.identifier, false));

    for sidl in sid.scoped_identifier_list.iter() {
        let id = sidl.identifier.as_ref();
        rv.push_str(&f(id, true));
    }

    rv
}
