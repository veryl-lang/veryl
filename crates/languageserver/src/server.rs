use crate::coordinates::{byte_position, parser_column, utf16_column};
use crate::incremental::LsIncrementalMap;
use crate::keyword::KEYWORDS;
use async_channel::{Receiver, Sender};
use dashmap::DashMap;
use futures::executor::block_on;
use ropey::Rope;
use std::borrow::Cow;
use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::io::ErrorKind;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tower_lsp_server::Client;
use tower_lsp_server::ls_types::ClientCapabilities;
use tower_lsp_server::ls_types::Uri as Url;
use tower_lsp_server::ls_types::*;
use veryl_analyzer::namespace::Namespace;
use veryl_analyzer::reference_table::{self, ReferenceCandidate};
use veryl_analyzer::symbol::SymbolKind as VerylSymbolKind;
use veryl_analyzer::symbol::{ParameterKind, Symbol, SymbolId, TbComponentKind, TypeKind};
use veryl_analyzer::symbol_path::{SymbolPath, SymbolPathNamespace};
use veryl_analyzer::symbol_table::{self, Import, PendingWatermark};
use veryl_analyzer::{
    Analyzer, AnalyzerError, Context, component_manifest_table, fragment_cache, scope,
};
use veryl_formatter::Formatter;
use veryl_metadata::{ComponentManifest, Metadata, MetadataError};
use veryl_parser::resource_table::{self, PathId};
use veryl_parser::veryl_token::{Token, TokenSource};
use veryl_parser::veryl_walker::VerylWalker;
use veryl_parser::{Finder, Parser, ParserError};
use veryl_path::PathSet;

const REFERENCE_REFRESH_DELAY: Duration = Duration::from_millis(100);

pub struct Capability {
    work_done_progress: bool,
    semantic_tokens_refresh: bool,
}

impl From<ClientCapabilities> for Capability {
    fn from(value: ClientCapabilities) -> Self {
        let work_done_progress = if let Some(x) = &value.window {
            x.work_done_progress.unwrap_or(false)
        } else {
            false
        };

        let semantic_tokens_refresh = value
            .workspace
            .and_then(|workspace| workspace.semantic_tokens)
            .and_then(|tokens| tokens.refresh_support)
            .unwrap_or(false);

        Self {
            work_done_progress,
            semantic_tokens_refresh,
        }
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
    DidClose {
        url: Url,
    },
    DiskSourcesChanged(Vec<Url>),
    DidChangeConfiguration(ServerConfigItem),
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
    runtime: tokio::runtime::Handle,
    rcv: Receiver<MsgToServer>,
    snd: Sender<MsgFromServer>,
    document_map: DashMap<PathBuf, Rope>,
    parser_map: DashMap<PathBuf, Parser>,
    metadata_map: DashMap<PathBuf, Metadata>,
    cache_dir: PathBuf,
    lsp_token: i32,
    background_tasks: VecDeque<BackgroundTask>,
    loaded_projects: HashMap<PathBuf, Arc<Metadata>>,
    config: ServerConfig,
    pending_diagnostics: BTreeMap<Url, i32>,
    pending_disk_updates: BTreeSet<Url>,
    reference_refresh_at: Option<Instant>,
    work_done_progress: bool,
    semantic_tokens_refresh: bool,
    document_inputs: BTreeMap<PathId, DocumentInputs>,
    references: ReferenceIndex,
    incremental: LsIncrementalMap,
}

struct DocumentInputs {
    imports: Vec<Import>,
    candidates: Vec<ReferenceCandidate>,
}

impl DocumentInputs {
    fn capture(imports: PendingWatermark, candidates: usize) -> Self {
        Self {
            imports: symbol_table::export_imports_since(imports),
            candidates: reference_table::export_candidates_since(candidates),
        }
    }
}

struct DiskSource {
    path: PathBuf,
    metadata: Arc<Metadata>,
    text: String,
}

#[derive(Default)]
struct ReferenceIndex {
    by_file: HashMap<PathId, Vec<(SymbolId, Token)>>,
    by_symbol: HashMap<SymbolId, Vec<Token>>,
}

impl Server {
    pub fn new(client: Client, rcv: Receiver<MsgToServer>, snd: Sender<MsgFromServer>) -> Self {
        Server {
            client,
            runtime: tokio::runtime::Handle::current(),
            rcv,
            snd,
            document_map: DashMap::new(),
            parser_map: DashMap::new(),
            metadata_map: DashMap::new(),
            cache_dir: veryl_path::cache_path(),
            lsp_token: 0,
            background_tasks: VecDeque::new(),
            loaded_projects: HashMap::new(),
            config: ServerConfig::default(),
            pending_diagnostics: BTreeMap::new(),
            pending_disk_updates: BTreeSet::new(),
            reference_refresh_at: None,
            work_done_progress: true,
            semantic_tokens_refresh: false,
            document_inputs: BTreeMap::new(),
            references: ReferenceIndex::default(),
            incremental: LsIncrementalMap::default(),
        }
    }

    pub fn serve(&mut self) {
        loop {
            let msg = match self.rcv.try_recv() {
                Ok(msg) => Ok(msg),
                Err(async_channel::TryRecvError::Closed) => break,
                Err(async_channel::TryRecvError::Empty) => {
                    if !self.background_tasks.is_empty() {
                        self.run_background_step();
                        continue;
                    }
                    if let Some(deadline) = self.reference_refresh_at {
                        match self.runtime.block_on(async {
                            tokio::time::timeout_at(deadline.into(), self.rcv.recv()).await
                        }) {
                            Ok(msg) => msg,
                            Err(_) => {
                                self.finish_reference_refresh();
                                continue;
                            }
                        }
                    } else {
                        self.rcv.recv_blocking()
                    }
                }
            };
            let Ok(msg) = msg else {
                break;
            };
            match msg {
                MsgToServer::Initialize { capability } => {
                    self.work_done_progress = capability.work_done_progress;
                    self.semantic_tokens_refresh = capability.semantic_tokens_refresh;
                }
                MsgToServer::DidOpen { url, text, version } => self.did_open(&url, &text, version),
                MsgToServer::DidChange { url, text, version } => {
                    self.on_change(&url, &text, version)
                }
                MsgToServer::DidClose { url } => self.did_close(&url),
                MsgToServer::DiskSourcesChanged(urls) => self.update_disk_sources(urls),
                MsgToServer::DidChangeConfiguration(x) => self.config.set(x),
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
    }

    fn run_background_step(&mut self) {
        let mut task = self.background_tasks.pop_front().unwrap();
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
        if !task.paths.is_empty() {
            self.background_tasks.push_front(task);
            return;
        }

        self.apply_project_imports();
        Analyzer::analyze_post_pass1();
        let metadata = Arc::new(task.metadata);
        self.loaded_projects
            .insert(metadata.metadata_path.clone(), Arc::clone(&metadata));
        if self.background_tasks.is_empty() {
            let urls = std::mem::take(&mut self.pending_disk_updates);
            if self.reload_closed_files(urls) {
                self.apply_project_imports();
                Analyzer::analyze_post_pass1();
            }
            for (url, version) in std::mem::take(&mut self.pending_diagnostics) {
                let text = url.to_file_path().and_then(|path| {
                    self.document_map
                        .get(path.as_ref())
                        .map(|rope| rope.to_string())
                });
                if let Some(text) = text {
                    self.on_change(&url, &text, version);
                }
            }
        }
        if let Some(inc) = self.incremental.get(&metadata) {
            inc.save();
        }
        self.progress_done("background analyze done");
        if self.background_tasks.is_empty() {
            self.finish_reference_refresh();
        }
    }
}

impl Server {
    fn did_open(&mut self, url: &Url, text: &str, version: i32) {
        if let Ok(metadata) = self.get_metadata(url)
            && !self.loaded_projects.contains_key(&metadata.metadata_path)
            && !self
                .background_tasks
                .iter()
                .any(|task| task.metadata.metadata_path == metadata.metadata_path)
            && url
                .to_file_path()
                .is_some_and(|path| !path.starts_with(&self.cache_dir))
        {
            self.queue_background_analysis(metadata, url);
        }
        self.on_change(url, text, version);
    }

    fn did_close(&mut self, url: &Url) {
        let Some(path) = url.to_file_path() else {
            return;
        };
        if self.document_map.remove(path.as_ref()).is_none() {
            return;
        }
        self.parser_map.remove(path.as_ref());
        let path = path.into_owned();
        self.pending_diagnostics.remove(url);
        if !self.background_tasks.is_empty() {
            self.drop_closed_file(&path);
            self.pending_disk_updates.insert(url.clone());
            return;
        }
        if self.reload_closed_files([url.clone()]) || self.drop_closed_file(&path) {
            self.update_project_after_disk_change();
        }
    }

    fn update_disk_sources(&mut self, urls: impl IntoIterator<Item = Url>) {
        if !self.background_tasks.is_empty() {
            self.pending_disk_updates.extend(urls);
            return;
        }
        if self.reload_closed_files(urls) {
            self.update_project_after_disk_change();
        }
    }

    fn reload_closed_files(&mut self, urls: impl IntoIterator<Item = Url>) -> bool {
        let urls: BTreeSet<_> = urls.into_iter().collect();
        let mut sources = Vec::new();
        let mut changed = false;
        // Drop the previous declarations for all changed paths before registering
        // replacements. A rename can move declarations between those paths.
        for url in urls {
            changed |= self.prepare_closed_file(&url, &mut sources);
        }
        for DiskSource {
            path,
            metadata,
            text,
        } in sources
        {
            self.analyze_disk_source(&path, &metadata.project.name, &metadata, &text);
        }
        changed
    }

    fn prepare_closed_file(&mut self, url: &Url, sources: &mut Vec<DiskSource>) -> bool {
        let Some(path) = url.to_file_path() else {
            return false;
        };
        if path.extension().is_none_or(|ext| ext != "veryl")
            || path.starts_with(&self.cache_dir)
            || self.document_map.contains_key(path.as_ref())
        {
            return false;
        }
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(err) if matches!(err.kind(), ErrorKind::NotFound | ErrorKind::InvalidData) => {
                return self.drop_closed_file(&path.into_owned());
            }
            Err(_) => return false,
        };
        let Ok(manifest) = Metadata::search_from(&path) else {
            return false;
        };
        let Ok(canonical_manifest) = manifest.canonicalize() else {
            return false;
        };
        let Some(metadata) = self.loaded_projects.get(&canonical_manifest).cloned() else {
            return false;
        };
        let project_root = metadata.project_path();
        let editor_root = manifest.parent().unwrap();
        let source_path = if editor_root
            .canonicalize()
            .is_ok_and(|root| root == project_root)
            && let Ok(relative) = path.strip_prefix(editor_root)
        {
            project_root.join(relative)
        } else {
            path.to_path_buf()
        };
        if !metadata.contains_source_path(&source_path) {
            return false;
        }
        let path = path.into_owned();
        let project = &metadata.project.name;
        if let Some(id) = resource_table::get_path_id(&path) {
            self.drop_document(id, Some(project));
        }
        sources.push(DiskSource {
            path,
            metadata,
            text,
        });
        true
    }

    fn drop_closed_file(&mut self, path: &PathBuf) -> bool {
        if let Some(id) = resource_table::get_path_id(path)
            && self.document_inputs.contains_key(&id)
        {
            self.drop_document(id, None);
            true
        } else {
            false
        }
    }

    fn queue_background_analysis(&mut self, mut metadata: Metadata, url: &Url) {
        let manifest = url
            .to_file_path()
            .and_then(|path| Metadata::search_from(path).ok());
        if let (Some(manifest), Ok(mut paths)) = (manifest, metadata.paths::<&str>(&[], true, true))
        {
            let editor_root = manifest.parent().unwrap();
            let project_root = metadata.project_path();
            // Use the editor's path spelling for both disk sources and open buffers.
            if editor_root
                .canonicalize()
                .is_ok_and(|root| root == project_root)
            {
                for path in &mut paths {
                    if path.prj == metadata.project.name
                        && let Ok(relative) = path.src.strip_prefix(&project_root)
                    {
                        path.src = editor_root.join(relative);
                    }
                }
            }
            self.background_tasks.push_back(BackgroundTask {
                total: paths.len(),
                metadata,
                paths,
                progress: false,
            });
        }
    }

    fn get_line(&self, url: &Url, line: usize) -> Option<String> {
        if let Some(path) = url.to_file_path()
            && let Some(rope) = self.document_map.get(path.as_ref())
            && let Some(slice) = rope.get_line(line - 1)
            && let Some(text) = slice.as_str()
        {
            return Some(text.to_string());
        }
        None
    }

    fn source_column(&self, url: &Url, line: usize, column: usize) -> usize {
        url.to_file_path()
            .and_then(|path| {
                self.document_map
                    .get(path.as_ref())
                    .map(|text| parser_column(&text, line - 1, column - 1))
            })
            .unwrap_or(column)
    }

    fn completion(
        &mut self,
        url: &Url,
        line: usize,
        column: usize,
        context: &Option<CompletionContext>,
    ) {
        let source_column = self.source_column(url, line, column);
        let ret = if let Some(context) = context {
            match context.trigger_kind {
                CompletionTriggerKind::TRIGGER_CHARACTER => {
                    let trigger = context.trigger_character.as_ref().unwrap();
                    if trigger == "." {
                        if let Some(text) = self.get_line(url, line) {
                            let text = text.split_whitespace().last().unwrap();
                            let text = text.trim_matches(|c| !char::is_alphanumeric(c));
                            let items = completion_member(url, line, source_column, text);
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
                    let mut items = if let Ok(metadata) = self.get_metadata(url) {
                        completion_symbol(&metadata, url, line, source_column)
                    } else {
                        vec![]
                    };
                    items.append(&mut completion_keyword());
                    if let Some(text) = self.get_line(url, line) {
                        set_completion_edit_ranges(&mut items, &text, line, source_column);
                    }
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

    fn symbol_at(&self, url: &Url, line: usize, column: usize) -> Option<Rc<Symbol>> {
        let path = url.to_file_path()?;
        let parser = self.parser_map.get(path.as_ref())?;
        let mut finder = Finder::new();
        finder.line = line as u32;
        finder.column = self.source_column(url, line, column) as u32;
        finder.veryl(&parser.veryl);
        let token = finder.token?;

        if let Some((scope, define_context)) = scope::token_scope(token.id) {
            let path = if finder.token_group.is_empty() {
                SymbolPath::new(&[token.text])
            } else {
                SymbolPath::from(finder.token_group.as_slice())
            };
            if let Ok(symbol) =
                symbol_table::resolve(SymbolPathNamespace::from_scope(path, scope, define_context))
            {
                return Some(Rc::clone(&symbol.found));
            }
        }

        // The reference analysis has the full generic syntax, which the token
        // group's name-only path cannot always represent.
        let path = resource_table::get_path_id(path.to_path_buf())?;
        let references = self.file_references(path)?;
        references
            .iter()
            .find(|(_, reference)| reference.id == token.id)
            .and_then(|(id, _)| symbol_table::get_shared(*id))
    }

    fn goto_definition(&mut self, url: &Url, line: usize, column: usize) {
        let location = self
            .symbol_at(url, line, column)
            .and_then(|symbol| to_location(&symbol.token, &mut HashMap::new()));
        self.snd
            .send_blocking(MsgFromServer::GotoDefinition(location))
            .unwrap();
    }

    fn symbol(&mut self, query: &str) {
        let mut sources = HashMap::new();
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
                    VerylSymbolKind::PropNamespace => SymbolKind::NAMESPACE,
                };
                if let Some(location) = to_location(&symbol.token, &mut sources) {
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
        let hover = self.symbol_at(url, line, column).map(|symbol| Hover {
            contents: HoverContents::Scalar(MarkedString::String(symbol.kind.to_string())),
            range: None,
        });
        self.snd.send_blocking(MsgFromServer::Hover(hover)).unwrap();
    }

    fn references(&mut self, url: &Url, line: usize, column: usize) {
        if self.background_tasks.is_empty() && self.reference_refresh_at.is_some() {
            self.finish_reference_refresh();
        }
        let mut sources = HashMap::new();
        let mut locations = Vec::new();
        if let Some(symbol) = self.symbol_at(url, line, column)
            && let Some(references) = self.references.by_symbol.get(&symbol.id)
        {
            locations.extend(
                references
                    .iter()
                    .filter_map(|token| to_location(token, &mut sources)),
            );
        }
        self.snd
            .send_blocking(MsgFromServer::References(locations))
            .unwrap();
    }

    fn semantic_tokens(&mut self, url: &Url) {
        let ret = url
            .to_file_path()
            .and_then(|path| resource_table::get_path_id(path.to_path_buf()))
            .map(|path| {
                let references = self.file_references(path);
                SemanticTokensResult::Tokens(SemanticTokens {
                    result_id: None,
                    data: collect_semantic_tokens(path, references.as_deref()),
                })
            });

        self.snd
            .send_blocking(MsgFromServer::SemanticTokens(ret))
            .unwrap();
    }

    fn file_references(&self, path: PathId) -> Option<Cow<'_, [(SymbolId, Token)]>> {
        if self.reference_refresh_at.is_some() || !self.background_tasks.is_empty() {
            self.document_inputs.get(&path).map(|document| {
                Cow::Owned(reference_table::collect_references(
                    document.candidates.iter(),
                ))
            })
        } else {
            self.references
                .by_file
                .get(&path)
                .map(|references| Cow::Borrowed(references.as_slice()))
        }
    }

    fn refresh_semantic_tokens(&self) {
        if self.semantic_tokens_refresh {
            let client = self.client.clone();
            self.runtime.spawn(async move {
                let _ = client.semantic_tokens_refresh().await;
            });
        }
    }

    fn formatting(&mut self, url: &Url) {
        if let Some(path) = url.to_file_path()
            && let Ok(metadata) = self.get_metadata(url)
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
        let track_references = path.prj == metadata.project.name;
        if track_references
            && let Some(id) = resource_table::get_path_id(&src)
            && let Some(document) = self.document_inputs.remove(&id)
        {
            symbol_table::remove_imports(&document.imports);
        }

        // A cache hit restores this file's pass1 state, skipping parse.
        let imports = symbol_table::pending_watermark();
        let candidates = reference_table::candidates_len();
        let restored = self
            .incremental
            .get(metadata)
            .is_some_and(|inc| inc.try_restore(path, &text));
        if restored {
            if track_references {
                let id = resource_table::get_path_id(&src).unwrap();
                self.document_inputs
                    .insert(id, DocumentInputs::capture(imports, candidates));
            }
            return;
        }

        // Clear stale state from a previous analysis before re-registering.
        if let Some(src_id) = resource_table::get_path_id(&src) {
            Analyzer::drop_file(src_id, Some(path.prj.as_str().into()));
        }
        // Snapshot the ID counters before parse so the file's pass1 output
        // can be captured as a fragment afterwards.
        let watermark = self
            .incremental
            .get(metadata)
            .map(|_| fragment_cache::watermark());
        if let Some(cacheable) = self.analyze_disk_source(&src, &path.prj, metadata, &text) {
            if let (Some(inc), Some(wm)) = (self.incremental.get(metadata), watermark.as_ref()) {
                inc.capture(path, &text, wm, cacheable);
            }

            block_on(self.client.log_message(
                MessageType::INFO,
                format!("background_analyze: {}", src.to_string_lossy()),
            ));
        }
    }

    fn analyze_disk_source(
        &mut self,
        path: &PathBuf,
        project: &str,
        metadata: &Metadata,
        text: &str,
    ) -> Option<bool> {
        let parser = Parser::parse(text, path).ok()?;
        let analyzer = Analyzer::new(metadata);
        let imports = symbol_table::pending_watermark();
        let candidates = reference_table::candidates_len();
        let errors = analyzer.analyze_pass1(project, &parser.veryl);
        if project == metadata.project.name {
            let id = resource_table::get_path_id(path).unwrap();
            self.document_inputs
                .insert(id, DocumentInputs::capture(imports, candidates));
        }
        Some(errors.is_empty())
    }

    fn apply_project_imports(&self) {
        symbol_table::apply_import();
        let imports = self
            .document_inputs
            .values()
            .flat_map(|document| &document.imports)
            .collect();
        symbol_table::reapply_imports(imports);
    }

    fn update_project_after_disk_change(&mut self) {
        self.apply_project_imports();
        Analyzer::analyze_post_pass1();
        self.reference_refresh_at = Some(Instant::now() + REFERENCE_REFRESH_DELAY);
    }

    fn finish_reference_refresh(&mut self) {
        self.reference_refresh_at = None;
        let candidates = self
            .document_inputs
            .values()
            .flat_map(|document| &document.candidates);
        let references = reference_table::collect_references(candidates);
        self.publish_references(references);
        self.refresh_semantic_tokens();
    }

    fn publish_references(&mut self, references: Vec<(SymbolId, Token)>) {
        let mut index = ReferenceIndex::default();
        for &path in self.document_inputs.keys() {
            index.by_file.insert(path, Vec::new());
        }
        for (symbol, token) in references {
            if let TokenSource::File { path, .. } = token.source
                && let Some(file_references) = index.by_file.get_mut(&path)
            {
                file_references.push((symbol, token));
                index.by_symbol.entry(symbol).or_default().push(token);
            }
        }
        symbol_table::for_each_symbol_with_references(|symbol, references| {
            for &token in references {
                if let TokenSource::File { path, .. } = token.source
                    // Implicit references such as the default clock at always_ff
                    // come from IR conversion rather than name resolution.
                    && (!index.by_file.contains_key(&path) || is_keyword_token(token))
                {
                    index.by_symbol.entry(symbol.id).or_default().push(token);
                }
            }
        });
        for references in index.by_symbol.values_mut() {
            references.sort_unstable_by_key(|token| (token.source, token.pos, token.length));
            references.dedup_by_key(|token| (token.source, token.pos, token.length));
        }
        self.references = index;
    }

    fn get_metadata(&mut self, url: &Url) -> Result<Metadata, MetadataError> {
        let path = url
            .to_file_path()
            .ok_or_else(|| MetadataError::FileNotFound(PathBuf::from(url.as_str())))?;
        if let Some(metadata) = self.metadata_map.get(path.as_ref()) {
            return Ok(metadata.to_owned());
        }
        let metadata_path = Metadata::search_from(path.as_ref())?;
        let metadata = Metadata::load(metadata_path)?;
        self.metadata_map
            .insert(path.to_path_buf(), metadata.clone());
        Ok(metadata)
    }

    fn on_change(&mut self, url: &Url, text: &str, version: i32) {
        if let Some(path) = url.to_file_path() {
            let rope = Rope::from_str(text);

            if path.starts_with(&self.cache_dir) {
                return;
            }

            match self.get_metadata(url) {
                Ok(metadata) => {
                    let prj = &metadata.project.name;
                    // Drop before parse: parse re-registers the text, so
                    // dropping after would erase the just-registered entry.
                    if let Some(path_id) = resource_table::get_path_id(path.to_path_buf()) {
                        self.drop_document(path_id, Some(prj));
                    }
                    let diag = match Parser::parse(text, &path) {
                        Ok(x) => {
                            let path_id = resource_table::get_path_id(path.to_path_buf());

                            let analyzer = Analyzer::new(&metadata);
                            let mut context = Context::default();
                            let mut ir = veryl_analyzer::ir::Ir::default();
                            let imports = symbol_table::pending_watermark();
                            let candidates = reference_table::candidates_len();
                            let mut errors = analyzer.analyze_pass1(prj, &x.veryl);
                            if let Some(path_id) = path_id {
                                self.document_inputs
                                    .insert(path_id, DocumentInputs::capture(imports, candidates));
                            }
                            self.apply_project_imports();
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
                                    if self.background_tasks.is_empty() {
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
                                                | AnalyzerError::ReferringInactiveDefinition { .. }
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
                            self.apply_project_imports();
                            Analyzer::analyze_post_pass1();
                            vec![to_diag(x.into(), &rope)]
                        }
                    };

                    block_on(
                        self.client
                            .publish_diagnostics(url.clone(), diag, Some(version)),
                    );
                    if self.background_tasks.is_empty() {
                        self.reference_refresh_at = Some(Instant::now() + REFERENCE_REFRESH_DELAY);
                    } else {
                        self.pending_diagnostics.insert(url.clone(), version);
                    }
                }
                Err(err) => {
                    block_on(self.client.log_message(
                        MessageType::ERROR,
                        format!(
                            "failed to load metadata: {}: {}",
                            url.as_str(),
                            error_chain(&err)
                        ),
                    ));
                }
            }

            self.document_map.insert(path.to_path_buf(), rope);
        }
    }

    fn on_remove(&mut self, url: Url) {
        if let Some(path) = url.to_file_path()
            && let Some(path_id) = resource_table::get_path_id(path.to_path_buf())
        {
            self.drop_document(path_id, None);
            if self.background_tasks.is_empty() {
                self.update_project_after_disk_change();
            }
        }
    }

    fn drop_document(&mut self, path: PathId, project: Option<&str>) {
        self.references.by_file.remove(&path);
        if let Some(document) = self.document_inputs.remove(&path) {
            symbol_table::remove_imports(&document.imports);
        }
        Analyzer::drop_file(path, project.map(Into::into));
    }
}

fn error_chain(err: &dyn std::error::Error) -> String {
    let mut message = err.to_string();
    let mut source = err.source();
    while let Some(x) = source {
        message.push_str(&format!(": {x}"));
        source = x.source();
    }
    message
}

fn to_diag(err: miette::ErrReport, rope: &Rope) -> Diagnostic {
    let miette_diag: &dyn miette::Diagnostic = err.as_ref();

    let (primary, participants) = miette_diag
        .labels()
        .map(|mut it| (it.next(), it.collect::<Vec<_>>()))
        .unwrap_or((None, Vec::new()));

    let range = primary
        .as_ref()
        .map(|label| byte_range(rope, label.offset(), label.len()))
        .unwrap_or_default();

    // Non-primary labels and related diagnostics become navigable
    // related-information.
    let mut related: Vec<_> = miette_diag
        .source_code()
        .map(|sc| {
            participants
                .iter()
                .filter_map(|label| label_to_related(label, sc))
                .collect()
        })
        .unwrap_or_default();
    for diag in miette_diag.related().into_iter().flatten() {
        let (Some(sc), Some(labels)) = (diag.source_code(), diag.labels()) else {
            continue;
        };
        for label in labels {
            if let Some(mut x) = label_to_related(&label, sc) {
                x.message = diag.to_string();
                related.push(x);
            }
        }
    }
    let related_information = (!related.is_empty()).then_some(related);

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

fn byte_range(rope: &Rope, offset: usize, length: usize) -> Range {
    let (line, column) = byte_position(rope, offset);
    let (end_line, end_column) = byte_position(rope, offset.saturating_add(length));
    Range::new(
        Position::new(line, column),
        Position::new(end_line, end_column),
    )
}

fn label_to_related(
    label: &miette::LabeledSpan,
    source_code: &dyn miette::SourceCode,
) -> Option<DiagnosticRelatedInformation> {
    let sc = source_code.read_span(label.inner(), 1, 0).ok()?;
    let uri = Url::from_file_path(sc.name()?)?;
    let text = std::str::from_utf8(sc.data()).ok()?;
    let offset = label.offset().checked_sub(sc.span().offset())?;
    let mut range = byte_range(&Rope::from_str(text), offset, label.len());
    range.start.line += sc.line() as u32;
    range.end.line += sc.line() as u32;
    Some(DiagnosticRelatedInformation {
        location: Location { uri, range },
        message: label.label().unwrap_or("").to_string(),
    })
}

fn to_location(token: &Token, sources: &mut HashMap<TokenSource, Rope>) -> Option<Location> {
    let uri = Url::from_file_path(token.source.to_string())?;
    let text = sources
        .entry(token.source)
        .or_insert_with(|| Rope::from_str(&token.source.get_text()));
    let range = byte_range(text, token.pos as usize, token.length as usize);
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

    // Not interned yet when no source of the project has been analyzed, e.g. every open
    // file has a syntax error.
    let prj = resource_table::insert_str(&metadata.project.name);

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

/// Replaces the typed text that each item's insert text starts with, so `$cl` completes to
/// `$clog2` and `$tb::c` to `$tb::clock_gen` instead of keeping the `$` / `$tb::` typed before
/// the editor's word, which does not include `$` or `:`.
fn set_completion_edit_ranges(
    items: &mut [CompletionItem],
    text: &str,
    line: usize,
    column: usize,
) {
    let before: Vec<char> = text.chars().take(column - 1).collect();
    let is_word = |c: &char| c.is_alphanumeric() || *c == '_';
    let is_path = |c: &char| is_word(c) || *c == '$' || *c == ':';
    let word_len = before.iter().rev().take_while(|c| is_word(c)).count();
    let path_len = before.iter().rev().take_while(|c| is_path(c)).count();

    let line = (line - 1) as u32;
    let end = Position::new(line, utf16_column(text, column));
    for item in items.iter_mut() {
        let Some(new_text) = item.insert_text.take() else {
            continue;
        };
        let replace_len = (word_len..=path_len)
            .rev()
            .find(|&len| {
                let typed: String = before[before.len() - len..].iter().collect();
                new_text.starts_with(&typed)
            })
            .unwrap_or(word_len);
        let start = Position::new(line, utf16_column(text, column - replace_len));
        item.text_edit = Some(CompletionTextEdit::Edit(TextEdit {
            range: Range::new(start, end),
            new_text,
        }));
    }
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

fn semantic_kind(kind: &VerylSymbolKind) -> Option<(semantic_legend::TokenType, u32)> {
    use semantic_legend::{READONLY, TokenType};

    Some(match kind {
        VerylSymbolKind::Port(_) => (TokenType::Property, 0),
        VerylSymbolKind::Function(_) => (TokenType::Function, 0),
        VerylSymbolKind::Struct(_) => (TokenType::Struct, 0),
        VerylSymbolKind::Union(_) | VerylSymbolKind::TypeDef(_) => (TokenType::Type, 0),
        VerylSymbolKind::Enum(_) => (TokenType::Enum, 0),
        VerylSymbolKind::EnumMember(_) => (TokenType::EnumMember, 0),
        VerylSymbolKind::GenericConst(_) => (TokenType::Variable, READONLY),
        VerylSymbolKind::Parameter(x) if matches!(x.kind, ParameterKind::Const) => {
            (TokenType::Variable, READONLY)
        }
        _ => return None,
    })
}

fn collect_semantic_tokens(
    path: PathId,
    refreshed_references: Option<&[(SymbolId, Token)]>,
) -> Vec<SemanticToken> {
    use semantic_legend::DECLARATION;

    let in_document = |token: Token| {
        matches!(token.source, TokenSource::File { path: source, .. } if source == path)
            && token.length != 0
    };
    let mut tokens = Vec::new();
    symbol_table::for_each_symbol_with_references(|symbol, references| {
        let Some((kind, modifiers)) = semantic_kind(&symbol.kind) else {
            return;
        };
        if in_document(symbol.token) {
            tokens.push((symbol.token, kind, modifiers | DECLARATION));
        }
        if refreshed_references.is_none() {
            for &reference in references {
                if in_document(reference) && !is_keyword_token(reference) {
                    tokens.push((reference, kind, modifiers));
                }
            }
        }
    });
    for &(id, reference) in refreshed_references.unwrap_or_default() {
        if in_document(reference)
            && !is_keyword_token(reference)
            && let Some(symbol) = symbol_table::get_shared(id)
            && let Some((kind, modifiers)) = semantic_kind(&symbol.kind)
        {
            tokens.push((reference, kind, modifiers));
        }
    }

    // Resolution can record the same span more than once. LSP tokens must not overlap.
    tokens.sort_unstable_by_key(|(token, kind, modifiers)| {
        (
            token.pos,
            Reverse(token.length),
            modifiers & DECLARATION == 0,
            *kind,
        )
    });

    let mut data = Vec::new();
    let mut sources = HashMap::new();
    let mut previous = Position::default();
    let mut previous_end = 0;
    for (token, kind, modifiers) in tokens {
        if token.pos < previous_end {
            continue;
        }
        let text = sources
            .entry(token.source)
            .or_insert_with(|| Rope::from_str(&token.source.get_text()));
        let end = token.pos + token.length;
        if end as usize > text.len_bytes() {
            continue;
        }
        let range = byte_range(text, token.pos as usize, token.length as usize);
        let start = range.start;
        if start.line != range.end.line || start.character >= range.end.character {
            continue;
        }

        let delta_line = start.line - previous.line;
        data.push(SemanticToken {
            delta_line,
            delta_start: if delta_line == 0 {
                start.character - previous.character
            } else {
                start.character
            },
            length: range.end.character - start.character,
            token_type: kind as u32,
            token_modifiers_bitset: modifiers,
        });
        previous = start;
        previous_end = end;
    }
    data
}

pub mod semantic_legend {
    use super::*;

    #[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
    pub(super) enum TokenType {
        Property,
        Function,
        Struct,
        Type,
        Enum,
        EnumMember,
        Variable,
    }

    pub(super) const DECLARATION: u32 = 1 << 0;
    pub(super) const READONLY: u32 = 1 << 1;

    pub fn get_token_types() -> Vec<SemanticTokenType> {
        vec![
            SemanticTokenType::PROPERTY,
            SemanticTokenType::FUNCTION,
            SemanticTokenType::STRUCT,
            SemanticTokenType::TYPE,
            SemanticTokenType::ENUM,
            SemanticTokenType::ENUM_MEMBER,
            SemanticTokenType::VARIABLE,
        ]
    }

    pub fn get_token_modifiers() -> Vec<SemanticTokenModifier> {
        vec![
            SemanticTokenModifier::DECLARATION,
            SemanticTokenModifier::READONLY,
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use veryl_metadata::component_manifest::{
        ManifestGroup, ManifestMember, ManifestMethod, ManifestParam, ManifestPort,
    };

    #[test]
    fn semantic_tokens_include_declarations_and_cross_file_references() {
        use semantic_legend::{DECLARATION, READONLY, TokenType};

        let definitions = "package Types {
    enum State { Idle, Busy, }
    struct Frame { state: State, }
    type Word = logic<8>;
    const WIDTH: u32 = 8;
    function idle() -> State { return State::Idle; }
}
";
        let uses = "module Top (out: output logic) {
    /* 🦀 */ let _state: Types::State = Types::State::Idle;
    var _frame: Types::Frame;
    var _word: Types::Word;
    let _call: Types::State = Types::idle();
    let _width: u32 = Types::WIDTH;
    assign out = 0;
}
";
        let metadata = Metadata::create_default("semantic_test").unwrap();
        let analyzer = Analyzer::new(&metadata);
        for (path, text) in [("types.veryl", definitions), ("top.veryl", uses)] {
            let parser = Parser::parse(text, &path).unwrap();
            let errors = analyzer.analyze_pass1("semantic_test", &parser.veryl);
            assert!(errors.is_empty(), "{errors:?}");
        }
        let errors = Analyzer::analyze_post_pass1();
        assert!(errors.is_empty(), "{errors:?}");

        fn decode(path: &str, source: &str) -> Vec<(String, u32, u32)> {
            let lines: Vec<Vec<u16>> = source
                .lines()
                .map(|line| line.encode_utf16().collect())
                .collect();
            let tokens = collect_semantic_tokens(
                resource_table::get_path_id(PathBuf::from(path)).unwrap(),
                None,
            );
            let mut line = 0;
            let mut column = 0;
            let mut previous_end = 0;
            tokens
                .into_iter()
                .map(|token| {
                    line += token.delta_line;
                    if token.delta_line == 0 {
                        column += token.delta_start;
                    } else {
                        column = token.delta_start;
                        previous_end = 0;
                    }
                    assert!(column >= previous_end && token.length != 0);
                    previous_end = column + token.length;
                    let name = String::from_utf16(
                        &lines[line as usize][column as usize..previous_end as usize],
                    )
                    .unwrap();
                    (name, token.token_type, token.token_modifiers_bitset)
                })
                .collect()
        }

        let tokens = decode("types.veryl", definitions);
        for (name, kind, modifiers) in [
            ("State", TokenType::Enum, DECLARATION),
            ("Idle", TokenType::EnumMember, DECLARATION),
            ("Frame", TokenType::Struct, DECLARATION),
            ("Word", TokenType::Type, DECLARATION),
            ("WIDTH", TokenType::Variable, DECLARATION | READONLY),
            ("idle", TokenType::Function, DECLARATION),
        ] {
            assert!(
                tokens.contains(&(name.into(), kind as u32, modifiers)),
                "{tokens:?}"
            );
        }

        let expected = [
            ("out", TokenType::Property, DECLARATION),
            ("State", TokenType::Enum, 0),
            ("State", TokenType::Enum, 0),
            ("Idle", TokenType::EnumMember, 0),
            ("Frame", TokenType::Struct, 0),
            ("Word", TokenType::Type, 0),
            ("State", TokenType::Enum, 0),
            ("idle", TokenType::Function, 0),
            ("WIDTH", TokenType::Variable, READONLY),
            ("out", TokenType::Property, 0),
        ]
        .map(|(name, kind, modifiers)| (name.into(), kind as u32, modifiers));
        assert_eq!(decode("top.veryl", uses), expected);
    }

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

    fn edit_range(line_text: &str, insert_text: &str) -> (u32, u32, String) {
        let mut items = vec![CompletionItem {
            label: insert_text.to_string(),
            insert_text: Some(insert_text.to_string()),
            ..Default::default()
        }];
        let column = line_text.chars().count() + 1;
        set_completion_edit_ranges(&mut items, line_text, 3, column);
        let Some(CompletionTextEdit::Edit(edit)) = items[0].text_edit.take() else {
            panic!("expected a text edit");
        };
        assert_eq!(items[0].insert_text, None);
        assert_eq!(edit.range.start.line, 2);
        (
            edit.range.start.character,
            edit.range.end.character,
            edit.new_text,
        )
    }

    #[test]
    fn completion_replaces_typed_dollar_prefix() {
        assert_eq!(
            edit_range("    a = $cl", "$clog2"),
            (8, 11, "$clog2".to_string())
        );
        assert_eq!(
            edit_range("    inst c: $tb::c", "$tb::clock_gen"),
            (12, 18, "$tb::clock_gen".to_string())
        );
    }

    #[test]
    fn completion_falls_back_to_the_typed_word() {
        assert_eq!(
            edit_range("    x = pkg::fo", "foo"),
            (13, 15, "foo".to_string())
        );
        assert_eq!(
            edit_range("    x = clk", "clock"),
            (8, 11, "clock".to_string())
        );
        assert_eq!(
            edit_range("    x = cl", "$clog2"),
            (8, 10, "$clog2".to_string())
        );
        assert_eq!(
            edit_range("    x = ", "module"),
            (8, 8, "module".to_string())
        );
    }

    #[test]
    fn metadata_error_names_the_bad_key() {
        let err = "[project]
name = \"t\"
version = \"0.1.0\"

[test]
no_such_option = true
"
        .parse::<Metadata>()
        .unwrap_err();
        let message = error_chain(&err);
        assert!(message.starts_with("toml load failed: "), "{message}");
        assert!(message.contains("line 6, column 1"), "{message}");
        assert!(message.contains("no_such_option"), "{message}");
        assert!(!message.contains("version = "), "{message}");
    }

    #[test]
    fn metadata_error_includes_io_cause() {
        let err = MetadataError::file_io(
            std::io::Error::new(std::io::ErrorKind::PermissionDenied, "permission denied"),
            &PathBuf::from("/prj/Veryl.toml"),
        );
        assert_eq!(
            error_chain(&err),
            "file I/O error (/prj/Veryl.toml): permission denied"
        );
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

        let key = resource_table::insert_str("golden");
        component_manifest_table::insert(key, manifest());

        let token = Token::builtin_text("golden");
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
