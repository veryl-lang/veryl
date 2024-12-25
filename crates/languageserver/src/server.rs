use crate::keyword::KEYWORDS;
use async_channel::{Receiver, Sender};
use dashmap::DashMap;
use futures::executor::block_on;
use ropey::Rope;
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use tower_lsp::lsp_types::*;
use tower_lsp::Client;
use veryl_analyzer::namespace::Namespace;
use veryl_analyzer::symbol::SymbolKind as VerylSymbolKind;
use veryl_analyzer::symbol::{Symbol, TypeKind};
use veryl_analyzer::symbol_path::SymbolPath;
use veryl_analyzer::{namespace_table, symbol_table, Analyzer, AnalyzerError};
use veryl_formatter::Formatter;
use veryl_metadata::Metadata;
use veryl_parser::veryl_token::Token;
use veryl_parser::veryl_walker::VerylWalker;
use veryl_parser::{resource_table, Finder, Parser, ParserError};
use veryl_path::PathSet;

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
    DidChangeConfiguration(ServerConfigItem),
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
    paths: Vec<PathSet>,
    total: usize,
    progress: bool,
}

pub enum ServerConfigItem {
    UseOperatorCompletion(bool),
}

#[derive(Default)]
pub struct ServerConfig {
    use_operator_completion: bool,
}

impl ServerConfig {
    pub fn set(&mut self, item: ServerConfigItem) {
        match item {
            ServerConfigItem::UseOperatorCompletion(x) => self.use_operator_completion = x,
        }
    }
}

pub struct Server {
    client: Client,
    rcv: Receiver<MsgToServer>,
    snd: Sender<MsgFromServer>,
    document_map: DashMap<PathBuf, Rope>,
    parser_map: DashMap<PathBuf, Parser>,
    metadata_map: DashMap<PathBuf, Metadata>,
    cache_dir: PathBuf,
    lsp_token: i32,
    background_tasks: VecDeque<BackgroundTask>,
    background_done: bool,
    config: ServerConfig,
    latest_change: Option<(Url, String, i32)>,
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
            cache_dir: veryl_path::cache_path(),
            lsp_token: 0,
            background_tasks: VecDeque::new(),
            background_done: true,
            config: ServerConfig::default(),
            latest_change: None,
        }
    }

    pub fn serve(&mut self) {
        loop {
            if let Ok(msg) = self.rcv.recv_blocking() {
                match msg {
                    MsgToServer::DidOpen { url, text, version } => {
                        self.did_open(&url, &text, version);
                        self.latest_change = Some((url, text, version));
                    }
                    MsgToServer::DidChange { url, text, version } => {
                        self.did_change(&url, &text, version);
                        self.latest_change = Some((url, text, version));
                    }
                    MsgToServer::DidChangeConfiguration(x) => self.config.set(x),
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
                        Analyzer::analyze_post_pass1();
                        self.progress_done("background analyze done");
                        if self.background_tasks.is_empty() {
                            self.background_done = true;

                            // call did_change after background_done to notify filtered errors
                            if let Some((url, text, version)) = self.latest_change.take() {
                                self.did_change(&url, &text, version);
                            }
                        }
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
            self.background_done = false;
            self.on_change(&metadata.project.name, url, text, version);

            if let Ok(path) = url.to_file_path() {
                if !path.starts_with(&self.cache_dir) {
                    if let Ok(paths) = metadata.paths::<&str>(&[], true) {
                        let total = paths.len();
                        let task = BackgroundTask {
                            metadata,
                            paths,
                            total,
                            progress: false,
                        };
                        self.background_tasks.push_back(task);
                    } else {
                        self.background_done = true;
                    }
                } else {
                    self.background_done = true;
                }
            } else {
                self.background_done = true;
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

    fn get_line(&self, url: &Url, line: usize) -> Option<String> {
        if let Ok(path) = url.to_file_path() {
            if let Some(rope) = self.document_map.get(&path) {
                if let Some(text) = rope.line(line - 1).as_str() {
                    return Some(text.to_string());
                }
            }
        }
        None
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
                CompletionTriggerKind::TRIGGER_CHARACTER => {
                    let trigger = context.trigger_character.as_ref().unwrap();
                    if trigger == "." {
                        if let Some(text) = self.get_line(url, line) {
                            let text = text.split_whitespace().last().unwrap();
                            let text = text.trim_matches(|c| !char::is_alphanumeric(c));
                            let items = completion_member(url, line, column, text);
                            Some(CompletionResponse::Array(items))
                        } else {
                            None
                        }
                    } else if self.config.use_operator_completion {
                        completion_operator(line, column, trigger)
                    } else {
                        None
                    }
                }
                CompletionTriggerKind::INVOKED => {
                    let mut items = if let Some(metadata) = self.get_metadata(url) {
                        completion_symbol(&metadata, url, line, column)
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
        if let Ok(path) = url.to_file_path() {
            if let Some(parser) = self.parser_map.get(&path) {
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
                        if let Ok(symbol) = symbol_table::resolve((&path, &namespace)) {
                            let location = to_location(&symbol.found.token);
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
                    VerylSymbolKind::Port(_) => SymbolKind::VARIABLE,
                    VerylSymbolKind::Variable(_) => SymbolKind::VARIABLE,
                    VerylSymbolKind::Module(_) => SymbolKind::MODULE,
                    VerylSymbolKind::ProtoModule(_) => SymbolKind::MODULE,
                    VerylSymbolKind::Interface(_) => SymbolKind::INTERFACE,
                    VerylSymbolKind::Function(_) => SymbolKind::FUNCTION,
                    VerylSymbolKind::Parameter(_) => SymbolKind::CONSTANT,
                    VerylSymbolKind::Instance(_) => SymbolKind::OBJECT,
                    VerylSymbolKind::Block => SymbolKind::NAMESPACE,
                    VerylSymbolKind::Package(_) => SymbolKind::PACKAGE,
                    VerylSymbolKind::Struct(_) => SymbolKind::STRUCT,
                    VerylSymbolKind::StructMember(_) => SymbolKind::VARIABLE,
                    VerylSymbolKind::Union(_) => SymbolKind::STRUCT,
                    VerylSymbolKind::UnionMember(_) => SymbolKind::VARIABLE,
                    VerylSymbolKind::Enum(_) => SymbolKind::ENUM,
                    VerylSymbolKind::EnumMember(_) => SymbolKind::ENUM_MEMBER,
                    VerylSymbolKind::EnumMemberMangled => SymbolKind::ENUM_MEMBER,
                    VerylSymbolKind::Modport(_) => SymbolKind::INTERFACE,
                    VerylSymbolKind::Genvar => SymbolKind::VARIABLE,
                    VerylSymbolKind::TypeDef(_) => SymbolKind::TYPE_PARAMETER,
                    VerylSymbolKind::ModportVariableMember(_) => SymbolKind::VARIABLE,
                    VerylSymbolKind::ModportFunctionMember(_) => SymbolKind::FUNCTION,
                    VerylSymbolKind::SystemVerilog => SymbolKind::NAMESPACE,
                    VerylSymbolKind::Namespace => SymbolKind::NAMESPACE,
                    VerylSymbolKind::SystemFunction => SymbolKind::FUNCTION,
                    VerylSymbolKind::GenericParameter(_) => SymbolKind::TYPE_PARAMETER,
                    VerylSymbolKind::GenericInstance(_) => SymbolKind::MODULE,
                    VerylSymbolKind::ClockDomain => SymbolKind::TYPE_PARAMETER,
                    VerylSymbolKind::Test(_) => SymbolKind::MODULE,
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
        if let Ok(path) = url.to_file_path() {
            if let Some(parser) = self.parser_map.get(&path) {
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
                        if let Ok(symbol) = symbol_table::resolve((&path, &namespace)) {
                            let text = symbol.found.kind.to_string();
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
        let mut ret = Vec::new();
        if let Ok(path) = url.to_file_path() {
            if let Some(parser) = self.parser_map.get(&path) {
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
                        if let Ok(symbol) = symbol_table::resolve((&path, &namespace)) {
                            for reference in &symbol.found.references {
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
        let mut ret = None;

        if let Ok(path) = url.to_file_path() {
            if let Some(path) = resource_table::get_path_id(path) {
                let mut tokens = Vec::new();
                for symbol in &symbol_table::get_all() {
                    if symbol.token.source == path {
                        if let VerylSymbolKind::Port(_) = symbol.kind {
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
                ret = Some(SemanticTokensResult::Tokens(tokens))
            }
        }

        self.snd
            .send_blocking(MsgFromServer::SemanticTokens(ret))
            .unwrap();
    }

    fn formatting(&mut self, url: &Url) {
        if let Ok(path) = url.to_file_path() {
            if let Some(metadata) = self.get_metadata(url) {
                if let Some(rope) = self.document_map.get(&path) {
                    let line = rope.len_lines() as u32;
                    if let Some(parser) = self.parser_map.get(&path) {
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

    fn background_analyze(&self, path: &PathSet, metadata: &Metadata) {
        let src = path.src.clone();
        if let Ok(text) = std::fs::read_to_string(&src) {
            if self.document_map.contains_key(&src) {
                return;
            }
            if let Ok(x) = Parser::parse(&text, &src) {
                if let Some(src) = resource_table::get_path_id(&src) {
                    symbol_table::drop(src);
                    namespace_table::drop(src);
                }
                let analyzer = Analyzer::new(metadata);
                let _ = analyzer.analyze_pass1(&path.prj, &text, &src, &x.veryl);

                block_on(self.client.log_message(
                    MessageType::INFO,
                    format!("background_analyze: {}", src.to_string_lossy()),
                ));
            }
        }
    }

    fn get_metadata(&mut self, url: &Url) -> Option<Metadata> {
        if let Ok(path) = url.to_file_path() {
            if let Some(metadata) = self.metadata_map.get(&path) {
                return Some(metadata.to_owned());
            } else if let Ok(metadata_path) = Metadata::search_from(&path) {
                if let Ok(metadata) = Metadata::load(metadata_path) {
                    self.metadata_map.insert(path, metadata.clone());
                    return Some(metadata);
                }
            }
        }
        None
    }

    fn on_change(&mut self, prj: &str, url: &Url, text: &str, version: i32) {
        if let Ok(path) = url.to_file_path() {
            let rope = Rope::from_str(text);

            if path.starts_with(&self.cache_dir) {
                return;
            }

            if let Some(metadata) = self.get_metadata(url) {
                let diag = match Parser::parse(text, &path) {
                    Ok(x) => {
                        if let Some(path) =
                            resource_table::get_path_id(Path::new(&path).to_path_buf())
                        {
                            symbol_table::drop(path);
                            namespace_table::drop(path);
                        }
                        let analyzer = Analyzer::new(&metadata);
                        let mut errors = analyzer.analyze_pass1(prj, text, &path, &x.veryl);
                        Analyzer::analyze_post_pass1();
                        errors.append(&mut analyzer.analyze_pass2(prj, text, &path, &x.veryl));
                        errors.append(&mut analyzer.analyze_pass3(prj, text, &path, &x.veryl));
                        let ret: Vec<_> = errors
                            .drain(0..)
                            .filter(|x| {
                                // Filter errors caused by unresolve error until background completion
                                if self.background_done {
                                    true
                                } else {
                                    !matches!(
                                        x,
                                        AnalyzerError::UndefinedIdentifier { .. }
                                            | AnalyzerError::UnknownMember { .. }
                                            | AnalyzerError::UnassignVariable { .. }
                                    )
                                }
                            })
                            .map(|x| {
                                let x: miette::ErrReport = x.into();
                                to_diag(x, &rope)
                            })
                            .collect();
                        self.parser_map.insert(path.clone(), x);
                        ret
                    }
                    Err(x) => {
                        self.parser_map.remove(&path);
                        vec![to_diag(x.into(), &rope)]
                    }
                };

                block_on(
                    self.client
                        .publish_diagnostics(url.clone(), diag, Some(version)),
                );
            } else {
                block_on(
                    self.client
                        .log_message(MessageType::INFO, format!("failed to load metadata: {url}")),
                );
            }

            self.document_map.insert(path.clone(), rope);
        }
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
    let uri = Url::from_file_path(token.source.to_string()).unwrap();
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

fn get_member(symbol: &Symbol) -> Vec<CompletionItem> {
    let mut items = Vec::new();
    match &symbol.kind {
        VerylSymbolKind::Modport(x) => {
            for member in &x.members {
                let symbol = symbol_table::get(*member).unwrap();
                let label = symbol.token.text.to_string();
                let kind = Some(CompletionItemKind::FIELD);
                let detail = Some(format!("{}", symbol.kind));
                let documentation = if !symbol.doc_comment.is_empty() {
                    let content = MarkupContent {
                        kind: MarkupKind::Markdown,
                        value: symbol.doc_comment.format(false),
                    };
                    Some(Documentation::MarkupContent(content))
                } else {
                    None
                };
                let insert_text = Some(label.clone());

                let item = CompletionItem {
                    label,
                    kind,
                    detail,
                    documentation,
                    insert_text,
                    ..Default::default()
                };
                items.push(item);
            }
        }
        VerylSymbolKind::Struct(x) => {
            for member in &x.members {
                let symbol = symbol_table::get(*member).unwrap();
                let label = symbol.token.text.to_string();
                let kind = Some(CompletionItemKind::FIELD);
                let detail = Some(format!("{}", symbol.kind));
                let documentation = if !symbol.doc_comment.is_empty() {
                    let content = MarkupContent {
                        kind: MarkupKind::Markdown,
                        value: symbol.doc_comment.format(false),
                    };
                    Some(Documentation::MarkupContent(content))
                } else {
                    None
                };
                let insert_text = Some(label.clone());

                let item = CompletionItem {
                    label,
                    kind,
                    detail,
                    documentation,
                    insert_text,
                    ..Default::default()
                };
                items.push(item);
            }
        }
        _ => (),
    }
    items
}

fn completion_member(url: &Url, line: usize, column: usize, text: &str) -> Vec<CompletionItem> {
    let current_namespace = current_namespace(url, line, column);
    let text = resource_table::get_str_id(text.to_string()).unwrap();
    let mut items = Vec::new();

    if let Some(namespace) = current_namespace {
        if let Ok(symbol) = symbol_table::resolve((&vec![text], &namespace)) {
            match symbol.found.kind {
                VerylSymbolKind::Port(x) => {
                    if let Some(ref x) = x.r#type {
                        if let TypeKind::UserDefined(ref x) = x.kind {
                            if let Ok(symbol) = symbol_table::resolve((x, &namespace)) {
                                items.append(&mut get_member(&symbol.found));
                            }
                        }
                    }
                }
                VerylSymbolKind::Variable(x) => {
                    if let TypeKind::UserDefined(ref x) = x.r#type.kind {
                        if let Ok(symbol) = symbol_table::resolve((x, &namespace)) {
                            items.append(&mut get_member(&symbol.found));
                        }
                    }
                }
                _ => (),
            }
        }
    }

    items
}

fn completion_symbol(
    metadata: &Metadata,
    url: &Url,
    line: usize,
    column: usize,
) -> Vec<CompletionItem> {
    let current_namespace = current_namespace(url, line, column);

    let mut items = Vec::new();

    let prj = resource_table::get_str_id(&metadata.project.name).unwrap();

    for symbol in symbol_table::get_all() {
        let top_level_item = symbol.namespace.paths.len() <= 1;
        let current_item = if let Some(ref x) = current_namespace {
            symbol.namespace.included(x)
        } else {
            false
        };

        if top_level_item || current_item {
            let prefix = if symbol.namespace.paths.is_empty()
                || symbol.namespace.paths[0] == prj
                || current_item
            {
                "".to_string()
            } else {
                format!("{}::", symbol.namespace.paths[0])
            };
            let (new_text, kind) = match symbol.kind {
                VerylSymbolKind::Module(ref x) => {
                    let mut ports = String::new();
                    for port in &x.ports {
                        ports.push_str(&format!("{}, ", port.name()));
                    }
                    let text = format!("{}{} ({});", prefix, symbol.token.text, ports);
                    (text, Some(CompletionItemKind::CLASS))
                }
                VerylSymbolKind::Interface(_) => {
                    let text = format!("{}{} ();", prefix, symbol.token.text);
                    (text, Some(CompletionItemKind::INTERFACE))
                }
                VerylSymbolKind::Package(_) => {
                    let text = format!("{}{}::", prefix, symbol.token.text);
                    (text, Some(CompletionItemKind::MODULE))
                }
                VerylSymbolKind::Port(_)
                | VerylSymbolKind::Variable(_)
                | VerylSymbolKind::Parameter(_) => {
                    let text = format!("{}{}", prefix, symbol.token.text);
                    (text, Some(CompletionItemKind::VARIABLE))
                }
                VerylSymbolKind::Function(_) | VerylSymbolKind::SystemFunction => {
                    let text = format!("{}{}", prefix, symbol.token.text);
                    (text, Some(CompletionItemKind::FUNCTION))
                }
                _ => {
                    let text = format!("{}{}", prefix, symbol.token.text);
                    (text, None)
                }
            };

            let label = format!("{}{}", prefix, symbol.token.text);
            let detail = Some(format!("{}", symbol.kind));
            let documentation = if !symbol.doc_comment.is_empty() {
                let content = MarkupContent {
                    kind: MarkupKind::Markdown,
                    value: symbol.doc_comment.format(false),
                };
                Some(Documentation::MarkupContent(content))
            } else {
                None
            };

            let item = CompletionItem {
                label,
                kind,
                detail,
                documentation,
                insert_text: Some(new_text),
                ..Default::default()
            };
            items.push(item);
        }
    }

    items
}

fn current_namespace(url: &Url, line: usize, column: usize) -> Option<Namespace> {
    let path = url.to_file_path().ok()?;
    let url = resource_table::get_path_id(path)?;

    let mut ret = None;
    let mut ret_func = None;
    for symbol in symbol_table::get_all() {
        match symbol.kind {
            VerylSymbolKind::Module(x) => {
                if x.range.include(url, line as u32, column as u32) {
                    let mut namespace = symbol.namespace;
                    namespace.push(symbol.token.text);
                    ret = Some(namespace);
                }
            }
            VerylSymbolKind::Function(x) => {
                if x.range.include(url, line as u32, column as u32) {
                    let mut namespace = symbol.namespace;
                    namespace.push(symbol.token.text);
                    ret_func = Some(namespace);
                }
            }
            VerylSymbolKind::Interface(x) => {
                if x.range.include(url, line as u32, column as u32) {
                    let mut namespace = symbol.namespace;
                    namespace.push(symbol.token.text);
                    ret = Some(namespace);
                }
            }
            VerylSymbolKind::Package(x) => {
                if x.range.include(url, line as u32, column as u32) {
                    let mut namespace = symbol.namespace;
                    namespace.push(symbol.token.text);
                    ret = Some(namespace);
                }
            }
            _ => (),
        }
    }

    ret_func.or(ret)
}

fn completion_keyword(line: usize, column: usize) -> Vec<CompletionItem> {
    let line = (line - 1) as u32;
    let character = (column - 2) as u32;
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
