use crate::incremental::LsIncrementalMap;
use crate::keyword::KEYWORDS;
use async_channel::{Receiver, Sender};
use dashmap::DashMap;
use futures::executor::block_on;
use ropey::Rope;
use std::collections::VecDeque;
use std::path::PathBuf;
use tower_lsp_server::Client;
use tower_lsp_server::ls_types::ClientCapabilities;
use tower_lsp_server::ls_types::Uri as Url;
use tower_lsp_server::ls_types::*;
use veryl_analyzer::namespace::Namespace;
use veryl_analyzer::symbol::SymbolKind as VerylSymbolKind;
use veryl_analyzer::symbol::{Symbol, TbComponentKind, TypeKind};
use veryl_analyzer::symbol_path::{SymbolPath, SymbolPathNamespace};
use veryl_analyzer::{
    Analyzer, AnalyzerError, Context, attribute_table, component_manifest_table, definition_table,
    fragment_cache, scope, symbol_table, unsafe_table,
};
use veryl_formatter::Formatter;
use veryl_metadata::{ComponentManifest, Metadata};
use veryl_parser::resource_table::{self, PathId};
use veryl_parser::text_table;
use veryl_parser::veryl_token::Token;
use veryl_parser::veryl_walker::VerylWalker;
use veryl_parser::{Finder, Parser, ParserError};
use veryl_path::PathSet;

pub struct Capability {
    work_done_progress: bool,
}

impl From<ClientCapabilities> for Capability {
    fn from(value: ClientCapabilities) -> Self {
        let work_done_progress = if let Some(x) = &value.window {
            x.work_done_progress.unwrap_or(false)
        } else {
            false
        };

        Self { work_done_progress }
    }
}

pub enum MsgToServer {
    Initialize {
        capability: Capability,
    },
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
    WillRenameFile {
        old_url: Url,
        // new_uri: String, // This is not used currently
    },
    DidRenameFile {
        new_url: Url,
    },
    WillDeleteFile {
        url: Url,
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
    work_done_progress: bool,
    incremental: LsIncrementalMap,
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
            work_done_progress: true,
            incremental: LsIncrementalMap::default(),
        }
    }

    pub fn serve(&mut self) {
        loop {
            if let Ok(msg) = self.rcv.recv_blocking() {
                match msg {
                    MsgToServer::Initialize { capability } => {
                        self.work_done_progress = capability.work_done_progress;
                    }
                    MsgToServer::DidOpen { url, text, version } => {
                        self.did_open(&url, &text, version);
                        self.latest_change = Some((url, text, version));
                    }
                    MsgToServer::DidChange { url, text, version } => {
                        self.did_change(&url, &text, version);
                        self.latest_change = Some((url, text, version));
                    }
                    MsgToServer::DidChangeConfiguration(x) => self.config.set(x),
                    MsgToServer::WillRenameFile { old_url } => self.on_remove(old_url),
                    MsgToServer::DidRenameFile { new_url } => self.did_rename_files(new_url),
                    MsgToServer::WillDeleteFile { url } => self.on_remove(url),
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
                        if let Some(inc) = self.incremental.get(&task.metadata) {
                            inc.save();
                        }
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

            if let Some(path) = url.to_file_path() {
                if !path.starts_with(&self.cache_dir) {
                    if let Ok(paths) = metadata.paths::<&str>(&[], true, true) {
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

    fn did_rename_files(&mut self, new_path: Url) {
        // Do not dispatch if there's already a pending analysis
        if !self.background_done {
            return;
        }

        self.background_done = false;
        if let Some(mut metadata) = self.get_metadata(&new_path) {
            if let Ok(paths) = metadata.paths::<&str>(&[], true, true) {
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
    }

    fn get_line(&self, url: &Url, line: usize) -> Option<String> {
        if let Some(path) = url.to_file_path()
            && let Some(rope) = self.document_map.get(path.as_ref())
            && let Some(text) = rope.line(line - 1).as_str()
        {
            return Some(text.to_string());
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
                    items.append(&mut completion_keyword());
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
        if let Some(path) = url.to_file_path()
            && let Some(parser) = self.parser_map.get(path.as_ref())
        {
            let mut finder = Finder::new();
            finder.line = line as u32;
            finder.column = column as u32;
            finder.veryl(&parser.veryl);

            if let Some(token) = finder.token
                && let Some((scope, define_context)) = scope::token_scope(token.id)
            {
                let path = if finder.token_group.is_empty() {
                    SymbolPath::new(&[token.text])
                } else {
                    SymbolPath::from(finder.token_group.as_slice())
                };
                if let Ok(symbol) = symbol_table::resolve(SymbolPathNamespace::from_scope(
                    path,
                    scope,
                    define_context,
                )) {
                    let location = to_location(&symbol.found.token);
                    self.snd
                        .send_blocking(MsgFromServer::GotoDefinition(location))
                        .unwrap();
                    return;
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
                    VerylSymbolKind::AliasModule(_) => SymbolKind::MODULE,
                    VerylSymbolKind::Interface(_) => SymbolKind::INTERFACE,
                    VerylSymbolKind::AliasInterface(_) => SymbolKind::INTERFACE,
                    VerylSymbolKind::Function(_) => SymbolKind::FUNCTION,
                    VerylSymbolKind::Parameter(_) => SymbolKind::CONSTANT,
                    VerylSymbolKind::Instance(_) => SymbolKind::OBJECT,
                    VerylSymbolKind::Block => SymbolKind::NAMESPACE,
                    VerylSymbolKind::Package(_) => SymbolKind::PACKAGE,
                    VerylSymbolKind::AliasPackage(_) => SymbolKind::PACKAGE,
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
                    VerylSymbolKind::SystemFunction(_) => SymbolKind::FUNCTION,
                    VerylSymbolKind::GenericParameter(_) => SymbolKind::TYPE_PARAMETER,
                    VerylSymbolKind::GenericConst(_) => SymbolKind::TYPE_PARAMETER,
                    VerylSymbolKind::GenericInstance(_) => SymbolKind::MODULE,
                    VerylSymbolKind::ClockDomain => SymbolKind::TYPE_PARAMETER,
                    VerylSymbolKind::Test(_) => SymbolKind::MODULE,
                    VerylSymbolKind::Embed => SymbolKind::NAMESPACE,
                    VerylSymbolKind::TbComponent(_) => SymbolKind::MODULE,
                    VerylSymbolKind::ProjectProperty(_) => SymbolKind::CONSTANT,
                };
                if let Some(location) = to_location(&symbol.token) {
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
        }
        self.snd.send_blocking(MsgFromServer::Symbol(ret)).unwrap();
    }

    fn hover(&mut self, url: &Url, line: usize, column: usize) {
        if let Some(path) = url.to_file_path()
            && let Some(parser) = self.parser_map.get(path.as_ref())
        {
            let mut finder = Finder::new();
            finder.line = line as u32;
            finder.column = column as u32;
            finder.veryl(&parser.veryl);
            if let Some(token) = finder.token
                && let Some((scope, define_context)) = scope::token_scope(token.id)
            {
                let path = if finder.token_group.is_empty() {
                    SymbolPath::new(&[token.text])
                } else {
                    SymbolPath::from(finder.token_group.as_slice())
                };
                if let Ok(symbol) = symbol_table::resolve(SymbolPathNamespace::from_scope(
                    path,
                    scope,
                    define_context,
                )) {
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
        self.snd.send_blocking(MsgFromServer::Hover(None)).unwrap();
    }

    fn references(&mut self, url: &Url, line: usize, column: usize) {
        let mut ret = Vec::new();
        if let Some(path) = url.to_file_path()
            && let Some(parser) = self.parser_map.get(path.as_ref())
        {
            let mut finder = Finder::new();
            finder.line = line as u32;
            finder.column = column as u32;
            finder.veryl(&parser.veryl);
            if let Some(token) = finder.token
                && let Some((scope, define_context)) = scope::token_scope(token.id)
            {
                let path = if finder.token_group.is_empty() {
                    SymbolPath::new(&[token.text])
                } else {
                    SymbolPath::from(finder.token_group.as_slice())
                };
                if let Ok(symbol) = symbol_table::resolve(SymbolPathNamespace::from_scope(
                    path,
                    scope,
                    define_context,
                )) {
                    let refs = symbol_table::get_references(symbol.found.id).unwrap_or_default();
                    for reference in &refs {
                        if let Some(location) = to_location(reference) {
                            ret.push(location);
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

        if let Some(path) = url.to_file_path()
            && let Some(path) = resource_table::get_path_id(path.to_path_buf())
        {
            let mut tokens = Vec::new();
            for symbol in &symbol_table::get_all() {
                if symbol.token.source == path
                    && let VerylSymbolKind::Port(_) = symbol.kind
                {
                    let token_type = semantic_legend::PROPERTY;
                    tokens.push((symbol.token, token_type));
                    if let Some(refs) = symbol_table::get_references(symbol.id) {
                        for reference in &refs {
                            if reference.source == path && !is_keyword_token(*reference) {
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

        self.snd
            .send_blocking(MsgFromServer::SemanticTokens(ret))
            .unwrap();
    }

    fn formatting(&mut self, url: &Url) {
        if let Some(path) = url.to_file_path()
            && let Some(metadata) = self.get_metadata(url)
            && let Some(rope) = self.document_map.get(path.as_ref())
        {
            let line = rope.len_lines() as u32;
            if let Some(parser) = self.parser_map.get(path.as_ref()) {
                let mut formatter = Formatter::new(&metadata);
                let raw_input: String = String::from(&*rope);
                formatter.format(&parser.veryl, &raw_input);

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

        self.snd
            .send_blocking(MsgFromServer::Formatting(None))
            .unwrap();
    }
}

impl Server {
    fn progress_start(&mut self, msg: &str) {
        if !self.work_done_progress {
            return;
        }

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
        if !self.work_done_progress {
            return;
        }

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
        if !self.work_done_progress {
            return;
        }

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

    fn background_analyze(&mut self, path: &PathSet, metadata: &Metadata) {
        let src = path.src.clone();
        // Files open in the editor are analyzed from their buffer by
        // `on_change`; never cache or re-analyze them from disk here.
        if self.document_map.contains_key(&src) {
            return;
        }
        let Ok(text) = std::fs::read_to_string(&src) else {
            return;
        };

        // A cache hit restores this file's pass1 state, skipping parse.
        let restored = self
            .incremental
            .get(metadata)
            .is_some_and(|inc| inc.try_restore(path, &text));
        if restored {
            return;
        }

        // Clear stale state from a previous analysis before re-registering.
        if let Some(src_id) = resource_table::get_path_id(&src) {
            drop_tables(src_id);
        }
        // Snapshot the ID counters before parse so the file's pass1 output
        // can be captured as a fragment afterwards.
        let watermark = self
            .incremental
            .get(metadata)
            .map(|_| fragment_cache::watermark());
        if let Ok(x) = Parser::parse(&text, &src) {
            let analyzer = Analyzer::new(metadata);
            let errors = analyzer.analyze_pass1(&path.prj, &x.veryl);
            if let (Some(inc), Some(wm)) = (self.incremental.get(metadata), watermark.as_ref()) {
                inc.capture(path, &text, wm, errors.is_empty());
            }

            block_on(self.client.log_message(
                MessageType::INFO,
                format!("background_analyze: {}", src.to_string_lossy()),
            ));
        }
    }

    fn get_metadata(&mut self, url: &Url) -> Option<Metadata> {
        if let Some(path) = url.to_file_path() {
            if let Some(metadata) = self.metadata_map.get(path.as_ref()) {
                return Some(metadata.to_owned());
            } else if let Ok(metadata_path) = Metadata::search_from(path.as_ref())
                && let Ok(metadata) = Metadata::load(metadata_path)
            {
                self.metadata_map
                    .insert(path.to_path_buf(), metadata.clone());
                return Some(metadata);
            }
        }
        None
    }

    fn on_change(&mut self, prj: &str, url: &Url, text: &str, version: i32) {
        if let Some(path) = url.to_file_path() {
            let rope = Rope::from_str(text);

            if path.starts_with(&self.cache_dir) {
                return;
            }

            if let Some(metadata) = self.get_metadata(url) {
                // Drop before parse: parse re-registers the text, so
                // dropping after would erase the just-registered entry.
                if let Some(path_id) = resource_table::get_path_id(path.to_path_buf()) {
                    drop_tables(path_id);
                }
                let diag = match Parser::parse(text, &path) {
                    Ok(x) => {
                        let path_id = resource_table::get_path_id(path.to_path_buf());

                        let analyzer = Analyzer::new(&metadata);
                        let mut context = Context::default();
                        let mut ir = veryl_analyzer::ir::Ir::default();
                        let mut errors = analyzer.analyze_pass1(prj, &x.veryl);
                        errors.append(&mut Analyzer::analyze_post_pass1());
                        errors.append(&mut analyzer.analyze_pass2(
                            &x.veryl,
                            &mut context,
                            Some(&mut ir),
                        ));
                        errors.append(&mut Analyzer::analyze_post_pass2(&ir));
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
                                            | AnalyzerError::UnusedVariable { .. }
                                            | AnalyzerError::AnonymousIdentifierUsage { .. }
                                            | AnalyzerError::UnevaluableValue { .. }
                                            | AnalyzerError::MismatchType { .. }
                                            | AnalyzerError::ReferringBeforeDefinition { .. }
                                    )
                                }
                            })
                            // Filter errors caused by background sources
                            .filter(|x| x.token_source() == path_id)
                            .map(|x| {
                                let x: miette::ErrReport = x.into();
                                to_diag(x, &rope)
                            })
                            .collect();
                        self.parser_map.insert(path.to_path_buf(), x);
                        ret
                    }
                    Err(x) => {
                        self.parser_map.remove(path.as_ref());
                        vec![to_diag(x.into(), &rope)]
                    }
                };

                block_on(
                    self.client
                        .publish_diagnostics(url.clone(), diag, Some(version)),
                );
            } else {
                block_on(self.client.log_message(
                    MessageType::INFO,
                    format!("failed to load metadata: {}", url.as_str()),
                ));
            }

            self.document_map.insert(path.to_path_buf(), rope);
        }
    }

    fn on_remove(&mut self, path: Url) {
        if let Some(path) = path.to_file_path()
            && let Some(path_id) = resource_table::get_path_id(path.to_path_buf())
        {
            drop_tables(path_id);
        }
    }
}

fn to_diag(err: miette::ErrReport, rope: &Rope) -> Diagnostic {
    let miette_diag: &dyn miette::Diagnostic = err.as_ref();

    let (primary, participants) = miette_diag
        .labels()
        .map(|mut it| (it.next(), it.collect::<Vec<_>>()))
        .unwrap_or((None, Vec::new()));

    let range = primary
        .as_ref()
        .map(|label| {
            if rope.len_bytes() <= label.offset() {
                Range::default()
            } else {
                let line = rope.byte_to_line(label.offset());
                let pos = label.offset() - rope.line_to_byte(line);
                let len = label.len();
                Range::new(
                    Position::new(line as u32, pos as u32),
                    Position::new(line as u32, (pos + len) as u32),
                )
            }
        })
        .unwrap_or_default();

    // Non-primary labels become navigable related-information.
    let related_information: Option<Vec<DiagnosticRelatedInformation>> =
        miette_diag.source_code().and_then(|sc| {
            let related: Vec<_> = participants
                .iter()
                .filter_map(|label| label_to_related(label, sc))
                .collect();
            (!related.is_empty()).then_some(related)
        });

    let code = miette_diag
        .code()
        .map(|d| NumberOrString::String(format!("{d}")));

    let (severity, message) = if let Some(x) = err.downcast_ref::<ParserError>() {
        let msg = match x {
            ParserError::SyntaxError(x) => {
                use miette::Diagnostic;
                if let Some(help) = x.help()
                    && !help.to_string().is_empty()
                {
                    format!("Syntax Error: {x}\nhelp: {help}")
                } else {
                    format!("Syntax Error: {x}")
                }
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
        related_information,
        None,
    )
}

fn label_to_related(
    label: &miette::LabeledSpan,
    source_code: &dyn miette::SourceCode,
) -> Option<DiagnosticRelatedInformation> {
    let sc = source_code.read_span(label.inner(), 0, 0).ok()?;
    let path = sc.name()?;
    let uri = Url::from_file_path(path)?;
    let line = sc.line() as u32;
    let col = sc.column() as u32;
    let len = label.len() as u32;
    let range = Range::new(Position::new(line, col), Position::new(line, col + len));
    Some(DiagnosticRelatedInformation {
        location: Location { uri, range },
        message: label.label().unwrap_or("").to_string(),
    })
}

fn to_location(token: &Token) -> Option<Location> {
    let line = token.line - 1;
    let column = token.column - 1;
    let length = token.length;
    let uri = Url::from_file_path(token.source.to_string())?;
    let range = Range::new(
        Position::new(line, column),
        Position::new(line, column + length),
    );
    Some(Location { uri, range })
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
        VerylSymbolKind::TbComponent(x) => {
            if let TbComponentKind::External(key) = x.kind {
                if let Some(manifest) = component_manifest_table::get(key) {
                    items.append(&mut component_method_items(&manifest));
                }
            } else {
                // Builtin component methods are registered as function
                // symbols under the component's namespace.
                let mut namespace = symbol.namespace.clone();
                namespace.push(symbol.token.text);
                for method in symbol_table::get_all() {
                    if method.namespace.paths == namespace.paths
                        && matches!(method.kind, VerylSymbolKind::Function(_))
                    {
                        let label = method.token.text.to_string();
                        let item = CompletionItem {
                            label: label.clone(),
                            kind: Some(CompletionItemKind::METHOD),
                            detail: Some(format!("{}", method.kind)),
                            insert_text: Some(label),
                            ..Default::default()
                        };
                        items.push(item);
                    }
                }
            }
        }
        _ => (),
    }
    items
}

fn component_method_items(manifest: &ComponentManifest) -> Vec<CompletionItem> {
    manifest
        .methods
        .iter()
        .map(|method| {
            let args: Vec<_> = method
                .args
                .iter()
                .map(|arg| format!("{}: {}", arg.name, arg.ty))
                .collect();
            let documentation = method.doc.as_ref().map(|doc| {
                Documentation::MarkupContent(MarkupContent {
                    kind: MarkupKind::Markdown,
                    value: doc.clone(),
                })
            });
            let ret = method.ret_suffix();
            CompletionItem {
                label: method.name.clone(),
                kind: Some(CompletionItemKind::METHOD),
                detail: Some(format!("{}({}){}", method.name, args.join(", "), ret)),
                documentation,
                insert_text: Some(method.name.clone()),
                ..Default::default()
            }
        })
        .collect()
}

/// Instantiation text for an external component: method-only components are
/// declared with `var`, clocked ones with `inst` and port connections.
/// Required parameters are included so the inserted text names everything
/// that must be filled in.
fn component_new_text(name: &str, manifest: Option<&ComponentManifest>) -> String {
    if let Some(manifest) = manifest
        && manifest.kind.as_deref() == Some("method_only")
    {
        let args = generic_arg_placeholders(manifest);
        if args.is_empty() {
            format!("{name};")
        } else {
            format!("{name}::<{args}>;")
        }
    } else {
        let mut params = String::new();
        let mut ports = String::new();
        if let Some(manifest) = manifest {
            for param in manifest.params.iter().filter(|x| !x.optional) {
                params.push_str(&format!("{}: , ", param.name));
            }
            for port in &manifest.ports {
                ports.push_str(&format!("{}, ", port.name));
            }
            // Group members connect through one `<group>: ` connection.
            for group in &manifest.groups {
                ports.push_str(&format!("{}: , ", group.name));
            }
        }
        if params.is_empty() {
            format!("{name} ({ports});")
        } else {
            format!("{name} #({params}) ({ports});")
        }
    }
}

/// Positional generic arguments of a `var` component declaration must cover
/// every required parameter; parameter names stand in as placeholders.
fn generic_arg_placeholders(manifest: &ComponentManifest) -> String {
    match manifest.params.iter().rposition(|x| !x.optional) {
        Some(last_required) => manifest.params[..=last_required]
            .iter()
            .map(|x| x.name.as_str())
            .collect::<Vec<_>>()
            .join(", "),
        None => String::new(),
    }
}

fn component_documentation(symbol: &Symbol) -> Option<Documentation> {
    let VerylSymbolKind::TbComponent(x) = &symbol.kind else {
        return None;
    };
    let TbComponentKind::External(key) = x.kind else {
        return None;
    };
    let manifest = component_manifest_table::get(key)?;
    Some(Documentation::MarkupContent(MarkupContent {
        kind: MarkupKind::Markdown,
        value: manifest_markdown(&manifest),
    }))
}

fn manifest_markdown(manifest: &ComponentManifest) -> String {
    fn doc_suffix(doc: &Option<String>) -> String {
        doc.as_ref().map(|x| format!(" — {x}")).unwrap_or_default()
    }

    let mut ret = String::new();
    if let Some(doc) = &manifest.doc {
        ret.push_str(doc);
        ret.push('\n');
    }
    if let Some(kind) = &manifest.kind {
        ret.push_str(&format!("{kind} component\n"));
    } else {
        ret.push_str("component kind is not declared; both `inst` and `var` forms may apply\n");
    }
    if !manifest.ports.is_empty() {
        ret.push_str("\n**Ports**\n");
        for port in &manifest.ports {
            // A role names the Veryl type directly (always 1 bit); data-port
            // widths are inferred from the connection.
            let ty = match port.role.as_deref() {
                Some(role) => role.to_string(),
                None => port.dir.clone(),
            };
            ret.push_str(&format!(
                "- `{}`: {}{}\n",
                port.name,
                ty,
                doc_suffix(&port.doc)
            ));
        }
    }
    for group in &manifest.groups {
        ret.push_str(&format!(
            "\n**Interface `{}`** ({}.{})\n",
            group.name, group.interface, group.modport
        ));
        for m in &group.members {
            ret.push_str(&format!(
                "- `{}`: {}{}\n",
                m.member,
                m.dir,
                doc_suffix(&m.doc)
            ));
        }
    }
    if !manifest.params.is_empty() {
        ret.push_str("\n**Params**\n");
        for param in &manifest.params {
            let optional = if param.optional { " (optional)" } else { "" };
            ret.push_str(&format!(
                "- `{}`: {}{}{}\n",
                param.name,
                param.ty,
                optional,
                doc_suffix(&param.doc)
            ));
        }
    }
    if !manifest.methods.is_empty() {
        ret.push_str("\n**Methods**\n");
        for method in &manifest.methods {
            let args: Vec<_> = method
                .args
                .iter()
                .map(|arg| format!("{}: {}", arg.name, arg.ty))
                .collect();
            let ret_ty = method.ret_suffix();
            ret.push_str(&format!(
                "- `{}({}){}`{}\n",
                method.name,
                args.join(", "),
                ret_ty,
                doc_suffix(&method.doc)
            ));
        }
    }
    ret
}

fn completion_member(url: &Url, line: usize, column: usize, text: &str) -> Vec<CompletionItem> {
    let current_namespace = current_namespace(url, line, column);
    let Some(text) = resource_table::get_str_id(text.to_string()) else {
        return vec![];
    };
    let mut items = Vec::new();

    if let Some(namespace) = current_namespace
        && let Ok(symbol) = symbol_table::resolve((&vec![text], &namespace))
    {
        match &symbol.found.kind {
            VerylSymbolKind::Port(x) => {
                if let TypeKind::UserDefined(x) = &x.r#type.kind
                    && let Some(id) = x.symbol
                {
                    let symbol = symbol_table::get(id).unwrap();
                    items.append(&mut get_member(&symbol));
                }
            }
            VerylSymbolKind::Variable(x) => {
                if let TypeKind::UserDefined(x) = &x.r#type.kind {
                    if let Some(id) = x.symbol {
                        let symbol = symbol_table::get(id).unwrap();
                        items.append(&mut get_member(&symbol));
                    } else if let Ok(type_symbol) =
                        symbol_table::resolve_generic_structural(&x.path, &symbol.found.namespace)
                    {
                        items.append(&mut get_member(&type_symbol.found));
                    }
                }
            }
            VerylSymbolKind::Instance(x) => {
                if let Ok(type_symbol) =
                    symbol_table::resolve_generic_structural(&x.type_name, &symbol.found.namespace)
                {
                    items.append(&mut get_member(&type_symbol.found));
                }
            }
            _ => (),
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
        // External components live under `$comp::(<project>::)?`, which
        // is deeper than the top-level filter allows for the dependency form.
        let external_component = matches!(
            &symbol.kind,
            VerylSymbolKind::TbComponent(x) if matches!(x.kind, TbComponentKind::External(_))
        );

        if top_level_item || current_item || external_component {
            let prefix = if symbol.namespace.paths.is_empty()
                || symbol.namespace.paths[0] == prj
                || current_item
            {
                "".to_string()
            } else if external_component {
                symbol
                    .namespace
                    .paths
                    .iter()
                    .map(|x| format!("{x}::"))
                    .collect()
            } else {
                format!("{}::", symbol.namespace.paths[0])
            };
            let (new_text, kind) = match symbol.kind {
                VerylSymbolKind::Module(ref x) if !x.is_proto => {
                    let mut ports = String::new();
                    for port in &x.ports {
                        ports.push_str(&format!("{}, ", port.name()));
                    }
                    let text = format!("{}{} ({});", prefix, symbol.token.text, ports);
                    (text, Some(CompletionItemKind::CLASS))
                }
                VerylSymbolKind::Interface(ref x) if !x.is_proto => {
                    let text = format!("{}{} ();", prefix, symbol.token.text);
                    (text, Some(CompletionItemKind::INTERFACE))
                }
                VerylSymbolKind::Package(ref x) if !x.is_proto => {
                    let text = format!("{}{}::", prefix, symbol.token.text);
                    (text, Some(CompletionItemKind::MODULE))
                }
                VerylSymbolKind::TbComponent(ref x) => {
                    if let TbComponentKind::External(key) = x.kind {
                        let manifest = component_manifest_table::get(key);
                        let text = component_new_text(
                            &format!("{}{}", prefix, symbol.token.text),
                            manifest.as_deref(),
                        );
                        (text, Some(CompletionItemKind::CLASS))
                    } else {
                        let text = format!("{}{}", prefix, symbol.token.text);
                        (text, None)
                    }
                }
                VerylSymbolKind::Port(_)
                | VerylSymbolKind::Variable(_)
                | VerylSymbolKind::Parameter(_) => {
                    let text = format!("{}{}", prefix, symbol.token.text);
                    (text, Some(CompletionItemKind::VARIABLE))
                }
                VerylSymbolKind::Function(ref x) if !x.is_proto => {
                    let text = format!("{}{}", prefix, symbol.token.text);
                    (text, Some(CompletionItemKind::FUNCTION))
                }
                VerylSymbolKind::SystemFunction(_) => {
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
                component_documentation(&symbol)
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
    let path = url.to_file_path()?;
    let url = resource_table::get_path_id(path.to_path_buf())?;

    let mut ret = None;
    let mut ret_func = None;
    for symbol in symbol_table::get_all() {
        match symbol.kind {
            VerylSymbolKind::Module(x)
                if !x.is_proto && x.range.include(url, line as u32, column as u32) =>
            {
                let mut namespace = symbol.namespace;
                namespace.push(symbol.token.text);
                ret = Some(namespace);
            }
            VerylSymbolKind::Function(x)
                if !x.is_proto && x.range.include(url, line as u32, column as u32) =>
            {
                let mut namespace = symbol.namespace;
                namespace.push(symbol.token.text);
                ret_func = Some(namespace);
            }
            VerylSymbolKind::Interface(x)
                if !x.is_proto && x.range.include(url, line as u32, column as u32) =>
            {
                let mut namespace = symbol.namespace;
                namespace.push(symbol.token.text);
                ret = Some(namespace);
            }
            VerylSymbolKind::Package(x)
                if !x.is_proto && x.range.include(url, line as u32, column as u32) =>
            {
                let mut namespace = symbol.namespace;
                namespace.push(symbol.token.text);
                ret = Some(namespace);
            }
            _ => (),
        }
    }

    ret_func.or(ret)
}

fn drop_tables(path: PathId) {
    symbol_table::drop(path);
    scope::drop_tokens(path);
    text_table::drop(path);
    attribute_table::drop(path);
    unsafe_table::drop(path);
    definition_table::drop(path);
}

fn completion_keyword() -> Vec<CompletionItem> {
    let mut items = Vec::new();
    for keyword in KEYWORDS {
        let label = keyword.to_string();
        let insert_text = Some(label.clone());

        let item = CompletionItem {
            label: keyword.to_string(),
            kind: Some(CompletionItemKind::KEYWORD),
            detail: None,
            insert_text,
            ..Default::default()
        };
        items.push(item);
    }

    items
}

fn is_keyword_token(token: Token) -> bool {
    let token_text = token.text.to_string();
    KEYWORDS.contains(&token_text.as_str())
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

#[cfg(test)]
mod tests {
    use super::*;
    use veryl_metadata::component_manifest::{
        ManifestGroup, ManifestMember, ManifestMethod, ManifestParam, ManifestPort,
    };

    fn manifest() -> ComponentManifest {
        ComponentManifest {
            kind: Some("clocked".to_string()),
            doc: Some("Golden model checker.".to_string()),
            ports: vec![
                ManifestPort {
                    name: "clk".to_string(),
                    dir: "input".to_string(),
                    role: Some("clock".to_string()),
                    doc: Some("Sampling clock.".to_string()),
                },
                ManifestPort {
                    name: "q".to_string(),
                    dir: "output".to_string(),
                    role: None,
                    doc: None,
                },
            ],
            params: vec![ManifestParam {
                name: "XLEN".to_string(),
                ty: "u64".to_string(),
                optional: false,
                doc: None,
            }],
            methods: vec![ManifestMethod {
                name: "load".to_string(),
                args: vec![ManifestParam {
                    name: "path".to_string(),
                    ty: "str".to_string(),
                    optional: false,
                    doc: None,
                }],
                ret: Some("u64".to_string()),
                ret_width: None,
                doc: Some("Load an ELF file.".to_string()),
            }],
            requires: vec![],
            groups: vec![],
        }
    }

    #[test]
    fn component_method_completion() {
        let items = component_method_items(&manifest());
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].label, "load");
        assert_eq!(items[0].detail.as_deref(), Some("load(path: str) -> u64"));
        assert_eq!(items[0].insert_text.as_deref(), Some("load"));
        let Some(Documentation::MarkupContent(doc)) = &items[0].documentation else {
            panic!("expected markdown documentation");
        };
        assert_eq!(doc.value, "Load an ELF file.");
    }

    #[test]
    fn component_inst_text() {
        let text = component_new_text("$comp::x", Some(&manifest()));
        assert_eq!(text, "$comp::x #(XLEN: , ) (clk, q, );");

        let mut method_only = manifest();
        method_only.kind = Some("method_only".to_string());
        let text = component_new_text("$comp::x", Some(&method_only));
        assert_eq!(text, "$comp::x::<XLEN>;");

        let text = component_new_text("$comp::x", None);
        assert_eq!(text, "$comp::x ();");
    }

    #[test]
    fn component_inst_text_collapses_port_groups() {
        let mut grouped = manifest();
        grouped.params.clear();
        grouped.groups.push(ManifestGroup {
            name: "axi".to_string(),
            interface: "$std::axi4_if".to_string(),
            modport: "monitor".to_string(),
            members: vec![
                ManifestMember {
                    member: "awvalid".to_string(),
                    dir: "input".to_string(),
                    doc: None,
                },
                ManifestMember {
                    member: "awready".to_string(),
                    dir: "input".to_string(),
                    doc: None,
                },
            ],
            doc: None,
        });
        let text = component_new_text("$comp::x", Some(&grouped));
        assert_eq!(text, "$comp::x (clk, q, axi: , );");
    }

    #[test]
    fn component_inst_text_optional_params() {
        let mut optional = manifest();
        optional.params[0].optional = true;
        let text = component_new_text("$comp::x", Some(&optional));
        assert_eq!(text, "$comp::x (clk, q, );");

        optional.kind = Some("method_only".to_string());
        let text = component_new_text("$comp::x", Some(&optional));
        assert_eq!(text, "$comp::x;");

        // Positional generic arguments must reach the last required
        // parameter, spanning optional ones before it.
        optional.params.push(ManifestParam {
            name: "DEPTH".to_string(),
            ty: "u64".to_string(),
            optional: false,
            doc: None,
        });
        let text = component_new_text("$comp::x", Some(&optional));
        assert_eq!(text, "$comp::x::<XLEN, DEPTH>;");
    }

    #[test]
    fn external_component_member_completion() {
        use veryl_analyzer::symbol::{DocComment, TbComponentProperty};
        use veryl_parser::veryl_token::TokenSource;

        let key = resource_table::insert_str("golden");
        component_manifest_table::insert(key, manifest());

        let token = Token::new("golden", 0, 0, 0, 0, TokenSource::Builtin);
        let symbol = Symbol::new(
            &token,
            VerylSymbolKind::TbComponent(TbComponentProperty {
                kind: TbComponentKind::External(key),
                generic_parameters: vec![],
            }),
            &Namespace::new(),
            true,
            DocComment::default(),
        );

        let items = get_member(&symbol);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].label, "load");

        assert!(component_documentation(&symbol).is_some());
    }

    #[test]
    fn component_manifest_markdown() {
        let markdown = manifest_markdown(&manifest());
        assert!(markdown.starts_with("Golden model checker.\n"));
        assert!(markdown.contains("clocked component"));
        assert!(markdown.contains("- `clk`: clock — Sampling clock."));
        assert!(markdown.contains("- `q`: output\n"));
        assert!(markdown.contains("- `XLEN`: u64\n"));
        assert!(markdown.contains("- `load(path: str) -> u64` — Load an ELF file."));
    }

    #[test]
    fn component_manifest_markdown_undeclared_fields() {
        let mut manifest = manifest();
        manifest.kind = None;
        manifest.ports[0].role = None;
        manifest.params[0].optional = true;
        let markdown = manifest_markdown(&manifest);
        assert!(markdown.contains("component kind is not declared"));
        assert!(markdown.contains("- `clk`: input — Sampling clock."));
        assert!(markdown.contains("- `XLEN`: u64 (optional)\n"));
    }
}
