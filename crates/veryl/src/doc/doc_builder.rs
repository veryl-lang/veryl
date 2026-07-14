use crate::doc::{Mermaid, Wavedrom};
use handlebars::Handlebars;
use mdbook_driver::MDBook;
use mdbook_driver::config::Config;
use miette::{IntoDiagnostic, Result};
use serde::Serialize;
use std::collections::HashSet;
use std::fs::{self, File};
use std::io::Write;
use std::path::PathBuf;
use tempfile::TempDir;
use veryl_analyzer::symbol::{ClockDomain, ParameterKind, Symbol, SymbolKind};
use veryl_analyzer::symbol_table;
use veryl_metadata::{ComponentManifest, Metadata, MetadataError};
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

{{#if components}}
- [Components](components.md)
  {{#each components}}
  - [{{this.0}}]({{this.1}}.md)
  {{/each}}
{{/if}}
"###;

#[derive(Serialize)]
struct SummaryData {
    name: String,
    version: String,
    modules: Vec<(String, String)>,
    proto_modules: Vec<(String, String)>,
    interfaces: Vec<(String, String)>,
    packages: Vec<(String, String)>,
    components: Vec<(String, String)>,
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

{{#each catalog}}
<h2>{{this.name}}</h2>

<table class="table_list">
<tbody>
{{#each this.items}}
<tr>
    <th class="table_list_item"><a href="{{this.file_name}}.html">{{this.html_name}}</a></th>
    <td class="table_list_item">{{this.description}}</td>
</tr>
{{/each}}
</tbody>
</table>
{{/each}}
"###;

#[derive(Serialize)]
struct IndexData {
    name: String,
    description: Option<String>,
    version: String,
    repository: Option<String>,
    license: Option<String>,
    catalog: Vec<ListData>,
}

const LIST_TMPL: &str = r###"
# {{name}}
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
# {{name}}

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
    typ: String,
    description: Option<String>,
}

const PROTO_MODULE_TMPL: &str = r#"
# {{name}}

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
# {{name}}

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
# {{name}}

{{description}}

"###;

#[derive(Serialize)]
struct PackageData {
    name: String,
    description: String,
}

const COMPONENT_TMPL: &str = r#"
# {{name}}

{{#if kind}}
<p class="doc_subtitle"><span class="hljs-keyword">{{kind}}</span> component</p>
{{/if}}

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
    <td class="table_list_item">{{#if this.optional}}optional{{else}}required{{/if}}</td>
    <td class="table_list_item">{{this.description}}</td>
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
    <td class="table_list_item"><span class="hljs-type">{{this.width}}</span></td>
    <td class="table_list_item">{{this.description}}</td>
</tr>
{{/each}}
</tbody>
</table>
{{/if}}

{{#if methods}}
### Methods
---

{{#each methods}}
<h4 class="method_sig">{{this.signature}}</h4>
<div class="method_desc">

{{this.description}}

</div>
{{/each}}
{{/if}}

{{#if requires}}
### Requires
---

<table class="table_list">
<tbody>
{{#each requires}}
<tr>
    <th class="table_list_item">{{this}}</th>
</tr>
{{/each}}
</tbody>
</table>
{{/if}}

{{#if usage}}
### Usage
---

<pre><code class="language-veryl">{{usage}}</code></pre>
{{/if}}
"#;

#[derive(Serialize)]
struct ComponentData {
    name: String,
    description: String,
    kind: Option<String>,
    parameters: Vec<ComponentParameterData>,
    ports: Vec<ComponentPortData>,
    methods: Vec<ComponentMethodData>,
    requires: Vec<String>,
    usage: Option<String>,
}

#[derive(Serialize)]
struct ComponentParameterData {
    name: String,
    typ: String,
    optional: bool,
    description: Option<String>,
}

#[derive(Serialize)]
struct ComponentPortData {
    name: String,
    direction: String,
    width: String,
    description: Option<String>,
}

#[derive(Serialize)]
struct ComponentMethodData {
    signature: String,
    description: Option<String>,
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
    components: Vec<ComponentItem>,
}

#[derive(Clone)]
pub struct TopLevelItem {
    pub file_name: String,
    pub html_name: String,
    pub symbol: Symbol,
}

/// A user-defined verification component export of a `[[components]]`
/// package, documented from its interface manifest.
#[derive(Clone)]
pub struct ComponentItem {
    pub name: String,
    pub file_name: String,
    pub manifest: ComponentManifest,
}

impl DocBuilder {
    pub fn new(
        metadata: &Metadata,
        modules: Vec<TopLevelItem>,
        proto_modules: Vec<TopLevelItem>,
        interfaces: Vec<TopLevelItem>,
        packages: Vec<TopLevelItem>,
        components: Vec<ComponentItem>,
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
            components,
        })
    }

    pub fn build(&self) -> Result<()> {
        self.build_theme()?;

        self.build_component("SUMMARY.md", self.build_summary()?)?;
        self.build_component("index.md", self.build_index()?)?;
        self.build_component("modules.md", self.build_modules())?;
        self.build_component("proto_modules.md", self.build_proto_modules())?;
        self.build_component("interfaces.md", self.build_interfaces())?;
        self.build_component("packages.md", self.build_packages())?;
        self.build_component("components.md", self.build_components())?;

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

        for x in &self.components {
            let file = format!("{}.md", x.file_name);
            self.build_component(&file, build_component_page(x))?;
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

.doc_subtitle {
    margin-top: -0.6em;
    opacity: 0.75;
    font-size: 0.9em;
}

.method_sig {
    font-family: var(--mono-font-family, monospace);
    font-size: 1em;
    font-weight: 600;
}

.method_desc {
    margin-left: 1.5em;
}
        "##;

        let file = self.theme_dir.join("custom.css");
        let mut file = File::create(file).into_diagnostic()?;
        write!(file, "{custom_css}").into_diagnostic()?;

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
        write!(file, "{content}").into_diagnostic()?;
        Ok(())
    }

    fn build_summary(&self) -> Result<String> {
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
        let components: Vec<_> = self
            .components
            .iter()
            .cloned()
            .map(|x| (x.name, x.file_name))
            .collect();
        let data = SummaryData {
            name: self.metadata.project.name.clone(),
            version: format!(
                "{}",
                self.metadata
                    .project
                    .version
                    .as_ref()
                    .ok_or(MetadataError::MissingVersion)?
            ),
            modules,
            proto_modules,
            interfaces,
            packages,
            components,
        };

        let mut handlebars = Handlebars::new();
        handlebars.register_escape_fn(handlebars::no_escape);
        Ok(handlebars.render_template(SUMMARY_TMPL, &data).unwrap())
    }

    fn build_index(&self) -> Result<String> {
        let data = IndexData {
            name: self.metadata.project.name.clone(),
            version: format!(
                "{}",
                self.metadata
                    .project
                    .version
                    .as_ref()
                    .ok_or(MetadataError::MissingVersion)?
            ),
            description: self.metadata.project.description.clone(),
            repository: self.metadata.project.repository.clone(),
            license: self.metadata.project.license.clone(),
            catalog: self.catalog(),
        };

        let mut handlebars = Handlebars::new();
        handlebars.register_escape_fn(handlebars::no_escape);
        Ok(handlebars.render_template(INDEX_TMPL, &data).unwrap())
    }

    fn top_level_items(items: &[TopLevelItem]) -> Vec<ListItem> {
        items
            .iter()
            .map(|x| ListItem {
                file_name: x.file_name.clone(),
                html_name: x.html_name.clone(),
                description: x.symbol.doc_comment.format(true),
            })
            .collect()
    }

    fn component_items(&self) -> Vec<ListItem> {
        self.components
            .iter()
            .map(|x| ListItem {
                file_name: x.file_name.clone(),
                html_name: x.name.clone(),
                description: x
                    .manifest
                    .doc
                    .as_deref()
                    .and_then(|d| d.lines().next())
                    .unwrap_or_default()
                    .to_string(),
            })
            .collect()
    }

    /// Index-page catalog; the sidebar's list pages show the same data.
    fn catalog(&self) -> Vec<ListData> {
        [
            ("Modules", Self::top_level_items(&self.modules)),
            (
                "Module Prototypes",
                Self::top_level_items(&self.proto_modules),
            ),
            ("Interfaces", Self::top_level_items(&self.interfaces)),
            ("Packages", Self::top_level_items(&self.packages)),
            ("Components", self.component_items()),
        ]
        .into_iter()
        .filter(|(_, items)| !items.is_empty())
        .map(|(name, items)| ListData {
            name: name.to_string(),
            items,
        })
        .collect()
    }

    fn render_list(name: &str, items: Vec<ListItem>) -> String {
        let data = ListData {
            name: name.to_string(),
            items,
        };
        let mut handlebars = Handlebars::new();
        handlebars.register_escape_fn(handlebars::no_escape);
        handlebars.render_template(LIST_TMPL, &data).unwrap()
    }

    fn build_modules(&self) -> String {
        Self::render_list("Modules", Self::top_level_items(&self.modules))
    }

    fn build_proto_modules(&self) -> String {
        Self::render_list(
            "Module Prototypes",
            Self::top_level_items(&self.proto_modules),
        )
    }

    fn build_interfaces(&self) -> String {
        Self::render_list("Interfaces", Self::top_level_items(&self.interfaces))
    }

    fn build_packages(&self) -> String {
        Self::render_list("Packages", Self::top_level_items(&self.packages))
    }

    fn build_components(&self) -> String {
        if self.components.is_empty() {
            return String::new();
        }
        Self::render_list("Components", self.component_items())
    }

    fn build_module(&self, name: &str, symbol: &Symbol) -> String {
        if let SymbolKind::Module(property) = &symbol.kind
            && !property.is_proto
        {
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
                        typ: x.property().r#type.to_string(),
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
        if let SymbolKind::Module(property) = &symbol.kind
            && property.is_proto
        {
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
                        typ: x.property().r#type.to_string(),
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
        if let SymbolKind::Interface(property) = &symbol.kind
            && !property.is_proto
        {
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
        if let SymbolKind::Package(x) = &symbol.kind
            && !x.is_proto
        {
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

fn build_component_page(item: &ComponentItem) -> String {
    let manifest = &item.manifest;

    let parameters: Vec<_> = manifest
        .params
        .iter()
        .map(|x| ComponentParameterData {
            name: x.name.clone(),
            typ: x.ty.clone(),
            optional: x.optional,
            description: x.doc.clone(),
        })
        .collect();
    let mut ports: Vec<_> = manifest
        .ports
        .iter()
        .map(|x| ComponentPortData {
            name: x.name.clone(),
            direction: x.dir.clone(),
            // A role names the Veryl type directly; data-port widths are
            // inferred from the connection, so none is shown.
            width: match x.role.as_deref() {
                Some(role) => role.to_string(),
                None => String::new(),
            },
            description: x.doc.clone(),
        })
        .collect();
    ports.extend(manifest.groups.iter().map(|g| ComponentPortData {
        name: g.name.clone(),
        direction: "modport".to_string(),
        width: format!("{}.{}", g.interface, g.modport),
        description: g.doc.clone(),
    }));
    let methods: Vec<_> = manifest
        .methods
        .iter()
        .map(|x| {
            // ret_suffix stays plain for the language server; rebuilt here to colour the types.
            let args: Vec<_> = x
                .args
                .iter()
                .map(|a| format!("{}: <span class=\"hljs-type\">{}</span>", a.name, a.ty))
                .collect();
            let ret = match (&x.ret, &x.ret_width) {
                (Some(t), Some(w)) => format!(" -> <span class=\"hljs-type\">{t}[{w}]</span>"),
                (Some(t), None) => format!(" -> <span class=\"hljs-type\">{t}</span>"),
                (None, _) => String::new(),
            };
            ComponentMethodData {
                signature: format!("{}({}){}", x.name, args.join(", "), ret),
                description: x.doc.clone(),
            }
        })
        .collect();

    let data = ComponentData {
        name: item.name.clone(),
        description: manifest.doc.clone().unwrap_or_default(),
        kind: manifest.kind.clone(),
        parameters,
        ports,
        methods,
        requires: manifest.requires.clone(),
        usage: component_usage(&item.name, manifest),
    };

    let mut handlebars = Handlebars::new();
    handlebars.register_escape_fn(handlebars::no_escape);
    handlebars.render_template(COMPONENT_TMPL, &data).unwrap()
}

/// Usage snippet on a component's doc page, including every required
/// parameter; `None` when the manifest does not declare the component's
/// kind.
fn component_usage(name: &str, manifest: &ComponentManifest) -> Option<String> {
    match manifest.kind.as_deref() {
        Some("clocked") => {
            let params: Vec<_> = manifest
                .params
                .iter()
                .filter(|x| !x.optional)
                .map(|x| x.name.clone())
                .collect();
            let params = if params.is_empty() {
                String::new()
            } else {
                format!("#( {} ) ", params.join(", "))
            };
            let mut ports: Vec<_> = manifest.ports.iter().map(|x| x.name.clone()).collect();
            ports.extend(manifest.groups.iter().map(|g| format!("{}: ", g.name)));
            Some(format!(
                "inst u0: $comp::{} {}({});",
                name,
                params,
                ports.join(", ")
            ))
        }
        Some("method_only") => {
            // Positional generic arguments must reach the last required
            // parameter; parameter names stand in as placeholders.
            let args = match manifest.params.iter().rposition(|x| !x.optional) {
                Some(last_required) => manifest.params[..=last_required]
                    .iter()
                    .map(|x| x.name.clone())
                    .collect::<Vec<_>>()
                    .join(", "),
                None => String::new(),
            };
            if args.is_empty() {
                Some(format!("var u0: $comp::{name};"))
            } else {
                Some(format!("var u0: $comp::{name}::<{args}>;"))
            }
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use veryl_metadata::component_manifest::{ManifestParam, ManifestPort};

    fn manifest() -> ComponentManifest {
        ComponentManifest {
            kind: Some("clocked".to_string()),
            doc: Some("Golden model checker.".to_string()),
            ports: vec![
                ManifestPort {
                    name: "clk".to_string(),
                    dir: "input".to_string(),
                    role: Some("clock".to_string()),
                    doc: None,
                },
                ManifestPort {
                    name: "q".to_string(),
                    dir: "output".to_string(),
                    role: None,
                    doc: None,
                },
            ],
            params: vec![
                ManifestParam {
                    name: "XLEN".to_string(),
                    ty: "u64".to_string(),
                    optional: false,
                    doc: None,
                },
                ManifestParam {
                    name: "TRACE".to_string(),
                    ty: "bool".to_string(),
                    optional: true,
                    doc: None,
                },
            ],
            methods: vec![],
            requires: vec![],
            groups: vec![],
        }
    }

    #[test]
    fn component_usage_snippet() {
        let manifest = manifest();
        assert_eq!(
            component_usage("golden", &manifest).as_deref(),
            Some("inst u0: $comp::golden #( XLEN ) (clk, q);")
        );

        let mut method_only = manifest.clone();
        method_only.kind = Some("method_only".to_string());
        assert_eq!(
            component_usage("golden", &method_only).as_deref(),
            Some("var u0: $comp::golden::<XLEN>;")
        );

        let mut no_required = manifest.clone();
        no_required.params[0].optional = true;
        assert_eq!(
            component_usage("golden", &no_required).as_deref(),
            Some("inst u0: $comp::golden (clk, q);")
        );
        no_required.kind = Some("method_only".to_string());
        assert_eq!(
            component_usage("golden", &no_required).as_deref(),
            Some("var u0: $comp::golden;")
        );

        let mut unspecified = manifest;
        unspecified.kind = None;
        assert_eq!(component_usage("golden", &unspecified), None);
    }

    #[test]
    fn component_page() {
        let item = ComponentItem {
            name: "golden".to_string(),
            file_name: "component_golden".to_string(),
            manifest: manifest(),
        };
        let page = build_component_page(&item);
        assert!(page.contains("<th class=\"table_list_item\">XLEN</th>"));
        assert!(page.contains("<td class=\"table_list_item\">required</td>"));
        assert!(page.contains("<td class=\"table_list_item\">optional</td>"));
        // Width 0 means "not declared" and renders as an empty cell.
        assert!(page.contains("<span class=\"hljs-type\"></span>"));
        assert!(page.contains("inst u0: $comp::golden #( XLEN ) (clk, q);"));
    }

    #[test]
    fn component_page_renders_interface_groups() {
        use veryl_metadata::component_manifest::{ManifestGroup, ManifestMember};
        let mut grouped = manifest();
        grouped.groups.push(ManifestGroup {
            name: "axi".to_string(),
            interface: "$std::axi4_if".to_string(),
            modport: "monitor".to_string(),
            members: vec![ManifestMember {
                member: "awvalid".to_string(),
                dir: "input".to_string(),
                doc: Some("Write-address valid.".to_string()),
            }],
            doc: Some("AXI4 monitor bus.".to_string()),
        });
        let item = ComponentItem {
            name: "checker".to_string(),
            file_name: "component_checker".to_string(),
            manifest: grouped,
        };
        let page = build_component_page(&item);
        assert!(!page.contains("### Interfaces"), "{page}");
        assert!(
            page.contains("<th class=\"table_list_item\">axi</th>"),
            "{page}"
        );
        assert!(
            page.contains("<span class=\"hljs-keyword\">modport</span>"),
            "{page}"
        );
        assert!(
            page.contains("<span class=\"hljs-type\">$std::axi4_if.monitor</span>"),
            "{page}"
        );
        assert!(page.contains("AXI4 monitor bus."), "{page}");
        assert!(!page.contains("awvalid"), "{page}");
        // The usage snippet includes the group connection.
        assert!(
            page.contains("inst u0: $comp::checker #( XLEN ) (clk, q, axi: );"),
            "{page}"
        );
    }
}
