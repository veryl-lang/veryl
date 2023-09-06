use crate::{
    analyzer_error::AnalyzerError,
    symbol::Symbol,
    symbol_table::{self, SymbolPathNamespace},
    type_dag::{self},
};
use veryl_parser::veryl_walker::{Handler, HandlerPoint};
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
    struct_or_union: Option<StructOrUnion>,
    parent: Vec<Symbol>,
    point: HandlerPoint,
    in_type_def: bool,
    in_enum_declaration: bool,
    in_struct_union_declaration: bool,
}

#[derive(Clone)]
enum StructOrUnion {
    InStruct,
    InUnion,
}

impl<'a> CreateTypeDag<'a> {
    pub fn new(text: &'a str) -> Self {
        Self {
            text,
            in_type_def: false,
            in_enum_declaration: false,
            ..Default::default()
        }
    }

    fn insert_edge(&mut self, s: &Symbol, e: &Symbol) {
        let start = resource_table::get_str_value(s.token.text).unwrap();
        let end = resource_table::get_str_value(e.token.text).unwrap();
        match type_dag::insert_edge(s, e) {
            Ok(_) => {}
            Err(er) => match er {
                type_dag::DagError::Cyclic(_, e) => {
                    let token: VerylToken = VerylToken {
                        token: e.token,
                        comments: vec![],
                    };
                    self.errors.push(AnalyzerError::cyclic_type_dependency(
                        self.text, &start, &end, &token,
                    ));
                }
            },
        }
    }
}

impl<'a> Handler for CreateTypeDag<'a> {
    fn set_point(&mut self, p: HandlerPoint) {
        self.point = p;
    }
}

fn resolve_to_symbol<T: Into<SymbolPathNamespace>>(path: T) -> Option<Symbol> {
    if let Ok(rr) = symbol_table::resolve(path) {
        rr.found
    } else {
        None
    }
}

impl<'a> VerylGrammarTrait for CreateTypeDag<'a> {
    fn struct_union_declaration(&mut self, arg: &StructUnionDeclaration) -> Result<(), ParolError> {
        match self.point {
            HandlerPoint::Before => {
                // Unused for now, but will be useful in the future
                // to do this struct vs union chec
                match &*arg.struct_union {
                    StructUnion::Struct(_) => self.struct_or_union = Some(StructOrUnion::InStruct),
                    StructUnion::Union(_) => self.struct_or_union = Some(StructOrUnion::InUnion),
                }
                self.parent
                    .push(resolve_to_symbol(arg.identifier.as_ref()).unwrap());
                self.in_struct_union_declaration = true;
            }
            HandlerPoint::After => {
                self.parent.pop();
                self.in_struct_union_declaration = false;
            }
        }
        Ok(())
    }

    fn type_def_declaration(&mut self, arg: &TypeDefDeclaration) -> Result<(), ParolError> {
        match self.point {
            HandlerPoint::Before => {
                self.parent
                    .push(resolve_to_symbol(arg.identifier.as_ref()).unwrap());
                self.in_type_def = true;
            }
            HandlerPoint::After => {
                self.parent.pop();
                self.in_type_def = false;
            }
        }
        Ok(())
    }

    fn scoped_identifier(&mut self, arg: &ScopedIdentifier) -> Result<(), ParolError> {
        if self.in_type_def | self.in_enum_declaration | self.in_struct_union_declaration {
            if let HandlerPoint::Before = self.point {
                let child = resolve_to_symbol(arg);
                if let Some(child) = child {
                    let parent = self.parent.last().unwrap().clone();
                    self.insert_edge(&parent, &child);
                } else {
                    println!("Couldn't resolve scoped identifier");
                }
            }
        }
        Ok(())
    }

    fn enum_declaration(&mut self, arg: &EnumDeclaration) -> Result<(), ParolError> {
        match self.point {
            HandlerPoint::Before => {
                self.parent
                    .push(resolve_to_symbol(arg.identifier.as_ref()).unwrap());
                self.in_enum_declaration = true;
            }
            HandlerPoint::After => {
                self.parent.pop();
                self.in_enum_declaration = false;
            }
        }
        Ok(())
    }
}
