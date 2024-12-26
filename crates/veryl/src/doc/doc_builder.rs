use crate::doc::{Mermaid, Wavedrom};
use handlebars::Handlebars;
use mdbook::{Config, MDBook};
use miette::{IntoDiagnostic, Result};
use serde::Serialize;
use std::collections::HashSet;
use std::fs::{self, File};
use std::io::Write;
use std::path::PathBuf;
use tempfile::TempDir;
use veryl_analyzer::symbol::{ClockDomain, ParameterKind, Symbol, SymbolKind};
use veryl_analyzer::symbol_table;
use veryl_metadata::Metadata;
use veryl_parser::veryl_token::Token;

const SUMMARY_TMPL: &str = r###"
# Summary

[{{name}}](index.md)
- [{{version}}]()

---

- [Modules](modules.md)
  {{#each modules}}
  - [{{this.0}}]({{this.1}}.md)
  {{/each}}

- [Module Prototypes](proto_modules.md)
  {{#each proto_modules}}
  - [{{this.0}}]({{this.1}}.md)
  {{/each}}

- [Interfaces](interfaces.md)
  {{#each interfaces}}
  - [{{this.0}}]({{this.1}}.md)
  {{/each}}

- [Packages](packages.md)
  {{#each packages}}
  - [{{this.0}}]({{this.1}}.md)
  {{/each}}
"###;

#[derive(Serialize)]
struct SummaryData {
    name: String,
    version: String,
    modules: Vec<(String, String)>,
    proto_modules: Vec<(String, String)>,
    interfaces: Vec<(String, String)>,
    packages: Vec<(String, String)>,
}

const INDEX_TMPL: &str = r###"
# {{name}}

{{description}}

<table align="center" class="table_list">
<tbody>
<tr>
    <th class="table_list_item">Version</th>
    <td class="table_list_item">{{version}}</td>
</tr>
{{#if repository}}
<tr>
    <th class="table_list_item">Repository</th>
    <td class="table_list_item"><a href="{{repository}}">{{repository}}</a></td>
</tr>
{{/if}}
{{#if license}}
<tr>
    <th class="table_list_item">License</th>
    <td class="table_list_item">{{license}}</td>
</tr>
{{/if}}
</tbody>
</table>

{{{{raw}}}}
{{#include modules.md}}
{{#include proto_modules.md}}
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
    license: Option<String>,
}

const LIST_TMPL: &str = r###"
## {{name}}
---

<table class="table_list">
<tbody>
{{#each items}}
<tr>
    <th class="table_list_item"><a href="{{this.file_name}}.html">{{this.html_name}}</a></th>
    <td class="table_list_item">{{this.description}}</td>
</tr>
{{/each}}
</tbody>
</table>

"###;

#[derive(Serialize)]
struct ListData {
    name: String,
    items: Vec<ListItem>,
}

#[derive(Serialize)]
struct ListItem {
    file_name: String,
    html_name: String,
    description: String,
}

const MODULE_TMPL: &str = r#"
## {{name}}

{{description}}

{{#if generic_parameters}}
### Generic Parameters
---

<table class="table_list">
<tbody>
{{#each generic_parameters}}
<tr>
    <th class="table_list_item">{{this.name}}</th>
    <td class="table_list_item"><span class="hljs-type">{{this.bound}}</span></td>
</tr>
{{/each}}
</tbody>
</table>
{{/if}}

{{#if parameters}}
### Parameters
---

<table class="table_list">
<tbody>
{{#each parameters}}
<tr>
    <th class="table_list_item">{{this.name}}</th>
    <td class="table_list_item"><span class="hljs-type">{{this.typ}}</span></td>
    <td class="table_list_item">{{this.description}}</td>
</tr>
{{/each}}
</tbody>
</table>
{{/if}}

{{#if clock_domains}}
### Clock Domains
---

<table class="table_list">
<tbody>
{{#each clock_domains}}
<tr>
    <th class="table_list_item">{{this}}</th>
</tr>
{{/each}}
</tbody>
</table>
{{/if}}

{{#if ports}}
### Ports
---

<table class="table_list">
<tbody>
{{#each ports}}
<tr>
    <th class="table_list_item">{{this.name}}</th>
    <td class="table_list_item"><span class="hljs-keyword">{{this.direction}}</span></td>
    {{#if ../clock_domains}}
    <td class="table_list_item"><span class="hljs-attribute">{{this.clock_domain}}</span></td>
    {{/if}}
    <td class="table_list_item"><span class="hljs-type">{{this.typ}}</span></td>
    <td class="table_list_item">{{this.description}}</td>
</tr>
{{/each}}
</tbody>
</table>
{{/if}}
"#;

#[derive(Serialize)]
struct ModuleData {
    name: String,
    description: String,
    generic_parameters: Vec<GenericParameterData>,
    parameters: Vec<ParameterData>,
    clock_domains: Vec<String>,
    ports: Vec<PortData>,
}

#[derive(Serialize)]
struct GenericParameterData {
    name: String,
    bound: String,
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
    clock_domain: Option<String>,
    typ: Option<String>,
    description: Option<String>,
}

const PROTO_MODULE_TMPL: &str = r#"
## {{name}}

{{description}}

{{#if parameters}}
### Parameters
---

<table class="table_list">
<tbody>
{{#each parameters}}
<tr>
    <th class="table_list_item">{{this.name}}</th>
    <td class="table_list_item"><span class="hljs-type">{{this.typ}}</span></td>
    <td class="table_list_item">{{this.description}}</td>
</tr>
{{/each}}
</tbody>
</table>
{{/if}}

{{#if clock_domains}}
### Clock Domains
---

<table class="table_list">
<tbody>
{{#each clock_domains}}
<tr>
    <th class="table_list_item">{{this}}</th>
</tr>
{{/each}}
</tbody>
</table>
{{/if}}

{{#if ports}}
### Ports
---

<table class="table_list">
<tbody>
{{#each ports}}
<tr>
    <th class="table_list_item">{{this.name}}</th>
    <td class="table_list_item"><span class="hljs-keyword">{{this.direction}}</span></td>
    {{#if ../clock_domains}}
    <td class="table_list_item"><span class="hljs-attribute">{{this.clock_domain}}</span></td>
    {{/if}}
    <td class="table_list_item"><span class="hljs-type">{{this.typ}}</span></td>
    <td class="table_list_item">{{this.description}}</td>
</tr>
{{/each}}
</tbody>
</table>
{{/if}}
"#;

#[derive(Serialize)]
struct ProtoModuleData {
    name: String,
    description: String,
    parameters: Vec<ParameterData>,
    clock_domains: Vec<String>,
    ports: Vec<PortData>,
}

const INTERFACE_TMPL: &str = r#"
## {{name}}

{{description}}

{{#if parameters}}
### Parameters
---

<table class="table_list">
<tbody>
{{#each parameters}}
<tr>
    <th class="table_list_item">{{this.name}}</th>
    <td class="table_list_item"><span class="hljs-type">{{this.typ}}</span></td>
    <td class="table_list_item">{{this.description}}</td>
</tr>
{{/each}}
</tbody>
</table>
{{/if}}
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
    theme_dir: PathBuf,
    modules: Vec<TopLevelItem>,
    proto_modules: Vec<TopLevelItem>,
    interfaces: Vec<TopLevelItem>,
    packages: Vec<TopLevelItem>,
}

#[derive(Clone)]
pub struct TopLevelItem {
    pub file_name: String,
    pub html_name: String,
    pub symbol: Symbol,
}

impl DocBuilder {
    pub fn new(
        metadata: &Metadata,
        modules: Vec<TopLevelItem>,
        proto_modules: Vec<TopLevelItem>,
        interfaces: Vec<TopLevelItem>,
        packages: Vec<TopLevelItem>,
    ) -> Result<Self> {
        let temp_dir = tempfile::tempdir().into_diagnostic()?;
        let root_dir = temp_dir.path().to_path_buf();
        let src_dir = temp_dir.path().join("src");
        let theme_dir = temp_dir.path().join("theme");
        fs::create_dir(&src_dir).into_diagnostic()?;
        fs::create_dir(&theme_dir).into_diagnostic()?;

        Ok(Self {
            metadata: metadata.clone(),
            temp_dir,
            root_dir,
            src_dir,
            theme_dir,
            modules,
            proto_modules,
            interfaces,
            packages,
        })
    }

    pub fn build(&self) -> Result<()> {
        self.build_theme()?;

        self.build_component("SUMMARY.md", self.build_summary())?;
        self.build_component("index.md", self.build_index())?;
        self.build_component("modules.md", self.build_modules())?;
        self.build_component("proto_modules.md", self.build_proto_modules())?;
        self.build_component("interfaces.md", self.build_interfaces())?;
        self.build_component("packages.md", self.build_packages())?;

        for x in &self.modules {
            let file = format!("{}.md", x.file_name);
            self.build_component(&file, self.build_module(&x.html_name, &x.symbol))?;
        }

        for x in &self.proto_modules {
            let file = format!("{}.md", x.file_name);
            self.build_component(&file, self.build_proto_module(&x.html_name, &x.symbol))?;
        }

        for x in &self.interfaces {
            let file = format!("{}.md", x.file_name);
            self.build_component(&file, self.build_interface(&x.html_name, &x.symbol))?;
        }

        for x in &self.packages {
            let file = format!("{}.md", x.file_name);
            self.build_component(&file, self.build_package(&x.html_name, &x.symbol))?;
        }

        let mut cfg = Config::default();
        cfg.build.build_dir = self.metadata.doc_path();
        cfg.set("output.html.no-section-label", true).unwrap();
        cfg.set("output.html.fold.enable", true).unwrap();
        cfg.set("output.html.fold.level", 1).unwrap();
        cfg.set("output.html.additional-css", vec!["theme/custom.css"])
            .unwrap();
        cfg.set(
            "output.html.additional-js",
            vec![
                "theme/wavedrom.min.js",
                "theme/wavedrom_skin.js",
                "theme/mermaid.min.js",
            ],
        )
        .unwrap();

        let wavedrom = Wavedrom;
        let mermaid = Mermaid;
        let mut md = MDBook::load_with_config(&self.root_dir, cfg).unwrap();
        md.with_preprocessor(wavedrom);
        md.with_preprocessor(mermaid);
        md.build().unwrap();
        Ok(())
    }

    fn build_theme(&self) -> Result<()> {
        let custom_css = r##"
.affix {
    font-weight: bold;
    font-size: 1.8em;
}

.table_list {
    margin-left: 0;
    margin-right: auto;
}

.table_list_item {
    text-align: left;
    border: unset;
    background-color: var(--bg);
}
        "##;

        let file = self.theme_dir.join("custom.css");
        let mut file = File::create(file).into_diagnostic()?;
        write!(file, "{}", custom_css).into_diagnostic()?;

        let favicon = include_bytes!("../../resource/favicon.png");
        let file = self.theme_dir.join("favicon.png");
        let mut file = File::create(file).into_diagnostic()?;
        file.write(favicon).into_diagnostic()?;

        let wavedrom = include_bytes!("../../resource/wavedrom/wavedrom.min.js");
        let file = self.theme_dir.join("wavedrom.min.js");
        let mut file = File::create(file).into_diagnostic()?;
        file.write(wavedrom).into_diagnostic()?;

        let wavedrom_skin = include_bytes!("../../resource/wavedrom/skins/default.js");
        let file = self.theme_dir.join("wavedrom_skin.js");
        let mut file = File::create(file).into_diagnostic()?;
        file.write(wavedrom_skin).into_diagnostic()?;

        let mermaid = include_bytes!("../../resource/mermaid/mermaid.min.js");
        let file = self.theme_dir.join("mermaid.min.js");
        let mut file = File::create(file).into_diagnostic()?;
        file.write(mermaid).into_diagnostic()?;

        Ok(())
    }

    fn build_component(&self, name: &str, content: String) -> Result<()> {
        let file = self.src_dir.join(name);
        let mut file = File::create(file).into_diagnostic()?;
        write!(file, "{}", content).into_diagnostic()?;
        Ok(())
    }

    fn build_summary(&self) -> String {
        let modules: Vec<_> = self
            .modules
            .iter()
            .cloned()
            .map(|x| (x.html_name, x.file_name))
            .collect();
        let proto_modules: Vec<_> = self
            .proto_modules
            .iter()
            .cloned()
            .map(|x| (x.html_name, x.file_name))
            .collect();
        let interfaces: Vec<_> = self
            .interfaces
            .iter()
            .cloned()
            .map(|x| (x.html_name, x.file_name))
            .collect();
        let packages: Vec<_> = self
            .packages
            .iter()
            .cloned()
            .map(|x| (x.html_name, x.file_name))
            .collect();
        let data = SummaryData {
            name: self.metadata.project.name.clone(),
            version: format!("{}", self.metadata.project.version),
            modules,
            proto_modules,
            interfaces,
            packages,
        };

        let mut handlebars = Handlebars::new();
        handlebars.register_escape_fn(handlebars::no_escape);
        handlebars.render_template(SUMMARY_TMPL, &data).unwrap()
    }

    fn build_index(&self) -> String {
        let data = IndexData {
            name: self.metadata.project.name.clone(),
            version: format!("{}", self.metadata.project.version),
            description: self.metadata.project.description.clone(),
            repository: self.metadata.project.repository.clone(),
            license: self.metadata.project.license.clone(),
        };

        let mut handlebars = Handlebars::new();
        handlebars.register_escape_fn(handlebars::no_escape);
        handlebars.render_template(INDEX_TMPL, &data).unwrap()
    }

    fn build_modules(&self) -> String {
        let items: Vec<_> = self
            .modules
            .iter()
            .map(|x| ListItem {
                file_name: x.file_name.clone(),
                html_name: x.html_name.clone(),
                description: x.symbol.doc_comment.format(true),
            })
            .collect();

        let data = ListData {
            name: "Modules".to_string(),
            items,
        };

        let mut handlebars = Handlebars::new();
        handlebars.register_escape_fn(handlebars::no_escape);
        handlebars.render_template(LIST_TMPL, &data).unwrap()
    }

    fn build_proto_modules(&self) -> String {
        let items: Vec<_> = self
            .proto_modules
            .iter()
            .map(|x| ListItem {
                file_name: x.file_name.clone(),
                html_name: x.html_name.clone(),
                description: x.symbol.doc_comment.format(true),
            })
            .collect();

        let data = ListData {
            name: "Module Prototypes".to_string(),
            items,
        };

        let mut handlebars = Handlebars::new();
        handlebars.register_escape_fn(handlebars::no_escape);
        handlebars.render_template(LIST_TMPL, &data).unwrap()
    }

    fn build_interfaces(&self) -> String {
        let items: Vec<_> = self
            .interfaces
            .iter()
            .map(|x| ListItem {
                file_name: x.file_name.clone(),
                html_name: x.html_name.clone(),
                description: x.symbol.doc_comment.format(true),
            })
            .collect();

        let data = ListData {
            name: "Interfaces".to_string(),
            items,
        };

        let mut handlebars = Handlebars::new();
        handlebars.register_escape_fn(handlebars::no_escape);
        handlebars.render_template(LIST_TMPL, &data).unwrap()
    }

    fn build_packages(&self) -> String {
        let items: Vec<_> = self
            .packages
            .iter()
            .map(|x| ListItem {
                file_name: x.file_name.clone(),
                html_name: x.html_name.clone(),
                description: x.symbol.doc_comment.format(true),
            })
            .collect();

        let data = ListData {
            name: "Packages".to_string(),
            items,
        };

        let mut handlebars = Handlebars::new();
        handlebars.register_escape_fn(handlebars::no_escape);
        handlebars.render_template(LIST_TMPL, &data).unwrap()
    }

    fn build_module(&self, name: &str, symbol: &Symbol) -> String {
        if let SymbolKind::Module(property) = &symbol.kind {
            let generic_parameters: Vec<_> = property
                .generic_parameters
                .iter()
                .filter_map(|x| {
                    let symbol = symbol_table::get(*x).unwrap();
                    if let SymbolKind::GenericParameter(x) = symbol.kind {
                        Some(GenericParameterData {
                            name: symbol.token.text.to_string(),
                            bound: x.bound.to_string(),
                        })
                    } else {
                        None
                    }
                })
                .collect();

            let parameters: Vec<_> = property
                .parameters
                .iter()
                .filter(|x| matches!(x.property().kind, ParameterKind::Param,))
                .map(|x| ParameterData {
                    name: x.name.to_string(),
                    typ: format!("{}", x.property().r#type),
                    description: get_comment_from_token(&x.property().token),
                })
                .collect();

            let clock_domains: HashSet<_> = property
                .ports
                .iter()
                .filter_map(|x| {
                    if let ClockDomain::Explicit(_) = x.property().clock_domain {
                        Some(x.property().clock_domain.to_string())
                    } else {
                        None
                    }
                })
                .collect();
            let mut clock_domains: Vec<_> = clock_domains.into_iter().collect();
            clock_domains.sort();

            let ports: Vec<_> = property
                .ports
                .iter()
                .map(|x| {
                    let clock_domain = if let ClockDomain::Explicit(_) = x.property().clock_domain {
                        Some(x.property().clock_domain.to_string())
                    } else {
                        None
                    };
                    PortData {
                        name: x.name().to_string(),
                        direction: format!("{}", x.property().direction),
                        clock_domain,
                        typ: x.property().r#type.as_ref().map(|x| format!("{}", x)),
                        description: get_comment_from_token(&x.property().token),
                    }
                })
                .collect();

            let data = ModuleData {
                name: name.to_string(),
                description: symbol.doc_comment.format(false),
                generic_parameters,
                parameters,
                clock_domains,
                ports,
            };

            let mut handlebars = Handlebars::new();
            handlebars.register_escape_fn(handlebars::no_escape);
            handlebars.render_template(MODULE_TMPL, &data).unwrap()
        } else {
            String::new()
        }
    }

    fn build_proto_module(&self, name: &str, symbol: &Symbol) -> String {
        if let SymbolKind::ProtoModule(property) = &symbol.kind {
            let parameters: Vec<_> = property
                .parameters
                .iter()
                .filter(|x| matches!(x.property().kind, ParameterKind::Param,))
                .map(|x| ParameterData {
                    name: x.name.to_string(),
                    typ: format!("{}", x.property().r#type),
                    description: get_comment_from_token(&x.property().token),
                })
                .collect();

            let clock_domains: HashSet<_> = property
                .ports
                .iter()
                .filter_map(|x| {
                    if let ClockDomain::Explicit(_) = x.property().clock_domain {
                        Some(x.property().clock_domain.to_string())
                    } else {
                        None
                    }
                })
                .collect();
            let mut clock_domains: Vec<_> = clock_domains.into_iter().collect();
            clock_domains.sort();

            let ports: Vec<_> = property
                .ports
                .iter()
                .map(|x| {
                    let clock_domain = if let ClockDomain::Explicit(_) = x.property().clock_domain {
                        Some(x.property().clock_domain.to_string())
                    } else {
                        None
                    };
                    PortData {
                        name: x.name().to_string(),
                        direction: format!("{}", x.property().direction),
                        clock_domain,
                        typ: x.property().r#type.as_ref().map(|x| format!("{}", x)),
                        description: get_comment_from_token(&x.property().token),
                    }
                })
                .collect();

            let data = ProtoModuleData {
                name: name.to_string(),
                description: symbol.doc_comment.format(false),
                parameters,
                clock_domains,
                ports,
            };

            let mut handlebars = Handlebars::new();
            handlebars.register_escape_fn(handlebars::no_escape);
            handlebars
                .render_template(PROTO_MODULE_TMPL, &data)
                .unwrap()
        } else {
            String::new()
        }
    }

    fn build_interface(&self, name: &str, symbol: &Symbol) -> String {
        if let SymbolKind::Interface(property) = &symbol.kind {
            let parameters: Vec<_> = property
                .parameters
                .iter()
                .filter(|x| matches!(x.property().kind, ParameterKind::Param,))
                .map(|x| ParameterData {
                    name: x.name.to_string(),
                    typ: format!("{}", x.property().r#type),
                    description: get_comment_from_token(&x.property().token),
                })
                .collect();

            let data = InterfaceData {
                name: name.to_string(),
                description: symbol.doc_comment.format(false),
                parameters,
            };

            let mut handlebars = Handlebars::new();
            handlebars.register_escape_fn(handlebars::no_escape);
            handlebars.render_template(INTERFACE_TMPL, &data).unwrap()
        } else {
            String::new()
        }
    }

    fn build_package(&self, name: &str, symbol: &Symbol) -> String {
        if let SymbolKind::Package(_) = &symbol.kind {
            let data = PackageData {
                name: name.to_string(),
                description: symbol.doc_comment.format(false),
            };

            let mut handlebars = Handlebars::new();
            handlebars.register_escape_fn(handlebars::no_escape);
            handlebars.render_template(PACKAGE_TMPL, &data).unwrap()
        } else {
            String::new()
        }
    }
}

fn get_comment_from_token(token: &Token) -> Option<String> {
    if let Ok(symbol) = symbol_table::resolve(token) {
        Some(symbol.found.doc_comment.format(false))
    } else {
        None
    }
}
