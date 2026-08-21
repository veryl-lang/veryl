use veryl_analyzer::attribute::{AllowItem, Attribute};
use veryl_analyzer::{attribute_table, symbol_table};
use veryl_parser::veryl_grammar_trait::*;
use veryl_parser::veryl_walker::VerylWalker;

/// SystemVerilog forbids a variable written by an `always_ff` from being
/// written by any other process, which is exactly what these opt-ins allow, so
/// the block has to be emitted as a plain `always`.
pub fn drives_non_portable_variable(arg: &AlwaysFfDeclaration) -> bool {
    if !attribute_table::has_non_portable_allow() {
        return false;
    }

    let mut scan = Scan::default();
    scan.statement_block(&arg.statement_block);
    scan.found
}

#[derive(Default)]
struct Scan {
    found: bool,
    in_write_position: bool,
}

impl VerylWalker for Scan {
    fn expression_identifier(&mut self, arg: &ExpressionIdentifier) {
        if !self.in_write_position || self.found {
            return;
        }

        if let Ok(symbol) = symbol_table::resolve(arg) {
            let token = &symbol.found.token;
            self.found =
                attribute_table::contains(token, Attribute::Allow(AllowItem::MultipleAssign))
                    || attribute_table::contains(token, Attribute::Allow(AllowItem::InitialAssign));
        }
    }

    fn identifier_statement(&mut self, arg: &IdentifierStatement) {
        self.in_write_position = true;
        match &*arg.identifier_statement_group {
            IdentifierStatementGroup::Assignment(_) => {
                self.expression_identifier(&arg.expression_identifier);
            }
            // Argument directions are not resolved here, so every argument is
            // taken as a possible output.
            IdentifierStatementGroup::FunctionCall(x) => {
                self.function_call(&x.function_call);
            }
        }
        self.in_write_position = false;
    }

    fn concatenation_assignment(&mut self, arg: &ConcatenationAssignment) {
        self.in_write_position = true;
        self.assign_concatenation_list(&arg.assign_concatenation_list);
        self.in_write_position = false;
    }
}
