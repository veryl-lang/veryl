use crate::analyzer_error::AnalyzerError;
use crate::symbol::{is_dependency_symbol, Symbol, SymbolId, SymbolKind};
use crate::symbol_table;
use crate::type_dag::{self, Context, DagError};
use veryl_parser::resource_table;

#[derive(Default)]
pub struct CreateTypeDag<'a> {
    pub errors: Vec<AnalyzerError>,
    text: &'a str,
    context: Vec<Context>,
    parent: Vec<(u32, SymbolId)>,
}

impl<'a> CreateTypeDag<'a> {
    pub fn new(text: &'a str) -> Self {
        Self {
            text,
            ..Default::default()
        }
    }

    pub fn create_type_dag(&mut self, symbols: &[Symbol]) {
        for symbol in symbols.iter().filter(|x| {
            matches!(
                x.kind,
                SymbolKind::Module(_) | SymbolKind::Interface(_) | SymbolKind::Package(_)
            )
        }) {
            self.add_type_dag(symbol);
        }
    }

    fn add_type_dag(&mut self, symbol: &Symbol) {
        // Avoid stack overflow
        if self.parent.iter().filter(|(_, x)| *x == symbol.id).count() >= 2 {
            return;
        }

        let name = symbol.token.to_string();
        if let Some(x) = self.insert_node(symbol.id, &name) {
            self.add_parent_type_dag(symbol);
            if let Some((parent, _)) = self.parent.last().cloned() {
                self.insert_owned(parent, x);
                self.insert_edge(x, parent, *self.context.last().unwrap());
            }

            let context = match symbol.kind {
                SymbolKind::Module(_) => Context::Module,
                SymbolKind::Interface(_) => Context::Interface,
                SymbolKind::Modport(_) => Context::Modport,
                SymbolKind::Package(_) => Context::Package,
                SymbolKind::Enum(_) => Context::Enum,
                SymbolKind::Parameter(_) => Context::Const,
                SymbolKind::TypeDef(_) => Context::TypeDef,
                SymbolKind::Struct(_) => Context::Struct,
                SymbolKind::Union(_) => Context::Union,
                SymbolKind::Function(_) => Context::Function,
                _ => {
                    println!("token: {} kind: {}", symbol.token, symbol.kind);
                    unreachable!()
                }
            };
            self.context.push(context);
            self.parent.push((x, symbol.id));

            for dependency in &symbol.dependencies {
                let dependency = symbol_table::get(*dependency).unwrap();
                self.add_type_dag(&dependency);
            }

            self.context.pop();
            self.parent.pop();
        }
    }

    fn add_parent_type_dag(&mut self, symbol: &Symbol) {
        if let Some(ref parent) = symbol.get_parent() {
            if is_dependency_symbol(parent) {
                if !self.parent.iter().any(|(_, x)| *x == parent.id) {
                    self.add_type_dag(parent);
                }
            } else {
                self.add_parent_type_dag(parent);
            }
        }
    }

    fn insert_node(&mut self, symbol_id: SymbolId, name: &str) -> Option<u32> {
        match type_dag::insert_node(symbol_id, name) {
            Ok(x) => Some(x),
            Err(error) => {
                self.add_analyzer_error(error);
                None
            }
        }
    }

    fn insert_edge(&mut self, start: u32, end: u32, edge: Context) {
        // Reversing this order to make traversal work
        if let Err(error) = type_dag::insert_edge(start, end, edge) {
            self.add_analyzer_error(error);
        }
    }

    fn insert_owned(&mut self, parent: u32, child: u32) {
        // If there is already edge to owned type, remove it.
        // Argument order should be the same as insert_edge.
        if type_dag::exist_edge(child, parent) {
            type_dag::remove_edge(child, parent);
        }
    }

    fn add_analyzer_error(&mut self, error: DagError) {
        if let DagError::Cyclic(start, end) = error {
            let s = match resource_table::get_str_value(start.token.text) {
                Some(s) => s,
                _ => "<unknown StrId>".into(),
            };
            let e = match resource_table::get_str_value(end.token.text) {
                Some(s) => s,
                _ => "<unknown StrId>".into(),
            };
            self.errors.push(AnalyzerError::cyclic_type_dependency(
                self.text,
                &s,
                &e,
                &end.token.into(),
            ));
        }
    }
}
