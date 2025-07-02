use crate::cmd_build::CmdBuild;
use crate::runner::{Cocotb, CocotbSource, Vcs, Verilator, Vivado};
use crate::{OptBuild, OptTest};
use log::{error, info};
use miette::Result;
use veryl_analyzer::namespace::Namespace;
use veryl_analyzer::symbol::{SymbolKind, TestType};
use veryl_analyzer::symbol_path::GenericSymbolPath;
use veryl_analyzer::symbol_table;
use veryl_metadata::{FilelistType, Metadata, SimType};
use veryl_parser::veryl_grammar_trait::{self as syntax_tree};

pub struct CmdTest {
    opt: OptTest,
}

impl CmdTest {
    pub fn new(opt: OptTest) -> Self {
        Self { opt }
    }

    pub fn exec(&self, metadata: &mut Metadata) -> Result<bool> {
        // force filelist_type to absolute which can be refered from temporary directory
        metadata.build.filelist_type = FilelistType::Absolute;

        let build = CmdBuild::new(OptBuild {
            files: self.opt.files.clone(),
            check: false,
        });
        build.exec(metadata, true, false)?;

        let tests: Vec<_> = symbol_table::get_all()
            .into_iter()
            .filter_map(|symbol| {
                if symbol.namespace.to_string() == metadata.project.name {
                    if let SymbolKind::Test(x) = symbol.kind {
                        Some((symbol.token.text, x))
                    } else {
                        None
                    }
                } else {
                    None
                }
            })
            .collect();

        let sim_type = if let Some(x) = self.opt.sim {
            x.into()
        } else {
            metadata.test.simulator
        };

        let mut success = 0;
        let mut failure = 0;
        for (test, property) in &tests {
            let mut runner = match &property.r#type {
                TestType::Inline => match sim_type {
                    SimType::Verilator => Verilator::new().runner(),
                    SimType::Vcs => Vcs::new().runner(),
                    SimType::Vivado => Vivado::new().runner(),
                },
                TestType::CocotbEmbed(content, namespace) => {
                    let content_text = eval_embed_content(content, namespace);
                    Cocotb::new(CocotbSource::Embed(content_text)).runner()
                }
                TestType::CocotbInclude(x) => Cocotb::new(CocotbSource::Include(*x)).runner(),
            };

            if runner.run(metadata, *test, property.top, property.path, self.opt.wave)? {
                success += 1;
            } else {
                failure += 1;
            }
        }

        if failure == 0 {
            info!("Completed tests : {success} passed, {failure} failed");
            Ok(true)
        } else {
            error!("Completed tests : {success} passed, {failure} failed");
            Ok(false)
        }
    }
}

fn eval_embed_content(content: &syntax_tree::EmbedContent, namespace: &Namespace) -> String {
    let mut ret = "".to_string();

    for x in &content.embed_content_list {
        ret.push_str(&eval_embed_item(&x.embed_item, namespace));
    }

    ret
}

fn eval_embed_item(arg: &syntax_tree::EmbedItem, namespace: &Namespace) -> String {
    match arg {
        syntax_tree::EmbedItem::EmbedIdentifier(x) => {
            eval_enbed_identifier(&x.embed_identifier, namespace)
        }
        syntax_tree::EmbedItem::EscapedBackslash(_) => "\\".to_string(),
        syntax_tree::EmbedItem::EscapedChar(x) => x.escaped_char.escaped_char_token.to_string(),
        syntax_tree::EmbedItem::BracedEmbedItem(x) => {
            eval_braced_embed_item(&x.braced_embed_item, namespace)
        }
        syntax_tree::EmbedItem::ParenedEmbedItem(x) => {
            eval_parened_embed_item(&x.parened_embed_item, namespace)
        }
        syntax_tree::EmbedItem::CodeSnippet(x) => x.code_snippet.code_snippet_token.to_string(),
    }
}

fn eval_enbed_identifier(arg: &syntax_tree::EmbedIdentifier, namespace: &Namespace) -> String {
    let path: GenericSymbolPath = arg.scoped_identifier.as_ref().into();
    let (Ok(symbol), _) = path.resolve_path(namespace, None) else {
        unreachable!()
    };
    symbol.found.token.to_string()
}

fn eval_braced_embed_item(arg: &syntax_tree::BracedEmbedItem, namespace: &Namespace) -> String {
    let mut ret = "".to_string();

    ret.push_str(&arg.embed_l_brace.embed_l_brace_token.to_string());
    for x in &arg.braced_embed_item_list {
        ret.push_str(&eval_embed_item(&x.embed_item, namespace));
    }
    ret.push_str(&arg.embed_r_brace.embed_r_brace_token.to_string());

    ret
}

fn eval_parened_embed_item(arg: &syntax_tree::ParenedEmbedItem, namespace: &Namespace) -> String {
    let mut ret = "".to_string();

    ret.push_str(&arg.embed_l_paren.embed_l_paren_token.to_string());
    for x in &arg.parened_embed_item_list {
        ret.push_str(&eval_embed_item(&x.embed_item, namespace));
    }
    ret.push_str(&arg.embed_r_paren.embed_r_paren_token.to_string());

    ret
}
