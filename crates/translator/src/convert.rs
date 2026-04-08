mod expr;
mod types;
mod util;

use crate::writer::Writer;
use sv_parser::{NodeEvent, RefNode, SyntaxTree, unwrap_node};

/// Visitor signal for `walk_skip`. `Skip` causes the matched node's entire
/// subtree to be skipped (so a handler can claim a node and the walker won't
/// re-visit its descendants); `Continue` proceeds normally.
pub enum Walk {
    Continue,
    Skip,
}

/// Walk a node's event stream in pre-order. The visitor is called once per
/// `Enter` event and may signal `Skip` to elide the entered node's subtree.
/// Used to dispatch on outer constructs (modules, generates, statements, ...)
/// while avoiding double-processing of nested structures.
pub fn walk_skip<'a, F>(node: RefNode<'a>, mut visit: F)
where
    F: FnMut(&RefNode<'a>) -> Walk,
{
    let mut iter = node.into_iter().event();
    let mut depth: i32 = 0;
    while let Some(ev) = iter.next() {
        match ev {
            NodeEvent::Enter(n) => {
                depth += 1;
                if matches!(visit(&n), Walk::Skip) {
                    let target = depth - 1;
                    while depth > target {
                        match iter.next() {
                            Some(NodeEvent::Enter(_)) => depth += 1,
                            Some(NodeEvent::Leave(_)) => depth -= 1,
                            None => return,
                        }
                    }
                }
            }
            NodeEvent::Leave(_) => depth -= 1,
        }
    }
}

#[derive(Debug, Clone)]
pub struct UnsupportedReport {
    pub kind: String,
    pub line: usize,
}

pub struct Converter<'a> {
    tree: &'a SyntaxTree,
    src: &'a str,
    w: Writer,
    pub reports: Vec<UnsupportedReport>,
    /// Reset signal name within the current always_ff block, if any.
    current_reset: Option<String>,
}

impl<'a> Converter<'a> {
    pub fn new(tree: &'a SyntaxTree, src: &'a str, newline: &'static str) -> Self {
        Self {
            tree,
            src,
            w: Writer::new(newline),
            reports: Vec::new(),
            current_reset: None,
        }
    }

    /// Collect outermost descendants of `node` matching `pred`. The matched
    /// node's subtree is skipped, so nested matches are not returned (only
    /// the outermost ones in pre-order).
    fn collect_direct<P>(&self, node: &RefNode<'a>, mut pred: P) -> Vec<RefNode<'a>>
    where
        P: FnMut(&RefNode) -> bool,
    {
        let mut out: Vec<RefNode<'a>> = Vec::new();
        walk_skip(node.clone(), |n| {
            if pred(n) {
                out.push(n.clone());
                Walk::Skip
            } else {
                Walk::Continue
            }
        });
        out
    }

    /// Wrapper around `types::sv_type_to_veryl` that supplies the source text.
    fn sv_type(&self, node: &RefNode) -> String {
        types::sv_type_to_veryl(node, self.src)
    }

    pub fn run(mut self) -> (String, Vec<UnsupportedReport>) {
        // Take the top-level RefNode from the tree and walk it. The Iter for
        // `&SyntaxTree` yields the root RefNode as its first element.
        if let Some(root) = self.tree.into_iter().next() {
            walk_skip(root, |n| match n {
                RefNode::ModuleDeclarationAnsi(_) => {
                    self.module_ansi(n);
                    Walk::Skip
                }
                RefNode::ModuleDeclarationNonansi(_) => {
                    let line = self.node_line(n);
                    let src = self.node_text(n).to_string();
                    self.report("non-ANSI module", line);
                    self.w.unsupported("non-ANSI module", line, &src);
                    self.w.newline();
                    Walk::Skip
                }
                RefNode::PackageDeclaration(_) => {
                    self.emit_package(n);
                    Walk::Skip
                }
                RefNode::InterfaceDeclarationAnsi(_) => {
                    self.emit_interface(n);
                    Walk::Skip
                }
                RefNode::TypeDeclaration(_) => {
                    self.emit_typedef(n);
                    Walk::Skip
                }
                _ => Walk::Continue,
            });
        }
        (self.w.into_string(), self.reports)
    }

    fn report(&mut self, kind: &str, line: usize) {
        self.reports.push(UnsupportedReport {
            kind: kind.to_string(),
            line,
        });
    }

    fn node_text(&self, node: &RefNode) -> &'a str {
        util::node_text(node, self.src)
    }

    fn node_line(&self, node: &RefNode) -> usize {
        util::node_line(node)
    }

    fn module_ansi(&mut self, node: &RefNode<'a>) {
        let name = unwrap_node!(node.clone(), ModuleIdentifier)
            .map(|n| self.node_text(&n).trim().to_string())
            .unwrap_or_else(|| "Unknown".to_string());

        self.w.str("module ");
        self.w.str(&name);

        if let Some(pp) = unwrap_node!(node.clone(), ParameterPortList) {
            self.w.space();
            self.emit_param_port_list(&pp);
        }

        if let Some(ports) = unwrap_node!(node.clone(), ListOfPortDeclarations) {
            self.w.space();
            self.emit_ansi_ports(&ports);
        }

        self.w.space();
        self.w.str("{");
        self.w.newline();
        self.w.indent();

        self.emit_module_items(node);

        self.w.dedent();
        self.w.str("}");
        self.w.newline();
        self.w.newline();
    }

    fn emit_param_port_list(&mut self, node: &RefNode<'a>) {
        self.w.str("#(");
        self.w.newline();
        self.w.indent();

        // Walk ParameterDeclarationParam / LocalParameterDeclarationParam.
        // We use a flat scan since these may sit a few wrapper layers deep
        // inside ParameterPortList.
        let mut decls: Vec<RefNode<'a>> = Vec::new();
        for n in node.clone().into_iter() {
            if matches!(
                n,
                RefNode::ParameterDeclarationParam(_) | RefNode::LocalParameterDeclarationParam(_)
            ) {
                decls.push(n);
            }
        }

        for decl in &decls {
            // Determine type once per declaration. Default to u32 when implicit.
            let ty = self.sv_type(decl);
            let ty = if ty.is_empty() || ty == "logic" {
                "u32".to_string()
            } else {
                ty
            };
            for n in decl.clone().into_iter() {
                if let RefNode::ParamAssignment(_) = n {
                    let ident = unwrap_node!(n.clone(), ParameterIdentifier)
                        .map(|i| self.node_text(&i).trim().to_string())
                        .unwrap_or_default();
                    let value = unwrap_node!(n.clone(), ConstantParamExpression)
                        .map(|i| self.node_text(&i).trim().to_string())
                        .unwrap_or_else(|| "0".to_string());
                    self.w.str("param ");
                    self.w.str(&ident);
                    self.w.str(": ");
                    self.w.str(&ty);
                    self.w.str(" = ");
                    self.w.str(&value);
                    self.w.str(",");
                    self.w.newline();
                }
            }
        }

        self.w.dedent();
        self.w.str(")");
    }

    fn emit_ansi_ports(&mut self, node: &RefNode) {
        self.w.str("(");
        self.w.newline();
        self.w.indent();

        for n in node.clone().into_iter() {
            if let RefNode::AnsiPortDeclaration(_) = n {
                let ident = unwrap_node!(n.clone(), PortIdentifier)
                    .map(|i| self.node_text(&i).trim().to_string())
                    .unwrap_or_default();

                // Interface port? Detect via InterfacePortHeader subnode.
                if let Some(iph) = unwrap_node!(n.clone(), InterfacePortHeader) {
                    let intf = unwrap_node!(iph.clone(), InterfaceIdentifier)
                        .map(|i| self.node_text(&i).trim().to_string())
                        .unwrap_or_else(|| "intf".to_string());
                    let mp = unwrap_node!(iph.clone(), ModportIdentifier)
                        .map(|i| self.node_text(&i).trim().to_string());
                    self.w.str(&ident);
                    self.w.str(": ");
                    if let Some(mp) = mp {
                        self.w.str("modport ");
                        self.w.str(&intf);
                        self.w.str("::");
                        self.w.str(&mp);
                    } else {
                        // No modport — use interface instance.
                        self.w.str("interface ");
                        self.w.str(&intf);
                    }
                    self.w.str(",");
                    self.w.newline();
                    continue;
                }

                let dir = unwrap_node!(n.clone(), PortDirection)
                    .map(|i| self.node_text(&i).trim().to_string())
                    .unwrap_or_else(|| "input".to_string());
                let dir_v = match dir.as_str() {
                    "input" => "input",
                    "output" => "output",
                    "inout" => "inout",
                    _ => "input",
                };
                let ty = self.sv_type(&n);
                let ty = if ty.is_empty() {
                    "logic".to_string()
                } else {
                    ty
                };
                self.w.str(&ident);
                self.w.str(": ");
                self.w.str(dir_v);
                self.w.str(" ");
                self.w.str(&ty);
                self.w.str(",");
                self.w.newline();
            }
        }

        self.w.dedent();
        self.w.str(")");
    }

    fn emit_module_items(&mut self, module_node: &RefNode<'a>) {
        // Walk in pre-order; skip the module header (already rendered by
        // module_ansi) and dispatch each recognised item to its emitter,
        // skipping the matched subtree to avoid double-processing.
        walk_skip(module_node.clone(), |n| {
            if matches!(
                n,
                RefNode::ParameterPortList(_) | RefNode::ListOfPortDeclarations(_)
            ) {
                return Walk::Skip;
            }
            match n {
                RefNode::ContinuousAssign(_) => self.emit_continuous_assign(n),
                RefNode::AlwaysConstruct(_) => self.emit_always(n),
                RefNode::ModuleInstantiation(_) => self.emit_instantiation(n),
                RefNode::NetDeclaration(_) => self.emit_net_decl(n),
                RefNode::DataDeclaration(_) => self.emit_data_decl(n),
                RefNode::FunctionDeclaration(_) => self.emit_function(n),
                RefNode::TaskDeclaration(_) => self.emit_task(n),
                RefNode::LoopGenerateConstruct(_) => self.emit_loop_generate(n),
                RefNode::IfGenerateConstruct(_) => self.emit_if_generate(n),
                RefNode::ParameterDeclaration(_) | RefNode::LocalParameterDeclaration(_) => {
                    self.emit_standalone_param(n)
                }
                _ => return Walk::Continue,
            }
            Walk::Skip
        });
    }

    fn emit_continuous_assign(&mut self, node: &RefNode) {
        for n in node.clone().into_iter() {
            if let RefNode::NetAssignment(_) = n {
                let text = expr::expr_text_to_veryl(self.node_text(&n).trim());
                self.w.str("assign ");
                self.w.str(&text);
                self.w.str(";");
                self.w.newline();
            }
        }
    }

    fn emit_always(&mut self, node: &RefNode<'a>) {
        let kw = unwrap_node!(node.clone(), AlwaysKeyword)
            .map(|i| self.node_text(&i).trim().to_string())
            .unwrap_or_else(|| "always".to_string());

        let line = self.node_line(node);

        match kw.as_str() {
            "always_comb" => {
                self.w.str("always_comb {");
                self.w.newline();
                self.w.indent();
                self.emit_always_body(node);
                self.w.dedent();
                self.w.str("}");
                self.w.newline();
            }
            "always_ff" => {
                let (clk, rst) = self.extract_clock_reset(node);
                self.w.str("always_ff (");
                self.w.str(&clk);
                if let Some(r) = &rst {
                    self.w.str(", ");
                    self.w.str(r);
                }
                self.w.str(") {");
                self.w.newline();
                self.w.indent();
                self.current_reset = rst;
                self.emit_always_body(node);
                self.current_reset = None;
                self.w.dedent();
                self.w.str("}");
                self.w.newline();
            }
            other => {
                let kind = format!("{other} block");
                self.report(&kind, line);
                let src = self.node_text(node).to_string();
                self.w.unsupported(&kind, line, &src);
            }
        }
        self.w.newline();
    }

    fn extract_clock_reset(&self, node: &RefNode) -> (String, Option<String>) {
        let mut idents: Vec<String> = Vec::new();
        if let Some(ec) = unwrap_node!(node.clone(), EventControl) {
            for n in ec.into_iter() {
                if let RefNode::SimpleIdentifier(_) = n {
                    let txt = self.node_text(&n).trim().to_string();
                    if !txt.is_empty() && !idents.contains(&txt) {
                        idents.push(txt);
                    }
                }
            }
        }
        let clk = idents.first().cloned().unwrap_or_else(|| "clk".to_string());
        let rst = idents.get(1).cloned();
        (clk, rst)
    }

    fn emit_always_body(&mut self, node: &RefNode<'a>) {
        if let Some(s) = unwrap_node!(node.clone(), StatementOrNull) {
            self.emit_statement(&s);
        }
    }

    /// Dispatch a Statement / StatementOrNull to a specialised emitter based
    /// on its first significant inner kind.
    fn emit_statement(&mut self, node: &RefNode<'a>) {
        let mut found: Option<RefNode<'a>> = None;
        for n in node.clone().into_iter() {
            match n {
                RefNode::SeqBlock(_)
                | RefNode::ConditionalStatement(_)
                | RefNode::CaseStatement(_)
                | RefNode::BlockingAssignment(_)
                | RefNode::NonblockingAssignment(_)
                | RefNode::LoopStatement(_)
                | RefNode::JumpStatement(_)
                | RefNode::SubroutineCallStatement(_) => {
                    found = Some(n);
                    break;
                }
                _ => {}
            }
        }
        match found {
            Some(n) => match &n {
                RefNode::SeqBlock(_) => self.emit_seq_block(&n),
                RefNode::ConditionalStatement(_) => self.emit_if(&n),
                RefNode::CaseStatement(_) => self.emit_case(&n),
                RefNode::BlockingAssignment(_) | RefNode::NonblockingAssignment(_) => {
                    self.emit_assign_like(&n)
                }
                RefNode::LoopStatement(_) => self.emit_loop(&n),
                RefNode::JumpStatement(_) => self.emit_jump(&n),
                RefNode::SubroutineCallStatement(_) => self.emit_subroutine_call_stmt(&n),
                _ => self.emit_unsupported_stmt(node, "statement"),
            },
            None => self.emit_unsupported_stmt(node, "statement"),
        }
    }

    fn emit_unsupported_stmt(&mut self, node: &RefNode, kind: &str) {
        let line = self.node_line(node);
        let src = self.node_text(node).to_string();
        self.report(kind, line);
        self.w.unsupported(kind, line, &src);
    }

    fn emit_seq_block(&mut self, node: &RefNode<'a>) {
        // Veryl always_ff/always_comb/if/else already provide their own braces
        // for the body, so we expand SV `begin ... end` inline rather than
        // adding another `{ ... }` layer.
        let bodies = self.collect_direct(node, |n| matches!(n, RefNode::StatementOrNull(_)));
        for s in &bodies {
            self.emit_statement(s);
        }
    }

    /// Common LHS=RHS extractor for blocking and non-blocking assignments.
    /// Skips the LHS subtree once seen so a nested Expression inside the LHS
    /// index isn't mistaken for the RHS.
    fn emit_assign_like(&mut self, node: &RefNode<'a>) {
        let mut lhs = String::new();
        let mut rhs = String::new();
        let mut seen_lhs = false;
        walk_skip(node.clone(), |n| {
            if !seen_lhs && matches!(n, RefNode::VariableLvalue(_)) {
                lhs = self.node_text(n).trim().to_string();
                seen_lhs = true;
                Walk::Skip
            } else if seen_lhs && rhs.is_empty() && matches!(n, RefNode::Expression(_)) {
                rhs = expr::expr_text_to_veryl(self.node_text(n).trim());
                Walk::Skip
            } else {
                Walk::Continue
            }
        });
        self.w.str(&lhs);
        self.w.str(" = ");
        self.w.str(&rhs);
        self.w.str(";");
        self.w.newline();
    }

    fn emit_if(&mut self, node: &RefNode<'a>) {
        // Walk depth-2 children, collecting CondPredicates and StatementOrNulls
        // in order. For `if (a) X else if (b) Y else Z`, we get conds=[a,b]
        // and bodies=[X,Y,Z].
        let mut conds: Vec<String> = Vec::new();
        let mut bodies: Vec<RefNode<'a>> = Vec::new();
        walk_skip(node.clone(), |n| match n {
            RefNode::CondPredicate(_) => {
                conds.push(self.node_text(n).trim().to_string());
                Walk::Skip
            }
            RefNode::StatementOrNull(_) => {
                bodies.push(n.clone());
                Walk::Skip
            }
            _ => Walk::Continue,
        });

        // if_reset detection: only if we're inside an always_ff with a reset,
        // the first cond directly references that reset, and there are no
        // else-if branches.
        if conds.len() == 1
            && bodies.len() == 2
            && let Some(rst) = self.current_reset.clone()
        {
            let c = &conds[0];
            let is_reset = c == &rst
                || c == &format!("!{rst}")
                || c == &format!("~{rst}")
                || c == &format!("({})", rst)
                || c == &format!("(!{})", rst)
                || c == &format!("(~{})", rst);
            if is_reset {
                self.w.str("if_reset {");
                self.w.newline();
                self.w.indent();
                self.emit_statement(&bodies[0]);
                self.w.dedent();
                self.w.str("} else {");
                self.w.newline();
                self.w.indent();
                self.emit_statement(&bodies[1]);
                self.w.dedent();
                self.w.str("}");
                self.w.newline();
                return;
            }
        }

        // Generic if / else if / else chain.
        for (i, c) in conds.iter().enumerate() {
            if i == 0 {
                self.w.str("if ");
            } else {
                self.w.str(" else if ");
            }
            self.w.str(c);
            self.w.str(" {");
            self.w.newline();
            self.w.indent();
            if i < bodies.len() {
                self.emit_statement(&bodies[i]);
            }
            self.w.dedent();
            self.w.str("}");
        }
        if bodies.len() > conds.len() {
            self.w.str(" else {");
            self.w.newline();
            self.w.indent();
            self.emit_statement(&bodies[conds.len()]);
            self.w.dedent();
            self.w.str("}");
        }
        self.w.newline();
    }

    fn emit_case(&mut self, node: &RefNode<'a>) {
        // Case expression: first CaseExpression in the subtree is the outer.
        let case_expr = unwrap_node!(node.clone(), CaseExpression)
            .map(|n| self.node_text(&n).trim().to_string())
            .unwrap_or_default();

        self.w.str("case ");
        self.w.str(&case_expr);
        self.w.str(" {");
        self.w.newline();
        self.w.indent();

        // Collect outermost CaseItems (skip-on-match avoids descending into
        // case bodies which may contain nested case statements).
        let items = self.collect_direct(node, |n| matches!(n, RefNode::CaseItem(_)));

        for it in &items {
            let is_default = unwrap_node!(it.clone(), CaseItemDefault).is_some();
            let stmt = unwrap_node!(it.clone(), StatementOrNull);
            if is_default {
                self.w.str("default: ");
            } else {
                let mut exprs: Vec<String> = Vec::new();
                walk_skip(it.clone(), |n| {
                    if matches!(n, RefNode::CaseItemExpression(_)) {
                        exprs.push(self.node_text(n).trim().to_string());
                        Walk::Skip
                    } else {
                        Walk::Continue
                    }
                });
                self.w.str(&exprs.join(", "));
                self.w.str(": ");
            }
            if let Some(s) = stmt {
                // For single-statement case bodies we want them inline.
                self.emit_statement(&s);
            } else {
                self.w.newline();
            }
        }

        self.w.dedent();
        self.w.str("}");
        self.w.newline();
    }

    fn emit_instantiation(&mut self, node: &RefNode) {
        let mod_name = unwrap_node!(node.clone(), ModuleIdentifier)
            .map(|i| self.node_text(&i).trim().to_string())
            .unwrap_or_default();
        let inst_name = unwrap_node!(node.clone(), InstanceIdentifier)
            .map(|i| self.node_text(&i).trim().to_string())
            .unwrap_or_default();

        let mut ports: Vec<(String, String)> = Vec::new();
        for n in node.clone().into_iter() {
            if let RefNode::NamedPortConnection(_) = n {
                let pid = unwrap_node!(n.clone(), PortIdentifier)
                    .map(|i| self.node_text(&i).trim().to_string())
                    .unwrap_or_default();
                let expr = unwrap_node!(n.clone(), Expression)
                    .map(|i| self.node_text(&i).trim().to_string())
                    .unwrap_or_default();
                if !pid.is_empty() {
                    ports.push((pid, expr));
                }
            }
        }

        self.w.str("inst ");
        self.w.str(&inst_name);
        self.w.str(": ");
        self.w.str(&mod_name);
        self.w.space();
        self.w.str("(");
        self.w.newline();
        self.w.indent();
        for (p, e) in &ports {
            self.w.str(p);
            if !e.is_empty() && e != p {
                self.w.str(": ");
                self.w.str(e);
            }
            self.w.str(",");
            self.w.newline();
        }
        self.w.dedent();
        self.w.str(");");
        self.w.newline();
    }

    fn decl_width(&self, node: &RefNode) -> Option<String> {
        let dim = unwrap_node!(node.clone(), PackedDimension)
            .map(|i| self.node_text(&i).trim().to_string())?;
        types::packed_dim_to_width(&dim)
    }

    fn emit_net_decl(&mut self, node: &RefNode) {
        let width = self.decl_width(node);
        for n in node.clone().into_iter() {
            if let RefNode::NetIdentifier(_) = n {
                let name = self.node_text(&n).trim().to_string();
                self.w.str("var ");
                self.w.str(&name);
                self.w.str(": logic");
                if let Some(w) = &width {
                    self.w.str("<");
                    self.w.str(w);
                    self.w.str(">");
                }
                self.w.str(";");
                self.w.newline();
            }
        }
    }

    fn emit_data_decl(&mut self, node: &RefNode<'a>) {
        if unwrap_node!(node.clone(), TypeDeclaration).is_some() {
            self.emit_typedef(node);
            return;
        }
        let ty = self.sv_type(node);
        let ty = if ty.is_empty() {
            "logic".to_string()
        } else {
            ty
        };
        for n in node.clone().into_iter() {
            if let RefNode::VariableIdentifier(_) = n {
                let name = self.node_text(&n).trim().to_string();
                self.w.str("var ");
                self.w.str(&name);
                self.w.str(": ");
                self.w.str(&ty);
                self.w.str(";");
                self.w.newline();
            }
        }
    }

    fn emit_standalone_param(&mut self, node: &RefNode<'a>) {
        // Handles `parameter T X = V;` / `localparam T X = V;` outside the
        // module parameter port list (e.g. inside a package or as a module
        // item). Renders as `const X: T = V;` (Veryl uses `const` for module
        // and package level constants).
        let ty_node = unwrap_node!(node.clone(), DataTypeOrImplicit);
        let ty = ty_node.map(|n| self.sv_type(&n)).unwrap_or_default();
        let ty = if ty.is_empty() || ty == "logic" {
            "u32".to_string()
        } else {
            ty
        };
        for n in node.clone().into_iter() {
            if let RefNode::ParamAssignment(_) = n {
                let ident = unwrap_node!(n.clone(), ParameterIdentifier)
                    .map(|i| self.node_text(&i).trim().to_string())
                    .unwrap_or_default();
                let value = unwrap_node!(n.clone(), ConstantParamExpression)
                    .map(|i| self.node_text(&i).trim().to_string())
                    .unwrap_or_else(|| "0".to_string());
                self.w.str("const ");
                self.w.str(&ident);
                self.w.str(": ");
                self.w.str(&ty);
                self.w.str(" = ");
                self.w.str(&value);
                self.w.str(";");
                self.w.newline();
            }
        }
    }

    fn emit_typedef(&mut self, node: &RefNode<'a>) {
        let line = self.node_line(node);
        let name = unwrap_node!(node.clone(), TypeIdentifier)
            .map(|i| self.node_text(&i).trim().to_string())
            .unwrap_or_else(|| "T".to_string());

        // Try enum / struct first; fall back to plain type alias.
        if unwrap_node!(node.clone(), DataTypeEnum).is_some() {
            self.w.str("enum ");
            self.w.str(&name);
            if let Some(eb) = unwrap_node!(node.clone(), EnumBaseType) {
                let bt = self.sv_type(&eb);
                if !bt.is_empty() {
                    self.w.str(": ");
                    self.w.str(&bt);
                }
            }
            self.w.str(" {");
            self.w.newline();
            self.w.indent();
            for n in node.clone().into_iter() {
                if let RefNode::EnumNameDeclaration(_) = n {
                    let txt = self.node_text(&n).trim().to_string();
                    self.w.str(&txt);
                    self.w.str(",");
                    self.w.newline();
                }
            }
            self.w.dedent();
            self.w.str("}");
            self.w.newline();
            self.w.newline();
            return;
        }

        if unwrap_node!(node.clone(), DataTypeStructUnion).is_some() {
            self.w.str("struct ");
            self.w.str(&name);
            self.w.str(" {");
            self.w.newline();
            self.w.indent();
            for n in node.clone().into_iter() {
                if let RefNode::StructUnionMember(_) = n {
                    let mty = self.sv_type(&n);
                    let mty = if mty.is_empty() {
                        "logic".to_string()
                    } else {
                        mty
                    };
                    if let Some(id) = unwrap_node!(n.clone(), VariableIdentifier) {
                        let mname = self.node_text(&id).trim().to_string();
                        self.w.str(&mname);
                        self.w.str(": ");
                        self.w.str(&mty);
                        self.w.str(",");
                        self.w.newline();
                    }
                }
            }
            self.w.dedent();
            self.w.str("}");
            self.w.newline();
            self.w.newline();
            return;
        }

        // Plain `typedef logic [N-1:0] byte_t;` → `type byte_t = logic<N>;`
        // Pass the DataType subnode rather than the whole TypeDeclaration so
        // we don't pick up the new type identifier as the source type.
        let ty_node = unwrap_node!(node.clone(), DataType);
        let ty = ty_node.map(|n| self.sv_type(&n)).unwrap_or_default();
        if !ty.is_empty() {
            self.w.str("type ");
            self.w.str(&name);
            self.w.str(" = ");
            self.w.str(&ty);
            self.w.str(";");
            self.w.newline();
        } else {
            self.report("typedef", line);
            let src = self.node_text(node).to_string();
            self.w.unsupported("typedef", line, &src);
        }
    }

    fn emit_subroutine_call_stmt(&mut self, node: &RefNode<'a>) {
        // Render `$display(...);` and friends as-is. Veryl shares most system
        // function names with SystemVerilog so a textual passthrough is fine.
        let txt = self.node_text(node).trim().trim_end_matches(';').trim();
        let rewritten = expr::expr_text_to_veryl(txt);
        self.w.str(&rewritten);
        self.w.str(";");
        self.w.newline();
    }

    fn emit_jump(&mut self, node: &RefNode<'a>) {
        // return [expr];  →  return expr;
        let line = self.node_line(node);
        let txt = self.node_text(node).trim();
        if let Some(rest) = txt.strip_prefix("return") {
            let rest = rest.trim().trim_end_matches(';').trim();
            self.w.str("return");
            if !rest.is_empty() {
                self.w.str(" ");
                self.w.str(rest);
            }
            self.w.str(";");
            self.w.newline();
        } else {
            self.report("jump", line);
            self.w.unsupported("jump", line, txt);
        }
    }

    fn emit_loop(&mut self, node: &RefNode<'a>) {
        let line = self.node_line(node);
        let src = self.node_text(node);

        let var_init = self.extract_for_init(node);
        let limit = self.extract_for_limit(node);

        if let Some((var, init_val)) = var_init
            && let Some(limit) = limit
        {
            self.w.str("for ");
            self.w.str(&var);
            self.w.str(" in ");
            self.w.str(&init_val);
            self.w.str("..");
            self.w.str(&limit);
            self.w.str(" {");
            self.w.newline();
            self.w.indent();
            if let Some(body) = unwrap_node!(node.clone(), StatementOrNull) {
                self.emit_statement(&body);
            }
            self.w.dedent();
            self.w.str("}");
            self.w.newline();
            return;
        }

        self.report("loop", line);
        self.w.unsupported("loop", line, src);
    }

    /// Extract variable name and initial value from a for-loop's initialization.
    /// Handles both `int i = 0` (ForVariableDeclaration) and `i = 0`
    /// (ListOfVariableAssignments) forms.
    fn extract_for_init(&self, node: &RefNode) -> Option<(String, String)> {
        if let Some(fvd) = unwrap_node!(node.clone(), ForVariableDeclaration) {
            let var = unwrap_node!(fvd.clone(), VariableIdentifier)
                .map(|i| self.node_text(&i).trim().to_string())?;
            let init_val = unwrap_node!(fvd.clone(), Expression)
                .map(|i| self.node_text(&i).trim().to_string())
                .unwrap_or_else(|| "0".to_string());
            return Some((var, init_val));
        }
        if let Some(init) = unwrap_node!(node.clone(), ForInitialization) {
            let var = unwrap_node!(init.clone(), VariableIdentifier)
                .map(|i| self.node_text(&i).trim().to_string())?;
            let init_val = unwrap_node!(init.clone(), Expression)
                .map(|i| self.node_text(&i).trim().to_string())
                .unwrap_or_else(|| "0".to_string());
            return Some((var, init_val));
        }
        None
    }

    /// Extract the upper-bound from a for-loop condition like `i < N` or
    /// `i <= N`. Skips ForInitialization/ForStep subtrees to isolate the
    /// condition Expression.
    fn extract_for_limit(&self, node: &RefNode) -> Option<String> {
        let mut cond_text: Option<String> = None;
        walk_skip(node.clone(), |n| match n {
            RefNode::ForInitialization(_) | RefNode::ForStep(_) => Walk::Skip,
            RefNode::Expression(_) if cond_text.is_none() => {
                cond_text = Some(self.node_text(n).trim().to_string());
                Walk::Skip
            }
            _ => Walk::Continue,
        });
        let txt = cond_text?;
        let rhs = txt.split_once('<').map(|(_, r)| r)?;
        let rhs = rhs.trim().trim_start_matches('=').trim();
        Some(rhs.to_string())
    }

    fn emit_function(&mut self, node: &RefNode<'a>) {
        let name = unwrap_node!(node.clone(), FunctionIdentifier)
            .map(|i| self.node_text(&i).trim().to_string())
            .unwrap_or_else(|| "unknown".to_string());

        // Return type: scope to the FunctionDataTypeOrImplicit subnode so we
        // don't accidentally pick up packed dimensions from port arguments.
        let ret_ty = unwrap_node!(node.clone(), FunctionDataTypeOrImplicit)
            .map(|n| self.sv_type(&n))
            .unwrap_or_default();
        let is_void = self
            .node_text(node)
            .lines()
            .next()
            .map(|l| l.contains("void"))
            .unwrap_or(false);

        self.w.str("function ");
        self.w.str(&name);
        self.w.str(" (");
        self.w.newline();
        self.w.indent();
        self.emit_tf_ports(node);
        self.w.dedent();
        self.w.str(")");
        if !is_void && !ret_ty.is_empty() && ret_ty != "logic" {
            self.w.str(" -> ");
            self.w.str(&ret_ty);
        }
        self.w.str(" {");
        self.w.newline();
        self.w.indent();

        let stmts = self.collect_direct(node, |x| {
            matches!(
                x,
                RefNode::FunctionStatementOrNull(_) | RefNode::StatementOrNull(_)
            )
        });
        for s in &stmts {
            self.emit_statement(s);
        }
        // Fallback: flat scan if direct scan returned empty.
        if stmts.is_empty() {
            for c in node.clone().into_iter() {
                if matches!(
                    c,
                    RefNode::StatementOrNull(_) | RefNode::FunctionStatementOrNull(_)
                ) {
                    self.emit_statement(&c);
                }
            }
        }

        self.w.dedent();
        self.w.str("}");
        self.w.newline();
        self.w.newline();
    }

    fn emit_task(&mut self, node: &RefNode<'a>) {
        let name = unwrap_node!(node.clone(), TaskIdentifier)
            .map(|i| self.node_text(&i).trim().to_string())
            .unwrap_or_else(|| "unknown".to_string());

        self.w.str("function ");
        self.w.str(&name);
        self.w.str(" (");
        self.w.newline();
        self.w.indent();
        self.emit_tf_ports(node);
        self.w.dedent();
        self.w.str(") {");
        self.w.newline();
        self.w.indent();

        for c in node.clone().into_iter() {
            if matches!(c, RefNode::StatementOrNull(_)) {
                self.emit_statement(&c);
            }
        }

        self.w.dedent();
        self.w.str("}");
        self.w.newline();
        self.w.newline();
    }

    fn emit_tf_ports(&mut self, node: &RefNode<'a>) {
        // Emit each TfPortItem (function/task port) as Veryl `name: dir type,`.
        for n in node.clone().into_iter() {
            if let RefNode::TfPortItem(_) = n {
                let dir = unwrap_node!(n.clone(), TfPortDirection)
                    .map(|i| self.node_text(&i).trim().to_string())
                    .unwrap_or_else(|| "input".to_string());
                let ident = unwrap_node!(n.clone(), PortIdentifier)
                    .map(|i| self.node_text(&i).trim().to_string())
                    .unwrap_or_default();
                if ident.is_empty() {
                    continue;
                }
                let ty = self.sv_type(&n);
                let ty = if ty.is_empty() {
                    "logic".to_string()
                } else {
                    ty
                };
                let dir_v = match dir.as_str() {
                    "output" => "output",
                    "inout" => "inout",
                    _ => "input",
                };
                self.w.str(&ident);
                self.w.str(": ");
                self.w.str(dir_v);
                self.w.str(" ");
                self.w.str(&ty);
                self.w.str(",");
                self.w.newline();
            }
        }
    }

    fn emit_if_generate(&mut self, node: &RefNode<'a>) {
        // if (cond) generate_block [else generate_block]
        let cond = unwrap_node!(node.clone(), ConstantExpression)
            .map(|i| self.node_text(&i).trim().to_string())
            .unwrap_or_default();
        // Collect direct GenerateBlock children.
        let blocks = self.collect_direct(node, |x| matches!(x, RefNode::GenerateBlock(_)));
        self.w.str("if ");
        self.w.str(&cond);
        self.w.str(" {");
        self.w.newline();
        self.w.indent();
        if let Some(b) = blocks.first() {
            self.emit_generate_block(b);
        }
        self.w.dedent();
        self.w.str("}");
        if blocks.len() >= 2 {
            self.w.str(" else {");
            self.w.newline();
            self.w.indent();
            self.emit_generate_block(&blocks[1]);
            self.w.dedent();
            self.w.str("}");
        }
        self.w.newline();
        self.w.newline();
    }

    fn emit_loop_generate(&mut self, node: &RefNode<'a>) {
        // for (genvar i = INIT; i < LIMIT; i = i + STEP) generate_block
        // Try to extract: var name, init, limit. Emit `for i in INIT..LIMIT { }`.
        let var = unwrap_node!(node.clone(), GenvarIdentifier)
            .map(|i| self.node_text(&i).trim().to_string())
            .unwrap_or_else(|| "i".to_string());
        let init = unwrap_node!(node.clone(), ConstantExpression)
            .map(|i| self.node_text(&i).trim().to_string())
            .unwrap_or_else(|| "0".to_string());
        // limit: walk for GenvarExpression — the second relational expression.
        let limit = self
            .extract_loop_limit(node)
            .unwrap_or_else(|| "N".to_string());

        self.w.str("for ");
        self.w.str(&var);
        self.w.str(" in ");
        self.w.str(&init);
        self.w.str("..");
        self.w.str(&limit);
        self.w.str(" {");
        self.w.newline();
        self.w.indent();
        if let Some(b) = self
            .collect_direct(node, |x| matches!(x, RefNode::GenerateBlock(_)))
            .first()
        {
            self.emit_generate_block(b);
        }
        self.w.dedent();
        self.w.str("}");
        self.w.newline();
        self.w.newline();
    }

    fn extract_loop_limit(&self, node: &RefNode) -> Option<String> {
        // Look at the GenvarExpression or condition expression — extract the
        // RHS of `i < N` style comparisons.
        let expr = unwrap_node!(node.clone(), GenvarExpression)?;
        let txt = self.node_text(&expr).trim().to_string();
        if let Some(rhs) = txt.split_once('<').map(|(_, r)| r) {
            let rhs = rhs.trim().trim_start_matches('=').trim();
            return Some(rhs.to_string());
        }
        None
    }

    fn emit_generate_block(&mut self, node: &RefNode<'a>) {
        // Delegate to the same pre-order dispatcher used at module level. It
        // walks events and skips matched subtrees, so nested generates are
        // handled correctly.
        self.emit_module_items(node);
    }

    fn emit_package(&mut self, node: &RefNode<'a>) {
        let name = unwrap_node!(node.clone(), PackageIdentifier)
            .map(|i| self.node_text(&i).trim().to_string())
            .unwrap_or_else(|| "Pkg".to_string());
        self.w.str("package ");
        self.w.str(&name);
        self.w.str(" {");
        self.w.newline();
        self.w.indent();
        // Package body: typedef / parameter / function. Reuse the module-item
        // dispatcher which handles all of these (and ignores items that don't
        // belong inside a package).
        self.emit_module_items(node);
        self.w.dedent();
        self.w.str("}");
        self.w.newline();
        self.w.newline();
    }

    fn emit_interface(&mut self, node: &RefNode<'a>) {
        let name = unwrap_node!(node.clone(), InterfaceIdentifier)
            .map(|i| self.node_text(&i).trim().to_string())
            .unwrap_or_else(|| "Intf".to_string());
        self.w.str("interface ");
        self.w.str(&name);
        self.w.str(" {");
        self.w.newline();
        self.w.indent();

        // Walk interface body: var/wire decls, modports, and functions.
        walk_skip(node.clone(), |n| match n {
            RefNode::NetDeclaration(_) => {
                self.emit_net_decl(n);
                Walk::Skip
            }
            RefNode::DataDeclaration(_) => {
                self.emit_data_decl(n);
                Walk::Skip
            }
            RefNode::ModportDeclaration(_) => {
                self.emit_modport_decl(n);
                Walk::Skip
            }
            RefNode::FunctionDeclaration(_) => {
                self.emit_function(n);
                Walk::Skip
            }
            _ => Walk::Continue,
        });

        self.w.dedent();
        self.w.str("}");
        self.w.newline();
        self.w.newline();
    }

    fn emit_modport_decl(&mut self, node: &RefNode<'a>) {
        // A ModportDeclaration may contain multiple ModportItems
        // (e.g. `modport mp1 (...), mp2 (...);`). Emit each as its own block.
        for n in node.clone().into_iter() {
            if let RefNode::ModportItem(_) = n {
                self.emit_modport_item(&n);
            }
        }
    }

    fn emit_modport_item(&mut self, node: &RefNode<'a>) {
        let name = unwrap_node!(node.clone(), ModportIdentifier)
            .map(|i| self.node_text(&i).trim().to_string())
            .unwrap_or_default();
        self.w.str("modport ");
        self.w.str(&name);
        self.w.str(" {");
        self.w.newline();
        self.w.indent();

        // Walk for ModportSimplePortsDeclaration: each carries one direction
        // applied to a list of port identifiers.
        for n in node.clone().into_iter() {
            if let RefNode::ModportSimplePortsDeclaration(_) = n {
                let dir = unwrap_node!(n.clone(), PortDirection)
                    .map(|i| self.node_text(&i).trim().to_string())
                    .unwrap_or_else(|| "input".to_string());
                let dir_v = match dir.as_str() {
                    "output" => "output",
                    "inout" => "inout",
                    _ => "input",
                };
                for inner in n.clone().into_iter() {
                    if let RefNode::ModportSimplePortOrdered(_) = inner {
                        let id = self.node_text(&inner).trim().to_string();
                        if !id.is_empty() {
                            self.w.str(&id);
                            self.w.str(": ");
                            self.w.str(dir_v);
                            self.w.str(",");
                            self.w.newline();
                        }
                    }
                }
            }
        }

        self.w.dedent();
        self.w.str("}");
        self.w.newline();
        self.w.newline();
    }
}
