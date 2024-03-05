use crate::keyword::KEYWORDS;
use async_channel::{Receiver, Sender};
use dashmap::DashMap;
use futures::executor::block_on;
use ropey::Rope;
use std::collections::VecDeque;
use std::path::Path;
use tower_lsp::lsp_types::*;
use tower_lsp::Client;
use veryl_analyzer::symbol_table::{ResolveSymbol, SymbolPath};
use veryl_analyzer::{namespace_table, symbol_table, Analyzer, AnalyzerError};
use veryl_formatter::Formatter;
use veryl_metadata::{Metadata, PathPair};
use veryl_parser::veryl_token::Token;
use veryl_parser::veryl_walker::VerylWalker;
use veryl_parser::{resource_table, Finder, Parser, ParserError};

pub enum MsgToServer {
    DidOpen {
        url: Url,
        text: String,
        version: i32,
    },
    DidChange {
        url: Url,
        text: String,
        version: i32,
    },
    Completion {
        url: Url,
        line: usize,
        column: usize,
        context: Option<CompletionContext>,
    },
    GotoDefinition {
        url: Url,
        line: usize,
        column: usize,
    },
    Symbol {
        query: String,
    },
    Hover {
        url: Url,
        line: usize,
        column: usize,
    },
    References {
        url: Url,
        line: usize,
        column: usize,
    },
    SemanticTokens {
        url: Url,
    },
    Formatting {
        url: Url,
    },
}

pub enum MsgFromServer {
    Completion(Option<CompletionResponse>),
    GotoDefinition(Option<Location>),
    Symbol(Vec<SymbolInformation>),
    Hover(Option<Hover>),
    References(Vec<Location>),
    SemanticTokens(Option<SemanticTokensResult>),
    Formatting(Option<Vec<TextEdit>>),
}

pub struct BackgroundTask {
    metadata: Metadata,
    paths: Vec<PathPair>,
    total: usize,
    progress: bool,
}

pub struct Server {
    client: Client,
    rcv: Receiver<MsgToServer>,
    snd: Sender<MsgFromServer>,
    document_map: DashMap<String, Rope>,
    parser_map: DashMap<String, Parser>,
    metadata_map: DashMap<String, Metadata>,
    cache_dir: String,
    lsp_token: i32,
    background_tasks: VecDeque<BackgroundTask>,
}

impl Server {
    pub fn new(client: Client, rcv: Receiver<MsgToServer>, snd: Sender<MsgFromServer>) -> Self {
        Server {
            client,
            rcv,
            snd,
            document_map: DashMap::new(),
            parser_map: DashMap::new(),
            metadata_map: DashMap::new(),
            cache_dir: Metadata::cache_dir().to_string_lossy().to_string(),
            lsp_token: 0,
            background_tasks: VecDeque::new(),
        }
    }

    pub fn serve(&mut self) {
        loop {
            if let Ok(msg) = self.rcv.recv_blocking() {
                match msg {
                    MsgToServer::DidOpen { url, text, version } => {
                        self.did_open(&url, &text, version)
                    }
                    MsgToServer::DidChange { url, text, version } => {
                        self.did_change(&url, &text, version)
                    }
                    MsgToServer::Completion {
                        url,
                        line,
                        column,
                        context,
                    } => self.completion(&url, line, column, &context),
                    MsgToServer::GotoDefinition { url, line, column } => {
                        self.goto_definition(&url, line, column)
                    }
                    MsgToServer::Symbol { query } => self.symbol(&query),
                    MsgToServer::Hover { url, line, column } => self.hover(&url, line, column),
                    MsgToServer::References { url, line, column } => {
                        self.references(&url, line, column)
                    }
                    MsgToServer::SemanticTokens { url } => self.semantic_tokens(&url),
                    MsgToServer::Formatting { url } => self.formatting(&url),
                }
            }

            while self.rcv.is_empty() && !self.background_tasks.is_empty() {
                if let Some(mut task) = self.background_tasks.pop_front() {
                    if !task.progress {
                        self.progress_start("background analyze");
                        task.progress = true;
                    }
                    if let Some(path) = task.paths.pop() {
                        self.background_analyze(&path, &task.metadata);
                        let pcnt = (task.total - task.paths.len()) * 100 / task.total;
                        self.progress_report(
                            &format!("{}", path.src.file_name().unwrap().to_string_lossy()),
                            pcnt as u32,
                        );
                    }
                    if task.paths.is_empty() {
                        self.progress_done("background analyze done");
                    } else {
                        self.background_tasks.push_front(task);
                    }
                }
            }
        }
    }
}

impl Server {
    fn did_open(&mut self, url: &Url, text: &str, version: i32) {
        if let Some(mut metadata) = self.get_metadata(url) {
            self.on_change(&metadata.project.name, url, text, version);

            if !url.as_str().contains(&self.cache_dir) {
                if let Ok(paths) = metadata.paths::<&str>(&[]) {
                    let total = paths.len();
                    let task = BackgroundTask {
                        metadata,
                        paths,
                        total,
                        progress: false,
                    };
                    self.background_tasks.push_back(task);
                }
            }
        } else {
            self.on_change("", url, text, version);
        }
    }

    fn did_change(&mut self, url: &Url, text: &str, version: i32) {
        if let Some(metadata) = self.get_metadata(url) {
            self.on_change(&metadata.project.name, url, text, version);
        } else {
            self.on_change("", url, text, version);
        }
    }

    fn completion(
        &mut self,
        url: &Url,
        line: usize,
        column: usize,
        context: &Option<CompletionContext>,
    ) {
        let ret = if let Some(context) = context {
            match context.trigger_kind {
                CompletionTriggerKind::TRIGGER_CHARACTER => completion_operator(
                    line,
                    column,
                    context.trigger_character.as_ref().unwrap().as_str(),
                ),
                CompletionTriggerKind::INVOKED => {
                    let mut items = if let Some(metadata) = self.get_metadata(url) {
                        completion_toplevel_entity(&metadata, line, column)
                    } else {
                        vec![]
                    };
                    items.append(&mut completion_keyword(line, column));
                    Some(CompletionResponse::Array(items))
                }
                _ => None,
            }
        } else {
            None
        };

        self.snd
            .send_blocking(MsgFromServer::Completion(ret))
            .unwrap();
    }

    fn goto_definition(&mut self, url: &Url, line: usize, column: usize) {
        let path = url.as_str();

        if let Some(parser) = self.parser_map.get(path) {
            let mut finder = Finder::new();
            finder.line = line as u32;
            finder.column = column as u32;
            finder.veryl(&parser.veryl);

            if let Some(token) = finder.token {
                if let Some(namespace) = namespace_table::get(token.id) {
                    let path = if finder.token_group.is_empty() {
                        SymbolPath::new(&[token.text])
                    } else {
                        SymbolPath::from(finder.token_group.as_slice())
                    };
                    if let Ok(symbol) = symbol_table::get(&path, &namespace) {
                        if let ResolveSymbol::Symbol(symbol) = &symbol.found {
                            let location = to_location(&symbol.token);
                            self.snd
                                .send_blocking(MsgFromServer::GotoDefinition(Some(location)))
                                .unwrap();
                            return;
                        }
                    }
                }
            }
        }

        self.snd
            .send_blocking(MsgFromServer::GotoDefinition(None))
            .unwrap();
    }

    fn symbol(&mut self, query: &str) {
        let mut ret = Vec::new();
        for symbol in symbol_table::get_all() {
            let name = symbol.token.text.to_string();
            if name.contains(query) {
                let kind = match symbol.kind {
                    veryl_analyzer::symbol::SymbolKind::Port(_) => SymbolKind::VARIABLE,
                    veryl_analyzer::symbol::SymbolKind::Variable(_) => SymbolKind::VARIABLE,
                    veryl_analyzer::symbol::SymbolKind::Module(_) => SymbolKind::MODULE,
                    veryl_analyzer::symbol::SymbolKind::Interface(_) => SymbolKind::INTERFACE,
                    veryl_analyzer::symbol::SymbolKind::Function(_) => SymbolKind::FUNCTION,
                    veryl_analyzer::symbol::SymbolKind::Parameter(_) => SymbolKind::CONSTANT,
                    veryl_analyzer::symbol::SymbolKind::Instance(_) => SymbolKind::OBJECT,
                    veryl_analyzer::symbol::SymbolKind::Block => SymbolKind::NAMESPACE,
                    veryl_analyzer::symbol::SymbolKind::Package => SymbolKind::PACKAGE,
                    veryl_analyzer::symbol::SymbolKind::Struct => SymbolKind::STRUCT,
                    veryl_analyzer::symbol::SymbolKind::StructMember(_) => SymbolKind::VARIABLE,
                    veryl_analyzer::symbol::SymbolKind::Union => SymbolKind::STRUCT,
                    veryl_analyzer::symbol::SymbolKind::UnionMember(_) => SymbolKind::VARIABLE,
                    veryl_analyzer::symbol::SymbolKind::Enum(_) => SymbolKind::ENUM,
                    veryl_analyzer::symbol::SymbolKind::EnumMember(_) => SymbolKind::ENUM_MEMBER,
                    veryl_analyzer::symbol::SymbolKind::Modport(_) => SymbolKind::INTERFACE,
                    veryl_analyzer::symbol::SymbolKind::Genvar => SymbolKind::VARIABLE,
                    veryl_analyzer::symbol::SymbolKind::TypeDef(_) => SymbolKind::TYPE_PARAMETER,
                    veryl_analyzer::symbol::SymbolKind::ModportMember => SymbolKind::VARIABLE,
                    veryl_analyzer::symbol::SymbolKind::SystemVerilog => SymbolKind::NAMESPACE,
                    veryl_analyzer::symbol::SymbolKind::Namespace => SymbolKind::NAMESPACE,
                    veryl_analyzer::symbol::SymbolKind::SystemFunction => SymbolKind::FUNCTION,
                };
                let location = to_location(&symbol.token);
                #[allow(deprecated)]
                let symbol_info = SymbolInformation {
                    name,
                    kind,
                    tags: None,
                    deprecated: None,
                    location,
                    container_name: None,
                };
                ret.push(symbol_info);
            }
        }
        self.snd.send_blocking(MsgFromServer::Symbol(ret)).unwrap();
    }

    fn hover(&mut self, url: &Url, line: usize, column: usize) {
        let path = url.as_str();

        if let Some(parser) = self.parser_map.get(path) {
            let mut finder = Finder::new();
            finder.line = line as u32;
            finder.column = column as u32;
            finder.veryl(&parser.veryl);
            if let Some(token) = finder.token {
                if let Some(namespace) = namespace_table::get(token.id) {
                    let path = if finder.token_group.is_empty() {
                        SymbolPath::new(&[token.text])
                    } else {
                        SymbolPath::from(finder.token_group.as_slice())
                    };
                    if let Ok(symbol) = symbol_table::get(&path, &namespace) {
                        if let ResolveSymbol::Symbol(symbol) = &symbol.found {
                            let text = symbol.kind.to_string();
                            let hover = Hover {
                                contents: HoverContents::Scalar(MarkedString::String(text)),
                                range: None,
                            };
                            self.snd
                                .send_blocking(MsgFromServer::Hover(Some(hover)))
                                .unwrap();
                            return;
                        }
                    }
                }
            }
        }
        self.snd.send_blocking(MsgFromServer::Hover(None)).unwrap();
    }

    fn references(&mut self, url: &Url, line: usize, column: usize) {
        let path = url.as_str();

        let mut ret = Vec::new();
        if let Some(parser) = self.parser_map.get(path) {
            let mut finder = Finder::new();
            finder.line = line as u32;
            finder.column = column as u32;
            finder.veryl(&parser.veryl);
            if let Some(token) = finder.token {
                if let Some(namespace) = namespace_table::get(token.id) {
                    let path = if finder.token_group.is_empty() {
                        SymbolPath::new(&[token.text])
                    } else {
                        SymbolPath::from(finder.token_group.as_slice())
                    };
                    if let Ok(symbol) = symbol_table::get(&path, &namespace) {
                        if let ResolveSymbol::Symbol(symbol) = &symbol.found {
                            for reference in &symbol.references {
                                let location = to_location(reference);
                                ret.push(location);
                            }
                        }
                    }
                }
            }
        }
        self.snd
            .send_blocking(MsgFromServer::References(ret))
            .unwrap();
    }

    fn semantic_tokens(&mut self, url: &Url) {
        let path = url.as_str();

        let ret = if let Some(path) = resource_table::get_path_id(Path::new(path).to_path_buf()) {
            let mut tokens = Vec::new();
            for symbol in &symbol_table::get_all() {
                if symbol.token.source == path {
                    if let veryl_analyzer::symbol::SymbolKind::Port(_) = symbol.kind {
                        let token_type = semantic_legend::PROPERTY;
                        tokens.push((symbol.token, token_type));
                        for reference in &symbol.references {
                            if reference.source == path {
                                tokens.push((*reference, token_type));
                            }
                        }
                    }
                }
            }

            tokens.sort_by(|a, b| {
                a.0.line
                    .partial_cmp(&b.0.line)
                    .unwrap()
                    .then(a.0.column.partial_cmp(&b.0.column).unwrap())
            });

            let mut line = 0;
            let mut column = 0;
            let mut data = Vec::new();
            for (token, token_type) in tokens {
                let token_line = token.line - 1;
                let token_column = token.column - 1;

                let delta_line = token_line - line;
                let delta_start = if delta_line == 0 {
                    token_column - column
                } else {
                    token_column
                };

                let semantic_token = SemanticToken {
                    delta_line,
                    delta_start,
                    length: token.length,
                    token_type,
                    token_modifiers_bitset: 0,
                };
                data.push(semantic_token);

                line = token_line;
                column = token_column;
            }

            let tokens = SemanticTokens {
                result_id: None,
                data,
            };
            Some(SemanticTokensResult::Tokens(tokens))
        } else {
            None
        };

        self.snd
            .send_blocking(MsgFromServer::SemanticTokens(ret))
            .unwrap();
    }

    fn formatting(&mut self, url: &Url) {
        let path = url.as_str();

        if let Some(metadata) = self.get_metadata(url) {
            if let Some(rope) = self.document_map.get(path) {
                let line = rope.len_lines() as u32;
                if let Some(parser) = self.parser_map.get(path) {
                    let mut formatter = Formatter::new(&metadata);
                    formatter.format(&parser.veryl);

                    let text_edit = TextEdit {
                        range: Range::new(Position::new(0, 0), Position::new(line, u32::MAX)),
                        new_text: formatter.as_str().to_string(),
                    };

                    self.snd
                        .send_blocking(MsgFromServer::Formatting(Some(vec![text_edit])))
                        .unwrap();
                    return;
                }
            }
        }

        self.snd
            .send_blocking(MsgFromServer::Formatting(None))
            .unwrap();
    }
}

impl Server {
    fn progress_start(&mut self, msg: &str) {
        self.lsp_token += 1;
        let token = NumberOrString::Number(self.lsp_token);
        let begin = WorkDoneProgressBegin {
            title: msg.to_string(),
            cancellable: Some(false),
            message: None,
            percentage: Some(100),
        };

        block_on(self.client.send_request::<request::WorkDoneProgressCreate>(
            WorkDoneProgressCreateParams {
                token: token.clone(),
            },
        ))
        .unwrap();

        block_on(
            self.client
                .send_notification::<notification::Progress>(ProgressParams {
                    token,
                    value: ProgressParamsValue::WorkDone(WorkDoneProgress::Begin(begin)),
                }),
        );
    }

    fn progress_report(&self, msg: &str, pcnt: u32) {
        let token = NumberOrString::Number(self.lsp_token);
        let report = WorkDoneProgressReport {
            cancellable: Some(false),
            message: Some(msg.to_string()),
            percentage: Some(pcnt),
        };

        block_on(
            self.client
                .send_notification::<notification::Progress>(ProgressParams {
                    token,
                    value: ProgressParamsValue::WorkDone(WorkDoneProgress::Report(report)),
                }),
        );
    }

    fn progress_done(&self, msg: &str) {
        let token = NumberOrString::Number(self.lsp_token);
        let end = WorkDoneProgressEnd {
            message: Some(msg.to_string()),
        };

        block_on(
            self.client
                .send_notification::<notification::Progress>(ProgressParams {
                    token,
                    value: ProgressParamsValue::WorkDone(WorkDoneProgress::End(end)),
                }),
        );
    }

    fn background_analyze(&self, path: &PathPair, metadata: &Metadata) {
        if let Ok(text) = std::fs::read_to_string(&path.src) {
            if let Ok(uri) = Url::from_file_path(&path.src) {
                let uri = uri.as_str();
                if self.document_map.contains_key(uri) {
                    return;
                }
                if let Ok(x) = Parser::parse(&text, &uri) {
                    if let Some(uri) = resource_table::get_path_id(Path::new(uri).to_path_buf()) {
                        symbol_table::drop(uri);
                        namespace_table::drop(uri);
                    }
                    let analyzer = Analyzer::new(metadata);
                    let _ = analyzer.analyze_pass1(&path.prj, &text, uri, &x.veryl);

                    block_on(
                        self.client
                            .log_message(MessageType::INFO, format!("background_analyze: {uri}")),
                    );
                }
            }
        }
    }

    fn get_metadata(&mut self, url: &Url) -> Option<Metadata> {
        let path = url.as_str();
        if let Some(metadata) = self.metadata_map.get(path) {
            return Some(metadata.to_owned());
        } else if let Ok(metadata_path) = Metadata::search_from(path) {
            if let Ok(metadata) = Metadata::load(metadata_path) {
                self.metadata_map.insert(path.to_string(), metadata.clone());
                return Some(metadata);
            }
        }
        None
    }

    fn on_change(&mut self, prj: &str, url: &Url, text: &str, version: i32) {
        let path = url.as_str();
        let rope = Rope::from_str(text);

        if path.contains(&self.cache_dir) {
            return;
        }

        if let Some(metadata) = self.get_metadata(url) {
            let diag = match Parser::parse(text, &path) {
                Ok(x) => {
                    if let Some(path) = resource_table::get_path_id(Path::new(&path).to_path_buf())
                    {
                        symbol_table::drop(path);
                        namespace_table::drop(path);
                    }
                    let analyzer = Analyzer::new(&metadata);
                    let mut errors = analyzer.analyze_pass1(prj, text, path, &x.veryl);
                    errors.append(&mut analyzer.analyze_pass2(prj, text, path, &x.veryl));
                    errors.append(&mut analyzer.analyze_pass3(prj, text, path, &x.veryl));
                    let ret: Vec<_> = errors
                        .drain(0..)
                        .map(|x| {
                            let x: miette::ErrReport = x.into();
                            to_diag(x, &rope)
                        })
                        .collect();
                    self.parser_map.insert(path.to_string(), x);
                    ret
                }
                Err(x) => {
                    self.parser_map.remove(path);
                    vec![to_diag(x.into(), &rope)]
                }
            };

            block_on(
                self.client
                    .publish_diagnostics(url.clone(), diag, Some(version)),
            );
        }

        self.document_map.insert(path.to_string(), rope);
    }
}

fn to_diag(err: miette::ErrReport, rope: &Rope) -> Diagnostic {
    let miette_diag: &dyn miette::Diagnostic = err.as_ref();

    let range = if let Some(mut labels) = miette_diag.labels() {
        labels.next().map_or(Range::default(), |label| {
            let line = rope.byte_to_line(label.offset());
            let pos = label.offset() - rope.line_to_byte(line);
            let line = line as u32;
            let pos = pos as u32;
            let len = label.len() as u32;
            Range::new(Position::new(line, pos), Position::new(line, pos + len))
        })
    } else {
        Range::default()
    };

    let code = miette_diag
        .code()
        .map(|d| NumberOrString::String(format!("{d}")));

    let (severity, message) = if let Some(x) = err.downcast_ref::<ParserError>() {
        let msg = match x {
            ParserError::SyntaxError(x) => {
                format!("Syntax Error: {x}")
            }
            ParserError::ParserError(x) => {
                format!("Syntax Error: {x}")
            }
            ParserError::LexerError(x) => {
                format!("Syntax Error: {x}")
            }
            ParserError::UserError(x) => {
                format!("Syntax Error: {x}")
            }
        };
        (DiagnosticSeverity::ERROR, msg)
    } else if let Some(x) = err.downcast_ref::<AnalyzerError>() {
        use miette::Diagnostic;
        let (severity, text) = match x.severity() {
            Some(miette::Severity::Error) => (DiagnosticSeverity::ERROR, "Error"),
            Some(miette::Severity::Warning) => (DiagnosticSeverity::WARNING, "Warning"),
            Some(miette::Severity::Advice) => (DiagnosticSeverity::HINT, "Hint"),
            None => (DiagnosticSeverity::ERROR, "Error"),
        };
        (severity, format!("Semantic {text}: {err}"))
    } else {
        (DiagnosticSeverity::ERROR, format!("Semantic Error: {err}"))
    };

    Diagnostic::new(
        range,
        Some(severity),
        code,
        Some(String::from("veryl-ls")),
        message,
        None,
        None,
    )
}

fn to_location(token: &Token) -> Location {
    let line = token.line - 1;
    let column = token.column - 1;
    let length = token.length;
    let uri = Url::parse(&token.source.to_string()).unwrap();
    let range = Range::new(
        Position::new(line, column),
        Position::new(line, column + length),
    );
    Location { uri, range }
}

fn completion_item_operator(
    line: usize,
    column: usize,
    label: &str,
    detail: &str,
) -> CompletionItem {
    let line = (line - 1) as u32;
    let character = (column - 1) as u32;
    let start = Position { line, character };
    let end = Position { line, character };
    let text_edit = CompletionTextEdit::Edit(TextEdit {
        range: Range { start, end },
        new_text: label.to_string(),
    });
    CompletionItem {
        label: label.to_string(),
        kind: Some(CompletionItemKind::OPERATOR),
        detail: Some(detail.to_string()),
        text_edit: Some(text_edit),
        ..Default::default()
    }
}

fn completion_operator(line: usize, column: usize, trigger: &str) -> Option<CompletionResponse> {
    let l = line;
    let c = column;
    match trigger {
        "<" => {
            let items = vec![
                completion_item_operator(l, c, "=", "less than equal"),
                completion_item_operator(l, c, ":", "less than"),
                completion_item_operator(l, c, "<<", "arithmetic left shift"),
                completion_item_operator(l, c, "<<=", "arithmetic left shift assignment"),
                completion_item_operator(l, c, "<", "logical left shift"),
                completion_item_operator(l, c, "<=", "logical left shift assignment"),
            ];
            Some(CompletionResponse::Array(items))
        }
        ">" => {
            let items = vec![
                completion_item_operator(l, c, "=", "greater than equal"),
                completion_item_operator(l, c, ":", "greater than"),
                completion_item_operator(l, c, ">>", "arithmetic right shift"),
                completion_item_operator(l, c, ">>=", "arithmetic right shift assignment"),
                completion_item_operator(l, c, ">", "logical right shift"),
                completion_item_operator(l, c, ">=", "logical right shift assignment"),
            ];
            Some(CompletionResponse::Array(items))
        }
        "=" => {
            let items = vec![
                completion_item_operator(l, c, "=", "logical equality"),
                completion_item_operator(l, c, "==", "case equality"),
                completion_item_operator(l, c, "=?", "wildcard equality"),
            ];
            Some(CompletionResponse::Array(items))
        }
        "!" => {
            let items = vec![
                completion_item_operator(l, c, "=", "logical inequality"),
                completion_item_operator(l, c, "==", "case inequality"),
                completion_item_operator(l, c, "=?", "wildcard inequality"),
            ];
            Some(CompletionResponse::Array(items))
        }
        _ => None,
    }
}

fn completion_toplevel_entity(
    metadata: &Metadata,
    line: usize,
    column: usize,
) -> Vec<CompletionItem> {
    let mut items = Vec::new();
    let line = (line - 1) as u32;
    let character = (column - 1) as u32;
    let start = Position { line, character };
    let end = Position { line, character };

    let prj = resource_table::get_str_id(&metadata.project.name).unwrap();

    for symbol in symbol_table::get_all() {
        if symbol.namespace.paths.len() == 1 {
            //&& symbol.namespace.paths[0] == prj {
            let prefix = if symbol.namespace.paths[0] == prj {
                "".to_string()
            } else {
                format!("{}::", symbol.namespace.paths[0])
            };
            let (new_text, kind) = match symbol.kind {
                veryl_analyzer::symbol::SymbolKind::Module(ref x) => {
                    let mut ports = String::new();
                    for port in &x.ports {
                        ports.push_str(&format!("{}, ", port.name));
                    }
                    let text = format!("{}{} ({});", prefix, symbol.token.text, ports);
                    (text, Some(CompletionItemKind::CLASS))
                }
                veryl_analyzer::symbol::SymbolKind::Interface(_) => {
                    let text = format!("{}{} ();", prefix, symbol.token.text);
                    (text, Some(CompletionItemKind::INTERFACE))
                }
                veryl_analyzer::symbol::SymbolKind::Package => {
                    let text = format!("{}{}::", prefix, symbol.token.text);
                    (text, Some(CompletionItemKind::MODULE))
                }
                _ => {
                    let text = format!("{}{}", prefix, symbol.token.text);
                    (text, None)
                }
            };

            let label = format!("{}{}", prefix, symbol.token.text);
            let detail = Some(format!("{}", symbol.kind));

            let text_edit = CompletionTextEdit::Edit(TextEdit {
                range: Range { start, end },
                new_text,
            });

            let item = CompletionItem {
                label,
                kind,
                detail,
                text_edit: Some(text_edit),
                ..Default::default()
            };
            items.push(item);
        }
    }

    items
}

fn completion_keyword(line: usize, column: usize) -> Vec<CompletionItem> {
    let line = (line - 1) as u32;
    let character = (column - 1) as u32;
    let start = Position { line, character };
    let end = Position { line, character };

    let mut items = Vec::new();
    for keyword in KEYWORDS {
        let new_text = keyword.to_string();

        let text_edit = CompletionTextEdit::Edit(TextEdit {
            range: Range { start, end },
            new_text,
        });

        let item = CompletionItem {
            label: keyword.to_string(),
            kind: Some(CompletionItemKind::KEYWORD),
            detail: None,
            text_edit: Some(text_edit),
            ..Default::default()
        };
        items.push(item);
    }

    items
}

pub mod semantic_legend {
    use super::*;

    pub const PROPERTY: u32 = 0;

    pub fn get_token_types() -> Vec<SemanticTokenType> {
        vec![SemanticTokenType::PROPERTY]
    }

    pub fn get_token_modifiers() -> Vec<SemanticTokenModifier> {
        vec![]
    }
}
