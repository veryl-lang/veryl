use handlebars::Handlebars;
use mdbook::{Config, MDBook};
use miette::{IntoDiagnostic, Result};
use serde::Serialize;
use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::Write;
use std::path::PathBuf;
use tempfile::TempDir;
use veryl_analyzer::symbol::{ParameterScope, Symbol, SymbolKind};
use veryl_analyzer::symbol_table;
use veryl_metadata::Metadata;
use veryl_parser::resource_table::{self, StrId};
use veryl_parser::veryl_token::Token;

const SUMMARY_TMPL: &str = r###"
# Summary

[{{name}}](index.md)

- [Modules](modules.md)
  {{#each modules}}
  - [{{this}}]({{this}}.md)
  {{/each}}

- [Interfaces](interfaces.md)
  {{#each interfaces}}
  - [{{this}}]({{this}}.md)
  {{/each}}

- [Packages](packages.md)
  {{#each packages}}
  - [{{this}}]({{this}}.md)
  {{/each}}
"###;

#[derive(Serialize)]
struct SummaryData {
    name: String,
    modules: Vec<String>,
    interfaces: Vec<String>,
    packages: Vec<String>,
}

const INDEX_TMPL: &str = r###"
# {{name}}

## Project

{{description}}

* Version: {{version}}
{{#if repository}}
* Repository: [{{repository}}]({{repository}})
{{/if}}

{{{{raw}}}}
{{#include modules.md}}
{{#include interfaces.md}}
{{#include packages.md}}
{{{{/raw}}}}
"###;

#[derive(Serialize)]
struct IndexData {
    name: String,
    description: Option<String>,
    version: String,
    repository: Option<String>,
}

const LIST_TMPL: &str = r###"
## {{name}}

{{#each items}}
[{{this.name}}]({{this.name}}.md) {{this.description}}

{{/each}}
"###;

#[derive(Serialize)]
struct ListData {
    name: String,
    items: Vec<ListItem>,
}

#[derive(Serialize)]
struct ListItem {
    name: String,
    description: String,
}

const MODULE_TMPL: &str = r#"
## {{name}}

{{description}}

### Parameters
---

{{#each parameters}}
#### {{this.name}}: <span class="hljs-type">{{this.typ}}</span>
{{this.description}}

{{/each}}

### Ports
---

{{#each ports}}
#### {{this.name}}: <span class="hljs-keyword">{{this.direction}}</span> <span class="hljs-type">{{this.typ}}</span>
{{this.description}}

{{/each}}
"#;

#[derive(Serialize)]
struct ModuleData {
    name: String,
    description: String,
    parameters: Vec<ParameterData>,
    ports: Vec<PortData>,
}

#[derive(Serialize)]
struct ParameterData {
    name: String,
    typ: String,
    description: Option<String>,
}

#[derive(Serialize)]
struct PortData {
    name: String,
    direction: String,
    typ: Option<String>,
    description: Option<String>,
}

const INTERFACE_TMPL: &str = r#"
## {{name}}

{{description}}

### Parameters
---

{{#each parameters}}
#### {{this.name}}: <span class="hljs-type">{{this.typ}}</span>
{{this.description}}

{{/each}}
"#;

#[derive(Serialize)]
struct InterfaceData {
    name: String,
    description: String,
    parameters: Vec<ParameterData>,
}

const PACKAGE_TMPL: &str = r###"
## {{name}}

{{description}}

"###;

#[derive(Serialize)]
struct PackageData {
    name: String,
    description: String,
}

pub struct DocBuilder {
    metadata: Metadata,
    #[allow(dead_code)]
    temp_dir: TempDir,
    root_dir: PathBuf,
    src_dir: PathBuf,
    modules: BTreeMap<String, Symbol>,
    interfaces: BTreeMap<String, Symbol>,
    packages: BTreeMap<String, Symbol>,
}

impl DocBuilder {
    pub fn new(
        metadata: &Metadata,
        modules: BTreeMap<String, Symbol>,
        interfaces: BTreeMap<String, Symbol>,
        packages: BTreeMap<String, Symbol>,
    ) -> Result<Self> {
        let temp_dir = tempfile::tempdir().into_diagnostic()?;
        let root_dir = temp_dir.path().to_path_buf();
        let src_dir = temp_dir.path().join("src");
        fs::create_dir(&src_dir).into_diagnostic()?;

        Ok(Self {
            metadata: metadata.clone(),
            temp_dir,
            root_dir,
            src_dir,
            modules,
            interfaces,
            packages,
        })
    }

    pub fn build(&self) -> Result<()> {
        self.build_component("SUMMARY.md", self.build_summary())?;
        self.build_component("index.md", self.build_index())?;
        self.build_component("modules.md", self.build_modules())?;
        self.build_component("interfaces.md", self.build_interfaces())?;
        self.build_component("packages.md", self.build_packages())?;

        for (k, v) in &self.modules {
            let file = format!("{}.md", k);
            self.build_component(&file, self.build_module(k, v))?;
        }

        for (k, v) in &self.interfaces {
            let file = format!("{}.md", k);
            self.build_component(&file, self.build_interface(k, v))?;
        }

        for (k, v) in &self.packages {
            let file = format!("{}.md", k);
            self.build_component(&file, self.build_package(k, v))?;
        }

        let mut cfg = Config::default();
        cfg.build.build_dir = self.metadata.metadata_path.parent().unwrap().join("doc");
        cfg.set("output.html.no-section-label", true).unwrap();
        cfg.set("output.html.fold.enable", true).unwrap();
        cfg.set("output.html.fold.level", 0).unwrap();

        let md = MDBook::load_with_config(&self.root_dir, cfg).unwrap();
        md.build().unwrap();
        Ok(())
    }

    fn build_component(&self, name: &str, content: String) -> Result<()> {
        let file = self.src_dir.join(name);
        let mut file = File::create(file).into_diagnostic()?;
        write!(file, "{}", content).into_diagnostic()?;
        Ok(())
    }

    fn build_summary(&self) -> String {
        let modules: Vec<_> = self.modules.keys().cloned().collect();
        let interfaces: Vec<_> = self.interfaces.keys().cloned().collect();
        let packages: Vec<_> = self.packages.keys().cloned().collect();
        let data = SummaryData {
            name: self.metadata.project.name.clone(),
            modules,
            interfaces,
            packages,
        };

        let handlebars = Handlebars::new();
        handlebars.render_template(SUMMARY_TMPL, &data).unwrap()
    }

    fn build_index(&self) -> String {
        let data = IndexData {
            name: self.metadata.project.name.clone(),
            version: format!("{}", self.metadata.project.version),
            description: self.metadata.project.description.clone(),
            repository: self.metadata.project.repository.clone(),
        };

        let handlebars = Handlebars::new();
        handlebars.render_template(INDEX_TMPL, &data).unwrap()
    }

    fn build_modules(&self) -> String {
        let items: Vec<_> = self
            .modules
            .iter()
            .map(|(k, v)| ListItem {
                name: k.clone(),
                description: format_doc_comment(&v.doc_comment, true),
            })
            .collect();

        let data = ListData {
            name: "Modules".to_string(),
            items,
        };

        let handlebars = Handlebars::new();
        handlebars.render_template(LIST_TMPL, &data).unwrap()
    }

    fn build_interfaces(&self) -> String {
        let items: Vec<_> = self
            .interfaces
            .iter()
            .map(|(k, v)| ListItem {
                name: k.clone(),
                description: format_doc_comment(&v.doc_comment, true),
            })
            .collect();

        let data = ListData {
            name: "Interfaces".to_string(),
            items,
        };

        let handlebars = Handlebars::new();
        handlebars.render_template(LIST_TMPL, &data).unwrap()
    }

    fn build_packages(&self) -> String {
        let items: Vec<_> = self
            .packages
            .iter()
            .map(|(k, v)| ListItem {
                name: k.clone(),
                description: format_doc_comment(&v.doc_comment, true),
            })
            .collect();

        let data = ListData {
            name: "Packages".to_string(),
            items,
        };

        let handlebars = Handlebars::new();
        handlebars.render_template(LIST_TMPL, &data).unwrap()
    }

    fn build_module(&self, name: &str, symbol: &Symbol) -> String {
        if let SymbolKind::Module(property) = &symbol.kind {
            let parameters: Vec<_> = property
                .parameters
                .iter()
                .filter(|x| matches!(x.property.scope, ParameterScope::Global,))
                .map(|x| ParameterData {
                    name: resource_table::get_str_value(x.name).unwrap(),
                    typ: format!("{}", x.property.r#type),
                    description: get_comment_from_token(&x.property.token),
                })
                .collect();

            let ports: Vec<_> = property
                .ports
                .iter()
                .map(|x| PortData {
                    name: resource_table::get_str_value(x.name).unwrap(),
                    direction: format!("{}", x.property.direction),
                    typ: x.property.r#type.as_ref().map(|x| format!("{}", x)),
                    description: get_comment_from_token(&x.property.token),
                })
                .collect();

            let data = ModuleData {
                name: name.to_string(),
                description: format_doc_comment(&symbol.doc_comment, false),
                parameters,
                ports,
            };

            let handlebars = Handlebars::new();
            handlebars.render_template(MODULE_TMPL, &data).unwrap()
        } else {
            String::new()
        }
    }

    fn build_interface(&self, name: &str, symbol: &Symbol) -> String {
        if let SymbolKind::Interface(property) = &symbol.kind {
            let parameters: Vec<_> = property
                .parameters
                .iter()
                .filter(|x| matches!(x.property.scope, ParameterScope::Global,))
                .map(|x| ParameterData {
                    name: resource_table::get_str_value(x.name).unwrap(),
                    typ: format!("{}", x.property.r#type),
                    description: get_comment_from_token(&x.property.token),
                })
                .collect();

            let data = InterfaceData {
                name: name.to_string(),
                description: format_doc_comment(&symbol.doc_comment, false),
                parameters,
            };

            let handlebars = Handlebars::new();
            handlebars.render_template(INTERFACE_TMPL, &data).unwrap()
        } else {
            String::new()
        }
    }

    fn build_package(&self, name: &str, symbol: &Symbol) -> String {
        if let SymbolKind::Package = &symbol.kind {
            let data = PackageData {
                name: name.to_string(),
                description: format_doc_comment(&symbol.doc_comment, false),
            };

            let handlebars = Handlebars::new();
            handlebars.render_template(PACKAGE_TMPL, &data).unwrap()
        } else {
            String::new()
        }
    }
}

fn format_doc_comment(text: &[StrId], single_line: bool) -> String {
    let mut ret = String::new();
    for t in text {
        let t = format!("{}", t);
        let t = t.trim_start_matches("///");
        ret.push_str(t);
        if single_line {
            break;
        }
    }
    ret
}

fn get_comment_from_token(token: &Token) -> Option<String> {
    if let Ok(symbol) = symbol_table::resolve(token) {
        if let Some(symbol) = symbol.found {
            Some(format_doc_comment(&symbol.doc_comment, false))
        } else {
            None
        }
    } else {
        None
    }
}
