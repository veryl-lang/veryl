use crate::veryl_grammar_trait::*;
use crate::veryl_token::{Token, VerylToken};
use crate::veryl_walker::VerylWalker;
use std::collections::{HashMap, HashSet};
use veryl_metadata::{Format, Metadata};
use veryl_parser::resource_table::{self, StrId, TokenId};
use veryl_parser::veryl_grammar_trait::Veryl as NewVeryl;

pub struct Migrator {
    format_opt: Format,
    newline: &'static str,
    string: String,
    line: u32,
    column: u32,
    /// Identifier tokens of the `var` declarations that need the attribute.
    annotate: HashSet<TokenId>,
}

impl Default for Migrator {
    fn default() -> Self {
        Self {
            format_opt: Format::default(),
            newline: "\n",
            string: String::new(),
            line: 1,
            column: 1,
            annotate: HashSet::new(),
        }
    }
}

impl Migrator {
    pub fn new(metadata: &Metadata) -> Self {
        Self {
            format_opt: metadata.format.clone(),
            ..Default::default()
        }
    }

    pub fn migrate(&mut self, input: &Veryl, raw_input: &str) {
        self.newline = self.format_opt.newline_style.newline_str(raw_input);
        self.annotate = InitialAssignScan::collect(input);
        self.veryl(input);
    }

    pub fn as_str(&self) -> &str {
        &self.string
    }

    fn str(&mut self, x: &str) {
        self.string.push_str(x);
    }

    fn push_token(&mut self, x: &Token) {
        let newlines = x.line.saturating_sub(self.line);
        self.line = x.line;
        if newlines > 0 {
            self.column = 1;
        }
        let spaces = x.column.saturating_sub(self.column);
        self.column += spaces;

        for _ in 0..newlines {
            self.str(self.newline);
        }
        self.str(&" ".repeat(spaces as usize));

        let text = resource_table::get_str_value(x.text).unwrap();

        let newlines_in_text = text.matches('\n').count() as u32;
        self.line += newlines_in_text;
        let len = text.len() - text.rfind('\n').map(|x| x + 1).unwrap_or(0);
        if newlines_in_text > 0 {
            self.column = 1;
        } else {
            self.column += len as u32;
        }

        self.str(&text);
    }

    fn token(&mut self, x: &VerylToken) {
        self.push_token(&x.token);

        for x in &x.comments {
            self.push_token(x);
        }
    }

    /// Check whether the valid syntax tree should be migrated
    pub fn migratable(veryl: &NewVeryl) -> bool {
        use veryl_parser::veryl_grammar_trait as new;
        use veryl_parser::veryl_walker::VerylWalker as NewVerylWalker;

        #[derive(Default)]
        struct Checker {
            readmem_in_initial: bool,
            migrated: bool,
            in_initial: bool,
        }

        impl NewVerylWalker for Checker {
            fn attribute(&mut self, arg: &new::Attribute) {
                if arg.identifier.identifier_token.token.text.to_string() != "allow" {
                    return;
                }
                if let Some(opt) = &arg.attribute_opt {
                    let items: Vec<&new::AttributeItem> = opt.attribute_list.as_ref().into();
                    self.migrated |= items.iter().any(|x| match x {
                        new::AttributeItem::Identifier(x) => {
                            x.identifier.identifier_token.token.text.to_string() == "initial_assign"
                        }
                        new::AttributeItem::StringLiteral(_) => false,
                    });
                }
            }

            fn initial_declaration(&mut self, arg: &new::InitialDeclaration) {
                self.in_initial = true;
                self.statement_block(&arg.statement_block);
                self.in_initial = false;
            }

            fn identifier_statement(&mut self, arg: &new::IdentifierStatement) {
                if !self.in_initial {
                    return;
                }
                let text = arg
                    .expression_identifier
                    .identifier()
                    .token
                    .text
                    .to_string();
                self.readmem_in_initial |= text == "$readmemh" || text == "$readmemb";
            }
        }

        let mut checker = Checker::default();
        checker.veryl(veryl);

        checker.readmem_in_initial && !checker.migrated
    }
}

/// `$readmemh` / `$readmemb` assigns its second argument, which requires
/// `#[allow(initial_assign)]` on the declaration. Names are matched per module
/// or interface, so a same-named declaration elsewhere is left alone.
#[derive(Default)]
struct InitialAssignScan {
    written: HashMap<StrId, HashSet<StrId>>,
    declared: HashMap<StrId, Vec<(StrId, TokenId)>>,
    component: Option<StrId>,
    in_initial: bool,
}

impl InitialAssignScan {
    fn collect(input: &Veryl) -> HashSet<TokenId> {
        let mut scan = Self::default();
        scan.veryl(input);

        let mut ret = HashSet::new();
        for (component, declared) in &scan.declared {
            let Some(written) = scan.written.get(component) else {
                continue;
            };
            for (name, id) in declared {
                if written.contains(name) {
                    ret.insert(*id);
                }
            }
        }
        ret
    }
}

impl VerylWalker for InitialAssignScan {
    /// Testbenches are exempt from the check, so they need no attribute.
    fn skip_description_group(&mut self, arg: &DescriptionGroup) -> bool {
        arg.description_group_list.iter().any(|x| {
            x.attribute
                .identifier
                .identifier_token
                .token
                .text
                .to_string()
                == "test"
        })
    }

    fn module_declaration(&mut self, arg: &ModuleDeclaration) {
        self.component = Some(arg.identifier.identifier_token.token.text);
        for x in &arg.module_declaration_list {
            self.module_group(&x.module_group);
        }
        self.component = None;
    }

    fn interface_declaration(&mut self, arg: &InterfaceDeclaration) {
        self.component = Some(arg.identifier.identifier_token.token.text);
        for x in &arg.interface_declaration_list {
            self.interface_group(&x.interface_group);
        }
        self.component = None;
    }

    fn var_declaration(&mut self, arg: &VarDeclaration) {
        if let Some(component) = self.component {
            let token = &arg.identifier.identifier_token.token;
            self.declared
                .entry(component)
                .or_default()
                .push((token.text, token.id));
        }
    }

    fn initial_declaration(&mut self, arg: &InitialDeclaration) {
        self.in_initial = true;
        self.statement_block(&arg.statement_block);
        self.in_initial = false;
    }

    fn identifier_statement(&mut self, arg: &IdentifierStatement) {
        let Some(component) = self.component else {
            return;
        };
        if !self.in_initial {
            return;
        }

        let callee = arg
            .expression_identifier
            .scoped_identifier
            .identifier()
            .token
            .text
            .to_string();
        if callee != "$readmemh" && callee != "$readmemb" {
            return;
        }

        let IdentifierStatementGroup::FunctionCall(x) = &*arg.identifier_statement_group else {
            return;
        };
        let Some(opt) = &x.function_call.function_call_opt else {
            return;
        };

        let args: Vec<&ArgumentItem> = opt.argument_list.as_ref().into();
        let Some(memory) = args.get(1) else {
            return;
        };
        let Some(identifier) = memory.argument_expression.expression.unwrap_identifier() else {
            return;
        };
        if !identifier.expression_identifier_list0.is_empty() {
            return;
        }

        let name = identifier.scoped_identifier.identifier().token.text;
        self.written.entry(component).or_default().insert(name);
    }
}

impl VerylWalker for Migrator {
    fn veryl_token(&mut self, arg: &VerylToken) {
        self.token(arg);
    }

    fn var_declaration(&mut self, arg: &VarDeclaration) {
        if self
            .annotate
            .contains(&arg.identifier.identifier_token.token.id)
        {
            // Take the `var` keyword's position so the attribute lands at the
            // declaration's indent; `veryl fmt` runs afterwards and lays it out.
            let attribute = arg.var.var_token.replace("#[allow(initial_assign)]");
            self.push_token(&attribute.token);
            self.str(" ");
            // Rewind to the declaration's own column so the tokens after the
            // attribute keep their original spacing.
            self.column = arg.var.var_token.token.column;
        }
        self.var(&arg.var);
        self.identifier(&arg.identifier);
        if let Some(ref x) = arg.var_declaration_opt {
            self.colon(&x.colon);
            if let Some(ref y) = x.var_declaration_opt0 {
                self.clock_domain(&y.clock_domain);
            }
            self.array_type(&x.array_type);
        }
        self.semicolon(&arg.semicolon);
    }
}
