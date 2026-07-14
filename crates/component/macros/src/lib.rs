//! Macros generating the declaration/behavior glue of a component:
//!
//! - `#[derive(Component)]` on the component struct classifies fields into
//!   ports, parameters and plain state, generates the constructor resolving
//!   them from `BuildCtx`, and embeds the declarative half of the JSON
//!   manifest (kind, ports, params, requires, doc comments).
//! - `#[component_impl]` on an inherent impl block turns the reserved hook
//!   names (`on_build`, `on_init`, `on_reset`, `on_clock`, `on_finish`) into
//!   `Component` trait hooks and every other function into a typed testbench
//!   method; it generates `impl Component` with the name dispatch and the
//!   methods half of the manifest.
//!
//! ```ignore
//! /// Mirrors d to q with one cycle delay.
//! #[derive(Component)]
//! #[component(kind = clocked, requires(file))]
//! struct Mirror {
//!     /// Sampling clock.
//!     clk: ClockPort,
//!     d: InputPort,               // port name `d`, width from the connection
//!     #[port(name = "q_out")]
//!     q: OutputPort,              // an explicit port-name override
//!     #[param]
//!     limit: u64,
//!     last: u64, // plain state, initialized with Default
//! }
//!
//! #[component_impl]
//! impl Mirror {
//!     fn on_clock(&mut self, ctx: &mut SimCtx) -> Result<()> { ... }
//!     /// Load an ELF file.
//!     fn load(&mut self, ctx: &mut SimCtx, path: &str) -> Result<()> { ... }
//! }
//! ```

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::spanned::Spanned;
use syn::{
    Attribute, Data, DeriveInput, Expr, ExprLit, Fields, FnArg, Ident, ImplItem, ItemImpl, Lit,
    LitStr, Meta, Pat, ReturnType, Type,
};

fn json_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

/// Joins the `#[doc]` lines of an item into one string.
fn doc_of(attrs: &[Attribute]) -> Option<String> {
    let mut lines = Vec::new();
    for attr in attrs {
        if attr.path().is_ident("doc")
            && let Meta::NameValue(nv) = &attr.meta
            && let Expr::Lit(ExprLit {
                lit: Lit::Str(s), ..
            }) = &nv.value
        {
            lines.push(s.value().trim().to_string());
        }
    }
    if lines.is_empty() {
        None
    } else {
        Some(lines.join("\n").trim().to_string())
    }
}

/// `,"doc":"..."` if the item is documented, empty otherwise.
fn doc_json(doc: &Option<String>) -> String {
    doc.as_ref()
        .map(|d| format!(r#","doc":"{}""#, json_escape(d)))
        .unwrap_or_default()
}

fn last_segment(ty: &Type) -> Option<String> {
    if let Type::Path(p) = ty {
        p.path.segments.last().map(|s| s.ident.to_string())
    } else {
        None
    }
}

/// Whether `ty` is a reference whose target's last path segment is `name`.
fn is_ref_to(ty: &Type, name: &str) -> bool {
    if let Type::Reference(r) = ty {
        last_segment(&r.elem).as_deref() == Some(name)
    } else {
        false
    }
}

// ---------------------------------------------------------------------------
// #[derive(Component)]
// ---------------------------------------------------------------------------

#[proc_macro_derive(Component, attributes(component, port, param, state, interface))]
pub fn derive_component(input: TokenStream) -> TokenStream {
    let input = syn::parse_macro_input!(input as DeriveInput);
    match expand_derive(input) {
        Ok(ts) => ts.into(),
        Err(e) => e.to_compile_error().into(),
    }
}

#[derive(Default)]
struct StructSpec {
    kind: Option<Ident>,
    requires: Vec<String>,
}

fn parse_struct_attrs(attrs: &[Attribute]) -> syn::Result<StructSpec> {
    let mut spec = StructSpec::default();
    for attr in attrs {
        if attr.path().is_ident("component") {
            attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("kind") {
                    spec.kind = Some(meta.value()?.parse()?);
                    Ok(())
                } else if meta.path.is_ident("requires") {
                    meta.parse_nested_meta(|req| {
                        match req.path.get_ident() {
                            Some(x) => spec.requires.push(x.to_string()),
                            None => return Err(req.error("expected an identifier")),
                        }
                        Ok(())
                    })
                } else {
                    Err(meta.error("expected `kind` or `requires`"))
                }
            })?;
        } else if attr.path().is_ident("interface") {
            return Err(syn::Error::new(
                attr.span(),
                "the interface binding is declared on a #[derive(VerylInterface)] struct; \
                 embed it here as an `#[interface]`-marked field",
            ));
        }
    }
    Ok(spec)
}

/// Registers a manifest-visible connection name (port or group);
/// first-match lookups would silently shadow a duplicate.
fn claim_port_name(
    names: &mut std::collections::HashSet<String>,
    span: proc_macro2::Span,
    name: &str,
) -> syn::Result<()> {
    if !names.insert(name.to_string()) {
        return Err(syn::Error::new(
            span,
            format!("duplicate port name `{name}`"),
        ));
    }
    Ok(())
}

/// The Veryl-visible name of a field: a raw identifier (`r#type`, used
/// when the Veryl name is a Rust keyword) drops its `r#` spelling.
fn veryl_name(ident: &Ident) -> String {
    ident.to_string().trim_start_matches("r#").to_string()
}

/// Dots separate a group from a member or parameter in width expressions
/// and port references, so bare names must not contain them.
fn reject_dotted(span: proc_macro2::Span, what: &str, name: &str) -> syn::Result<()> {
    if name.contains('.') {
        Err(syn::Error::new(span, format!("{what} cannot contain `.`")))
    } else {
        Ok(())
    }
}

/// Name overrides are manifest strings, not Rust identifiers; the `r#`
/// spelling belongs only on fields.
fn reject_raw(span: proc_macro2::Span, what: &str, name: &str) -> syn::Result<()> {
    if name.starts_with("r#") {
        Err(syn::Error::new(
            span,
            format!("{what} must be written without the `r#` prefix"),
        ))
    } else {
        Ok(())
    }
}

/// Parses an attribute of the form `#[attr(<key> = "..")]`; a bare
/// `#[attr]` yields `None`.
fn parse_string_attr(attr: &Attribute, key: &str) -> syn::Result<Option<String>> {
    let mut value = None;
    if matches!(attr.meta, Meta::Path(_)) {
        return Ok(None);
    }
    attr.parse_nested_meta(|meta| {
        if meta.path.is_ident(key) {
            let lit: LitStr = meta.value()?.parse()?;
            value = Some(lit.value());
            Ok(())
        } else {
            Err(meta.error(format!("expected `{key}`")))
        }
    })?;
    Ok(value)
}

/// The manifest-name override of a `#[port]` attribute (`#[port(name = ..)]`).
fn parse_port_attr(attr: &Attribute) -> syn::Result<Option<String>> {
    let name = parse_string_attr(attr, "name")?;
    // Loose ports are connected by identifier, so a dotted name would be
    // unconnectable.
    if let Some(name) = &name {
        reject_dotted(attr.span(), "a port name", name)?;
        reject_raw(attr.span(), "a port name", name)?;
    }
    Ok(name)
}

/// The Veryl-side name override of a `#[param]` field.
fn parse_param_attr(attr: &Attribute) -> syn::Result<Option<String>> {
    let name = parse_string_attr(attr, "name")?;
    if let Some(name) = &name {
        reject_raw(attr.span(), "a parameter name", name)?;
    }
    Ok(name)
}

/// The parameter manifest type string and the conversion of a `Value`
/// bound to `__v`, for one supported parameter field type.
fn value_conversion(ty: &Type) -> syn::Result<(String, TokenStream2)> {
    let seg = last_segment(ty).unwrap_or_default();
    let conv = match seg.as_str() {
        "u64" => quote!(__v.as_u64()?),
        "u8" | "u16" | "u32" => quote!(<#ty>::try_from(__v.as_u64()?)?),
        "i64" => quote!(__v.as_i64()?),
        "i8" | "i16" | "i32" => quote!(<#ty>::try_from(__v.as_i64()?)?),
        "bool" => quote!(__v.as_bool_strict()?),
        "String" => quote!(__v.as_str()?.to_string()),
        "Value" => quote!(__v),
        _ => {
            return Err(syn::Error::new(
                ty.span(),
                "unsupported parameter type; use an integer, bool, String, Value or an Option of those",
            ));
        }
    };
    let ty_str = match seg.as_str() {
        "String" => "str".to_string(),
        "Value" => "value".to_string(),
        other => other.to_string(),
    };
    Ok((ty_str, conv))
}

/// The parameter manifest type string and the `BuildCtx` resolution for one
/// parameter field. `Option<T>` marks the parameter as omittable.
/// Conversion failures name the parameter; the raw conversion message alone
/// gives the test author no context.
fn param_conversion(ty: &Type, name: &str) -> syn::Result<(String, TokenStream2, bool)> {
    if last_segment(ty).as_deref() == Some("Option")
        && let Type::Path(p) = ty
        && let Some(seg) = p.path.segments.last()
        && let syn::PathArguments::AngleBracketed(args) = &seg.arguments
        && let Some(syn::GenericArgument::Type(inner)) = args.args.first()
    {
        let (ty_str, conv) = value_conversion(inner)?;
        let resolve = quote! {
            match ctx.param(#name) {
                ::core::result::Result::Ok(__v) => ::core::option::Option::Some(
                    (|| -> ::veryl_component::Result<_> {
                        ::core::result::Result::Ok(#conv)
                    })()
                    .map_err(|e| ::veryl_component::anyhow!(
                        "parameter `{}`: {e:#}", #name
                    ))?,
                ),
                ::core::result::Result::Err(_) => ::core::option::Option::None,
            }
        };
        return Ok((ty_str, resolve, true));
    }
    let (ty_str, conv) = value_conversion(ty)?;
    let resolve = quote! {
        {
            let __v = ctx.param(#name)?;
            (|| -> ::veryl_component::Result<_> {
                ::core::result::Result::Ok(#conv)
            })()
            .map_err(|e| ::veryl_component::anyhow!("parameter `{}`: {e:#}", #name))?
        }
    };
    Ok((ty_str, resolve, false))
}

fn expand_derive(input: DeriveInput) -> syn::Result<TokenStream2> {
    let ident = &input.ident;
    let spec = parse_struct_attrs(&input.attrs)?;
    let struct_doc = doc_of(&input.attrs);

    let declared_kind = match spec.kind.as_ref().map(|x| x.to_string()).as_deref() {
        None | Some("unspecified") => None,
        Some(x @ ("clocked" | "method_only")) => Some(x.to_string()),
        Some(_) => {
            return Err(syn::Error::new(
                spec.kind.unwrap().span(),
                "expected `clocked`, `method_only` or `unspecified`",
            ));
        }
    };

    let Data::Struct(data) = &input.data else {
        return Err(syn::Error::new(
            input.span(),
            "#[derive(Component)] supports structs only",
        ));
    };
    let fields: Vec<&syn::Field> = match &data.fields {
        Fields::Named(x) => x.named.iter().collect(),
        Fields::Unit => vec![],
        Fields::Unnamed(_) => {
            return Err(syn::Error::new(
                input.span(),
                "#[derive(Component)] requires named fields",
            ));
        }
    };

    let mut ports_json = Vec::new();
    let mut params_json = Vec::new();
    let mut inits = Vec::new();
    let mut has_clock_port = false;
    // #[interface] fields, spliced into the manifest by const
    // concatenation.
    let mut groups: Vec<(String, Type, Option<String>)> = Vec::new();
    // Port and group names share the connection-item namespace, so
    // duplicates are rejected here where both declarations are visible.
    let mut port_names: std::collections::HashSet<String> = std::collections::HashSet::new();
    for field in &fields {
        let field_ident = field.ident.as_ref().unwrap();
        let doc = doc_of(&field.attrs);
        let iface_attr = field.attrs.iter().find(|a| a.path().is_ident("interface"));
        let port_attr = field.attrs.iter().find(|a| a.path().is_ident("port"));
        let param_attr = field.attrs.iter().find(|a| a.path().is_ident("param"));
        let is_state = field.attrs.iter().any(|a| a.path().is_ident("state"));
        if (iface_attr.is_some() as u8)
            + (port_attr.is_some() as u8)
            + (param_attr.is_some() as u8)
            + (is_state as u8)
            > 1
        {
            return Err(syn::Error::new(
                field.span(),
                "`#[interface]`, `#[port]`, `#[param]` and `#[state]` are mutually exclusive",
            ));
        }
        if let Some(attr) = iface_attr {
            if !matches!(attr.meta, Meta::Path(_)) {
                return Err(syn::Error::new(
                    attr.span(),
                    "#[interface] on a component field takes no arguments; the binding \
                     lives on the field's #[derive(VerylInterface)] type",
                ));
            }
            let group = veryl_name(field_ident);
            claim_port_name(&mut port_names, field.span(), &group)?;
            let ty = field.ty.clone();
            inits.push(quote!(#field_ident:
                <#ty as ::veryl_component::VerylInterface>::resolve(ctx, #group)?));
            groups.push((group, ty, doc));
            continue;
        }

        let seg = last_segment(&field.ty).unwrap_or_default();
        if is_state {
            inits.push(quote!(#field_ident: ::core::default::Default::default()));
        } else if let Some(param_attr) = param_attr {
            let name = parse_param_attr(param_attr)?.unwrap_or_else(|| veryl_name(field_ident));
            reject_dotted(field.span(), "a parameter name", &name)?;
            let (ty_str, conv, optional) = param_conversion(&field.ty, &name)?;
            let optional = if optional { r#","optional":true"# } else { "" };
            params_json.push(format!(
                r#"{{"name":"{}","type":"{}"{}{}}}"#,
                json_escape(&name),
                ty_str,
                optional,
                doc_json(&doc)
            ));
            inits.push(quote!(#field_ident: #conv));
        } else if seg == "InputPort" || seg == "OutputPort" {
            let name = port_attr
                .map(parse_port_attr)
                .transpose()?
                .flatten()
                .unwrap_or_else(|| veryl_name(field_ident));
            claim_port_name(&mut port_names, field.span(), &name)?;
            let dir = if seg == "InputPort" {
                "input"
            } else {
                "output"
            };
            // Port widths are inferred from the connected signal; a
            // component enforces any width constraints in `on_build`.
            ports_json.push(format!(
                r#"{{"name":"{}","dir":"{}"{}}}"#,
                json_escape(&name),
                dir,
                doc_json(&doc)
            ));
            let resolve = if seg == "InputPort" {
                quote!(ctx.input(#name)?)
            } else {
                quote!(ctx.output(#name)?)
            };
            inits.push(quote!(#field_ident: #resolve));
        } else if seg == "ClockPort" || seg == "ResetPort" {
            if declared_kind.as_deref() == Some("method_only") {
                return Err(syn::Error::new(
                    field.span(),
                    "method-only components cannot have clock or reset ports",
                ));
            }
            has_clock_port |= seg == "ClockPort";
            let name = port_attr
                .map(parse_port_attr)
                .transpose()?
                .flatten()
                .unwrap_or_else(|| veryl_name(field_ident));
            claim_port_name(&mut port_names, field.span(), &name)?;
            let role = if seg == "ClockPort" { "clock" } else { "reset" };
            ports_json.push(format!(
                r#"{{"name":"{}","dir":"input","role":"{}"{}}}"#,
                json_escape(&name),
                role,
                doc_json(&doc)
            ));
            let resolve = if seg == "ClockPort" {
                quote!(ctx.clock(#name)?)
            } else {
                quote!(ctx.reset(#name)?)
            };
            inits.push(quote!(#field_ident: #resolve));
        } else if port_attr.is_some() {
            return Err(syn::Error::new(
                field.span(),
                "#[port] fields must be of type InputPort, OutputPort, ClockPort or ResetPort",
            ));
        } else {
            inits.push(quote!(#field_ident: ::core::default::Default::default()));
        }
    }

    // A `ClockPort` field is what makes a component clocked, so the kind is
    // inferred from it; an explicit declaration must agree.
    let (kind_variant, kind_str) = match declared_kind.as_deref() {
        Some("clocked") if !has_clock_port => {
            return Err(syn::Error::new(
                spec.kind.unwrap().span(),
                "clocked components declare their clock with a ClockPort field",
            ));
        }
        Some("clocked") => (quote!(Clocked), "clocked"),
        Some("method_only") => (quote!(MethodOnly), "method_only"),
        None if has_clock_port => (quote!(Clocked), "clocked"),
        _ => (quote!(Unspecified), "unspecified"),
    };

    let requires_json = spec
        .requires
        .iter()
        .map(|r| format!(r#""{}""#, json_escape(r)))
        .collect::<Vec<_>>()
        .join(",");
    // Brace-less fragment; `#[component_impl]` completes it with the
    // methods array. The leading `doc` key has no comma prefix, so it is
    // emitted first. `kind` is embedded so a prebuilt wasm manifest is
    // readable without executing the guest. Groups splice in the member
    // declarations of their `VerylInterface` types, whose definitions this
    // derive cannot see — hence the const concatenation.
    let decl_prefix = format!(
        r#"{}"kind":"{}","ports":[{}],"params":[{}],"requires":[{}],"groups":["#,
        struct_doc
            .as_ref()
            .map(|d| format!(r#""doc":"{}","#, json_escape(d)))
            .unwrap_or_default(),
        kind_str,
        ports_json.join(","),
        params_json.join(","),
        requires_json,
    );
    // Splicing another type's consts needs const concatenation; a component
    // without interface fields keeps the plain literal.
    let mut decl_parts: Vec<TokenStream2> = vec![quote!(#decl_prefix)];
    for (i, (group, ty, doc)) in groups.iter().enumerate() {
        let head = format!(
            r#"{}{{"name":"{}"{}"interface":""#,
            if i == 0 { "" } else { "," },
            json_escape(group),
            doc.as_ref()
                .map(|d| format!(r#","doc":"{}","#, json_escape(d)))
                .unwrap_or_else(|| ",".to_string()),
        );
        decl_parts.push(quote!(#head));
        decl_parts.push(quote!(<#ty as ::veryl_component::VerylInterface>::INTERFACE_PATH));
        decl_parts.push(quote!(r#"","modport":""#));
        decl_parts.push(quote!(<#ty as ::veryl_component::VerylInterface>::MODPORT));
        decl_parts.push(quote!(r#"","members":"#));
        decl_parts.push(quote!(<#ty as ::veryl_component::VerylInterface>::MEMBERS_JSON));
        decl_parts.push(quote!("}"));
    }
    decl_parts.push(quote!("]"));
    let decl_json: TokenStream2 = if groups.is_empty() {
        let lit = format!("{decl_prefix}]");
        quote!(#lit)
    } else {
        quote! {{
            const PARTS: &[&str] = &[#(#decl_parts),*];
            const LEN: usize = ::veryl_component::export::concat_len(PARTS);
            const BYTES: [u8; LEN] = ::veryl_component::export::concat_bytes(PARTS);
            ::veryl_component::export::bytes_as_str(&BYTES)
        }}
    };

    let body = if matches!(data.fields, Fields::Unit) {
        quote!(Self)
    } else {
        quote!(Self { #(#inits),* })
    };
    let (impl_generics, ty_generics, where_clause) = input.generics.split_for_impl();
    Ok(quote! {
        impl #impl_generics #ident #ty_generics #where_clause {
            #[doc(hidden)]
            pub const __COMPONENT_KIND: ::veryl_component::ComponentKind =
                ::veryl_component::ComponentKind::#kind_variant;
            #[doc(hidden)]
            pub const __COMPONENT_DECL_JSON: &'static str = #decl_json;
            #[doc(hidden)]
            #[allow(clippy::needless_question_mark)]
            pub fn __component_build(
                ctx: &mut ::veryl_component::BuildCtx,
            ) -> ::veryl_component::Result<Self> {
                ::core::result::Result::Ok(#body)
            }
        }
    })
}

// ---------------------------------------------------------------------------
// #[derive(VerylInterface)]
// ---------------------------------------------------------------------------

#[proc_macro_derive(VerylInterface, attributes(interface, port))]
pub fn derive_interface(input: TokenStream) -> TokenStream {
    let input = syn::parse_macro_input!(input as DeriveInput);
    match expand_interface(input) {
        Ok(ts) => ts.into(),
        Err(e) => e.to_compile_error().into(),
    }
}

/// Spliced verbatim into JSON string position by the embedding component's
/// const concatenation, so escapes cannot be applied there.
fn reject_json_unsafe(span: proc_macro2::Span, what: &str, value: &str) -> syn::Result<()> {
    if value.contains(['"', '\\']) || value.chars().any(|c| (c as u32) < 0x20) {
        return Err(syn::Error::new(
            span,
            format!("{what} cannot contain quotes, backslashes or control characters"),
        ));
    }
    Ok(())
}

fn expand_interface(input: DeriveInput) -> syn::Result<TokenStream2> {
    let ident = &input.ident;
    let (mut path, mut modport) = (None, None);
    for attr in &input.attrs {
        if !attr.path().is_ident("interface") {
            continue;
        }
        attr.parse_nested_meta(|meta| {
            let key = if meta.path.is_ident("path") {
                &mut path
            } else if meta.path.is_ident("modport") {
                &mut modport
            } else {
                return Err(meta.error("expected `path` or `modport`"));
            };
            let lit: LitStr = meta.value()?.parse()?;
            *key = Some(lit.value());
            Ok(())
        })?;
    }
    let missing = |k| {
        syn::Error::new(
            input.span(),
            format!("#[derive(VerylInterface)] needs `#[interface({k} = ..)]`"),
        )
    };
    let path = path.ok_or_else(|| missing("path"))?;
    let modport = modport.ok_or_else(|| missing("modport"))?;
    reject_json_unsafe(input.span(), "an interface path", &path)?;
    reject_json_unsafe(input.span(), "a modport name", &modport)?;

    let Data::Struct(data) = &input.data else {
        return Err(syn::Error::new(
            input.span(),
            "#[derive(VerylInterface)] supports structs only",
        ));
    };
    let Fields::Named(fields) = &data.fields else {
        return Err(syn::Error::new(
            input.span(),
            "#[derive(VerylInterface)] requires named fields",
        ));
    };

    let mut members_json = Vec::new();
    let mut inits = Vec::new();
    for field in &fields.named {
        let field_ident = field.ident.as_ref().unwrap();
        let doc = doc_of(&field.attrs);
        let seg = last_segment(&field.ty).unwrap_or_default();
        let (dir, resolve) = match seg.as_str() {
            "InputPort" => ("input", quote!(input)),
            "OutputPort" => ("output", quote!(output)),
            _ => {
                return Err(syn::Error::new(
                    field.span(),
                    "interface members are InputPort or OutputPort fields",
                ));
            }
        };
        if let Some(attr) = field.attrs.iter().find(|a| a.path().is_ident("port")) {
            return Err(syn::Error::new(
                attr.span(),
                "`#[port]` is not supported on interface members; the field \
                 name is the member name",
            ));
        }
        let member = veryl_name(field_ident);
        members_json.push(format!(
            r#"{{"member":"{}","dir":"{}"{}}}"#,
            json_escape(&member),
            dir,
            doc_json(&doc)
        ));
        // The group name is only known where the component embeds this
        // type.
        inits.push(quote! {
            #field_ident: ctx.#resolve(
                &::veryl_component::sys::member_port_name(group, #member),
            )?
        });
    }
    let members_lit = format!("[{}]", members_json.join(","));

    let (impl_generics, ty_generics, where_clause) = input.generics.split_for_impl();
    Ok(quote! {
        impl #impl_generics ::veryl_component::VerylInterface for #ident #ty_generics #where_clause {
            const INTERFACE_PATH: &'static str = #path;
            const MODPORT: &'static str = #modport;
            const MEMBERS_JSON: &'static str = #members_lit;

            fn resolve(
                ctx: &mut ::veryl_component::BuildCtx,
                group: &str,
            ) -> ::veryl_component::Result<Self> {
                ::core::result::Result::Ok(Self { #(#inits),* })
            }
        }
    })
}

// ---------------------------------------------------------------------------
// #[component_impl]
// ---------------------------------------------------------------------------

const HOOKS: [&str; 5] = ["on_build", "on_init", "on_reset", "on_clock", "on_finish"];

/// Hooks are forwarded from the generated `Component` impl, so a signature
/// mismatch would otherwise surface as an opaque error inside generated
/// code; reject it here with a spanned message instead.
fn check_hook_signature(func: &syn::ImplItemFn) -> syn::Result<()> {
    let name = func.sig.ident.to_string();
    let ctx_ty = if name == "on_build" {
        "BuildCtx"
    } else {
        "SimCtx"
    };
    let mut inputs = func.sig.inputs.iter();
    let self_ok = matches!(inputs.next(), Some(FnArg::Receiver(r)) if r.reference.is_some());
    let ctx_ok = matches!(inputs.next(), Some(FnArg::Typed(pat)) if is_ref_to(&pat.ty, ctx_ty));
    if !self_ok || !ctx_ok || inputs.next().is_some() {
        return Err(syn::Error::new(
            func.sig.span(),
            format!("`{name}` hooks take `&mut self, ctx: &mut {ctx_ty}` and no other arguments"),
        ));
    }
    Ok(())
}

struct MethodSpec {
    ident: Ident,
    doc: Option<String>,
    args: Vec<ArgSpec>,
    /// Manifest type string of the return value; `None` for unit.
    ret: Option<String>,
    /// Manifest width expression of a `value` return (`#[ret_width(..)]`).
    ret_width: Option<String>,
}

struct ArgSpec {
    name: String,
    /// Manifest type string.
    ty: String,
    /// Conversion from `args[idx]`.
    conv: TokenStream2,
}

/// Encodes a width expression (an integer, a Veryl parameter name, a
/// group-qualified interface parameter like `axi.DATA_WIDTH_BYTES`, or
/// `+ - * /` arithmetic over them) as structured manifest JSON: a number
/// literal, a quoted (possibly dotted) parameter name, or
/// `{"op":..,"lhs":..,"rhs":..}`. The metadata side then needs no
/// expression parser. Unsupported syntax is a compile error at the
/// component, not a silent runtime failure.
fn width_expr_json(expr: &Expr) -> syn::Result<String> {
    match expr {
        Expr::Lit(ExprLit {
            lit: Lit::Int(n), ..
        }) => Ok(n.base10_parse::<u64>()?.to_string()),
        Expr::Path(p) if p.path.get_ident().is_some() => {
            let name = veryl_name(p.path.get_ident().unwrap());
            Ok(format!("\"{}\"", json_escape(&name)))
        }
        // `group.NAME` parses as a field access; the analyzer resolves the
        // name in the interface the `group` interface port is bound to.
        Expr::Field(fa) => {
            let group = match fa.base.as_ref() {
                Expr::Path(p) if p.path.get_ident().is_some() => {
                    veryl_name(p.path.get_ident().unwrap())
                }
                _ => {
                    return Err(syn::Error::new_spanned(
                        expr,
                        "an interface parameter reference is `<group>.<NAME>` \
                         with a plain interface port name",
                    ));
                }
            };
            let syn::Member::Named(name) = &fa.member else {
                return Err(syn::Error::new_spanned(
                    expr,
                    "an interface parameter reference is `<group>.<NAME>` \
                     with a named constant",
                ));
            };
            Ok(format!(
                "\"{}.{}\"",
                json_escape(&group),
                json_escape(&veryl_name(name))
            ))
        }
        Expr::Paren(inner) => width_expr_json(&inner.expr),
        Expr::Group(inner) => width_expr_json(&inner.expr),
        Expr::Binary(b) => {
            let op = match b.op {
                syn::BinOp::Add(_) => '+',
                syn::BinOp::Sub(_) => '-',
                syn::BinOp::Mul(_) => '*',
                syn::BinOp::Div(_) => '/',
                _ => {
                    return Err(syn::Error::new_spanned(
                        expr,
                        "unsupported width operator (only + - * / are allowed)",
                    ));
                }
            };
            let lhs = width_expr_json(&b.left)?;
            let rhs = width_expr_json(&b.right)?;
            Ok(format!(r#"{{"op":"{op}","lhs":{lhs},"rhs":{rhs}}}"#))
        }
        _ => Err(syn::Error::new_spanned(
            expr,
            "unsupported width expression (use integers, parameter names, \
             `<group>.<NAME>` interface parameters, and + - * /)",
        )),
    }
}

/// Extracts and removes a `#[ret_width(<expr>)]` method attribute, declaring
/// the width of a `Value` return. (Argument widths are inferred from the
/// call-site expression, so arguments carry no width attribute.)
fn take_ret_width(attrs: &mut Vec<Attribute>) -> syn::Result<Option<String>> {
    let Some(pos) = attrs.iter().position(|a| a.path().is_ident("ret_width")) else {
        return Ok(None);
    };
    let attr = attrs.remove(pos);
    let expr: Expr = attr.parse_args()?;
    Ok(Some(width_expr_json(&expr)?))
}

/// The argument manifest type string and the conversion from `args[idx]`
/// for one supported method argument type.
fn arg_conversion(ty: &Type, idx: usize) -> syn::Result<(String, TokenStream2)> {
    if let Type::Reference(r) = ty {
        let seg = last_segment(&r.elem).unwrap_or_default();
        return match seg.as_str() {
            "str" => Ok(("str".to_string(), quote!(args[#idx].as_str()?))),
            "Value" => Ok(("value".to_string(), quote!(&args[#idx]))),
            _ => Err(syn::Error::new(
                ty.span(),
                "unsupported method argument type",
            )),
        };
    }
    let seg = last_segment(ty).unwrap_or_default();
    let conv = match seg.as_str() {
        "String" => quote!(args[#idx].as_str()?.to_string()),
        "u64" => quote!(args[#idx].as_u64()?),
        "u8" | "u16" | "u32" => quote!(<#ty>::try_from(args[#idx].as_u64()?)?),
        "i64" => quote!(args[#idx].as_i64()?),
        "i8" | "i16" | "i32" => quote!(<#ty>::try_from(args[#idx].as_i64()?)?),
        "bool" => quote!(args[#idx].as_bool_strict()?),
        "Value" => quote!(args[#idx].clone()),
        _ => {
            return Err(syn::Error::new(
                ty.span(),
                "unsupported method argument type; use &str, String, an integer, bool or Value",
            ));
        }
    };
    let ty_str = match seg.as_str() {
        "String" => "str".to_string(),
        "Value" => "value".to_string(),
        other => other.to_string(),
    };
    Ok((ty_str, conv))
}

/// The manifest type string of a method's return value (`None` for unit).
/// `Result` may be spelled through an alias, so any single-argument generic
/// return type except `Option` is accepted; a wrong type still fails on the
/// generated `Value::from`.
fn return_type(output: &ReturnType) -> syn::Result<Option<String>> {
    let ReturnType::Type(_, ty) = output else {
        return Err(syn::Error::new(
            output.span(),
            "testbench methods must return `Result<...>`",
        ));
    };
    if let Type::Path(p) = ty.as_ref()
        && let Some(seg) = p.path.segments.last()
    {
        if seg.ident == "Option" {
            return Err(syn::Error::new(
                ty.span(),
                "testbench methods must return `Result<...>`, not `Option`",
            ));
        }
        if let syn::PathArguments::AngleBracketed(args) = &seg.arguments
            && let Some(syn::GenericArgument::Type(inner)) = args.args.first()
        {
            if let Type::Tuple(t) = inner {
                return if t.elems.is_empty() {
                    Ok(None)
                } else {
                    Err(syn::Error::new(
                        inner.span(),
                        "unsupported method return type",
                    ))
                };
            }
            let seg = last_segment(inner).unwrap_or_default();
            let is_str_ref = matches!(inner, Type::Reference(r)
                if last_segment(&r.elem).as_deref() == Some("str"));
            if seg == "String" || is_str_ref {
                // The ABI return slot carries bits or unit only.
                return Err(syn::Error::new(
                    inner.span(),
                    "string return values are not supported by the component ABI",
                ));
            }
            let ty_str = match seg.as_str() {
                // Anything convertible with `Value::from` is accepted; without
                // a recognizable name the manifest records a generic value.
                "Value" | "" => "value".to_string(),
                other => other.to_string(),
            };
            return Ok(Some(ty_str));
        }
    }
    Err(syn::Error::new(
        ty.span(),
        "testbench methods must return `Result<...>` (with an explicit value type)",
    ))
}

/// Parses one testbench method, consuming its `#[ret_width(..)]` attribute
/// (declaration metadata, not a real Rust attribute).
fn parse_method(func: &mut syn::ImplItemFn) -> syn::Result<MethodSpec> {
    let ret = return_type(&func.sig.output)?;
    let ret_width = take_ret_width(&mut func.attrs)?;
    if ret_width.is_some() && ret.as_deref() != Some("value") {
        return Err(syn::Error::new(
            func.sig.output.span(),
            "#[ret_width(..)] applies to methods returning `Value` (other types imply their width)",
        ));
    }
    if ret.as_deref() == Some("value") && ret_width.is_none() {
        return Err(syn::Error::new(
            func.sig.output.span(),
            "a `Value` return must declare its width with `#[ret_width(..)]`; \
             use an integer type for a fixed width up to 64 bits",
        ));
    }

    let mut inputs = func.sig.inputs.iter_mut();
    let self_ok = matches!(inputs.next(), Some(FnArg::Receiver(r)) if r.reference.is_some());
    let ctx_ok = matches!(inputs.next(), Some(FnArg::Typed(pat)) if is_ref_to(&pat.ty, "SimCtx"));
    if !self_ok || !ctx_ok {
        return Err(syn::Error::new(
            func.sig.span(),
            "testbench methods take `&mut self, ctx: &mut SimCtx, ...`; \
             helper functions belong in a separate impl block",
        ));
    }
    let mut args = Vec::new();
    for (idx, input) in inputs.enumerate() {
        let FnArg::Typed(pat) = input else {
            unreachable!("receiver cannot appear after the first argument");
        };
        let Pat::Ident(name) = pat.pat.as_ref() else {
            return Err(syn::Error::new(
                pat.span(),
                "method arguments must be plain identifiers",
            ));
        };
        // A `Value` argument's width is inferred from the call-site expression.
        let (ty_str, conv) = arg_conversion(&pat.ty, idx)?;
        args.push(ArgSpec {
            name: veryl_name(&name.ident),
            ty: ty_str,
            conv,
        });
    }
    Ok(MethodSpec {
        ident: func.sig.ident.clone(),
        doc: doc_of(&func.attrs),
        args,
        ret,
        ret_width,
    })
}

#[proc_macro_attribute]
pub fn component_impl(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let item_impl = syn::parse_macro_input!(item as ItemImpl);
    match expand_component_impl(item_impl) {
        Ok(ts) => ts.into(),
        Err(e) => e.to_compile_error().into(),
    }
}

/// JSON for one method argument: `{"name":..,"type":..}`.
fn method_arg_json(a: &ArgSpec) -> String {
    format!(r#"{{"name":"{}","type":"{}"}}"#, json_escape(&a.name), a.ty,)
}

/// JSON for one method: name, args, and optional ret/ret_width/doc.
fn method_json(m: &MethodSpec) -> String {
    let args = m
        .args
        .iter()
        .map(method_arg_json)
        .collect::<Vec<_>>()
        .join(",");
    let ret = m
        .ret
        .as_ref()
        .map(|t| format!(r#","ret":"{t}""#))
        .unwrap_or_default();
    let ret_width = m
        .ret_width
        .as_ref()
        .map(|w| format!(r#","ret_width":{w}"#))
        .unwrap_or_default();
    format!(
        r#"{{"name":"{}","args":[{}]{}{}{}}}"#,
        json_escape(&veryl_name(&m.ident)),
        args,
        ret,
        ret_width,
        doc_json(&m.doc)
    )
}

/// The `"methods":[..]` half of the manifest JSON.
fn methods_manifest_json(methods: &[MethodSpec]) -> String {
    let entries = methods
        .iter()
        .map(method_json)
        .collect::<Vec<_>>()
        .join(",");
    format!(r#""methods":[{entries}]"#)
}

/// Constructor body: run `on_build` after `__component_build` when present.
fn new_body(self_ty: &Type, hooks: &[String]) -> TokenStream2 {
    if hooks.iter().any(|h| h == "on_build") {
        quote! {
            let mut __this = <#self_ty>::__component_build(ctx)?;
            __this.on_build(ctx)?;
            ::core::result::Result::Ok(__this)
        }
    } else {
        quote!(<#self_ty>::__component_build(ctx))
    }
}

/// Trait hook methods forwarding to the inherent functions. `<Self>::hook`
/// resolves to the inherent function (inherent items take priority), so the
/// forward does not recurse. `on_build` is handled in `new`, so it is
/// skipped here.
fn hook_forwards(self_ty: &Type, hooks: &[String]) -> Vec<TokenStream2> {
    hooks
        .iter()
        .filter(|h| *h != "on_build")
        .map(|h| {
            let ident = Ident::new(h, proc_macro2::Span::call_site());
            quote! {
                fn #ident(
                    &mut self,
                    ctx: &mut ::veryl_component::SimCtx,
                ) -> ::veryl_component::Result<()> {
                    <#self_ty>::#ident(self, ctx)
                }
            }
        })
        .collect()
}

/// The `method(name, args, ctx)` dispatcher over the typed methods; empty
/// when the component declares none.
fn method_dispatch(self_ty: &Type, methods: &[MethodSpec]) -> TokenStream2 {
    if methods.is_empty() {
        return quote!();
    }
    let arms: Vec<TokenStream2> = methods
        .iter()
        .map(|m| {
            let ident = &m.ident;
            let name = veryl_name(ident);
            let argc = m.args.len();
            let arg_names: Vec<Ident> = (0..m.args.len())
                .map(|i| Ident::new(&format!("__arg{i}"), ident.span()))
                .collect();
            // Conversion failures name the method and argument; the raw
            // TryFrom message alone gives the test author no context.
            let convs: Vec<TokenStream2> = m
                .args
                .iter()
                .map(
                    |ArgSpec {
                         name: arg_name,
                         conv: c,
                         ..
                     }| {
                        quote! {
                            (|| -> ::veryl_component::Result<_> {
                                ::core::result::Result::Ok(#c)
                            })()
                            .map_err(|e| ::veryl_component::anyhow!(
                                "method `{}` argument `{}`: {e:#}", #name, #arg_name
                            ))?
                        }
                    },
                )
                .collect();
            let call = quote!(<#self_ty>::#ident(self, ctx #(, #arg_names)*));
            let body = if m.ret.is_none() {
                quote! {
                    #call?;
                    ::core::result::Result::Ok(::veryl_component::Value::unit())
                }
            } else {
                quote! {
                    let __ret = #call?;
                    ::core::result::Result::Ok(::veryl_component::Value::from(__ret))
                }
            };
            quote! {
                #name => {
                    if args.len() != #argc {
                        ::veryl_component::bail!(
                            "method `{}` expects {} argument(s), got {}",
                            #name, #argc, args.len()
                        );
                    }
                    #(let #arg_names = #convs;)*
                    #body
                }
            }
        })
        .collect();
    quote! {
        #[allow(clippy::needless_question_mark)]
        fn method(
            &mut self,
            name: &str,
            args: &[::veryl_component::Value],
            ctx: &mut ::veryl_component::SimCtx,
        ) -> ::veryl_component::Result<::veryl_component::Value> {
            match name {
                #(#arms)*
                _ => ::veryl_component::bail!("unknown method: {name}"),
            }
        }
    }
}

fn expand_component_impl(mut item_impl: ItemImpl) -> syn::Result<TokenStream2> {
    if let Some((_, path, _)) = &item_impl.trait_ {
        return Err(syn::Error::new(
            path.span(),
            "#[component_impl] goes on an inherent impl block, not a trait impl",
        ));
    }

    let mut hooks = Vec::new();
    let mut methods = Vec::new();
    for item in &mut item_impl.items {
        let ImplItem::Fn(func) = item else { continue };
        let name = func.sig.ident.to_string();
        if HOOKS.contains(&name.as_str()) {
            check_hook_signature(func)?;
            hooks.push(name);
        } else {
            methods.push(parse_method(func)?);
        }
    }
    let self_ty = &item_impl.self_ty;

    let methods_lit = LitStr::new(
        &methods_manifest_json(&methods),
        proc_macro2::Span::call_site(),
    );
    let new_body = new_body(self_ty, &hooks);
    let hook_forwards = hook_forwards(self_ty, &hooks);
    let dispatch = method_dispatch(self_ty, &methods);

    let (impl_generics, _, where_clause) = item_impl.generics.split_for_impl();
    Ok(quote! {
        #item_impl

        impl #impl_generics ::veryl_component::Component for #self_ty #where_clause {
            const KIND: ::veryl_component::ComponentKind = <#self_ty>::__COMPONENT_KIND;

            const MANIFEST_JSON: ::core::option::Option<&'static str> = {
                const PARTS: &[&str] =
                    &["{", <#self_ty>::__COMPONENT_DECL_JSON, ",", #methods_lit, "}"];
                const LEN: usize = ::veryl_component::export::concat_len(PARTS);
                const BYTES: [u8; LEN] = ::veryl_component::export::concat_bytes(PARTS);
                ::core::option::Option::Some(::veryl_component::export::bytes_as_str(&BYTES))
            };

            fn new(
                ctx: &mut ::veryl_component::BuildCtx,
            ) -> ::veryl_component::Result<Self> {
                #new_body
            }

            #(#hook_forwards)*

            #dispatch
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use syn::parse_quote;

    fn method_err(mut func: syn::ImplItemFn) -> String {
        parse_method(&mut func).err().unwrap().to_string()
    }

    #[test]
    fn kind_must_agree_with_port_fields() {
        let err = expand_derive(parse_quote! {
            #[component(kind = clocked)]
            struct C {
                d: InputPort,
            }
        })
        .unwrap_err()
        .to_string();
        assert!(err.contains("ClockPort field"), "{err}");

        let err = expand_derive(parse_quote! {
            #[component(kind = method_only)]
            struct C {
                clk: ClockPort,
            }
        })
        .unwrap_err()
        .to_string();
        assert!(
            err.contains("method-only components cannot have clock or reset ports"),
            "{err}"
        );

        let ok = expand_derive(parse_quote! {
            struct C {
                clk: ClockPort,
            }
        })
        .unwrap()
        .to_string();
        assert!(ok.contains("Clocked"), "kind not inferred: {ok}");
    }

    #[test]
    fn component_embeds_interface_fields() {
        // An #[interface] field becomes a manifest group spliced from the
        // field type's VerylInterface constants, and resolves under its
        // name.
        let ok = expand_derive(parse_quote! {
            struct C {
                clk: ClockPort,
                #[interface]
                axi: AxiMon,
                #[interface]
                axi2: AxiMon,
                loose: InputPort,
            }
        })
        .unwrap()
        .to_string();
        for expected in [
            "as :: veryl_component :: VerylInterface > :: INTERFACE_PATH",
            "as :: veryl_component :: VerylInterface > :: MEMBERS_JSON",
            r#":: resolve (ctx , "axi")"#,
            r#":: resolve (ctx , "axi2")"#,
        ] {
            assert!(ok.contains(expected), "missing {expected}: {ok}");
        }

        // The marker takes no arguments; the binding lives on the type.
        let err = expand_derive(parse_quote! {
            struct C {
                clk: ClockPort,
                #[interface(group = "axi")]
                axi: AxiMon,
            }
        })
        .unwrap_err()
        .to_string();
        assert!(err.contains("takes no arguments"), "{err}");

        // The old struct-level binding points at #[derive(VerylInterface)].
        let err = expand_derive(parse_quote! {
            #[interface(group = "axi", path = "$std::axi4_if", modport = "monitor")]
            struct C {
                clk: ClockPort,
            }
        })
        .unwrap_err()
        .to_string();
        assert!(err.contains("#[derive(VerylInterface)]"), "{err}");
    }

    #[test]
    fn interface_derive_emits_members_and_resolve() {
        let ok = expand_interface(parse_quote! {
            #[interface(path = "$std::axi4_lite_if", modport = "monitor")]
            struct AxiMon {
                /// Write-address valid.
                awvalid: InputPort,
                r#in: InputPort,
                resp: OutputPort,
            }
        })
        .unwrap()
        .to_string();
        for expected in [
            r#"INTERFACE_PATH : & 'static str = "$std::axi4_lite_if""#,
            r#"MODPORT : & 'static str = "monitor""#,
        ] {
            assert!(ok.contains(expected), "missing {expected}: {ok}");
        }
        let members = r#"[{"member":"awvalid","dir":"input","doc":"Write-address valid."},{"member":"in","dir":"input"},{"member":"resp","dir":"output"}]"#;
        assert!(ok.contains(&members.replace('"', "\\\"")), "{ok}");
    }

    #[test]
    fn interface_derive_rejects_bad_declarations() {
        let err = expand_interface(parse_quote! {
            #[interface(path = "$std::axi4_if")]
            struct A {
                awvalid: InputPort,
            }
        })
        .unwrap_err()
        .to_string();
        assert!(err.contains("needs `#[interface(modport = ..)]`"), "{err}");

        let err = expand_interface(parse_quote! {
            #[interface(path = "$std::axi4_if", modport = "monitor")]
            struct A {
                clk: ClockPort,
            }
        })
        .unwrap_err()
        .to_string();
        assert!(err.contains("InputPort or OutputPort fields"), "{err}");

        let err = expand_interface(parse_quote! {
            #[interface(path = "$std::axi4_if", modport = "monitor")]
            struct A {
                #[port(member = "awvalid")]
                renamed: InputPort,
            }
        })
        .unwrap_err()
        .to_string();
        assert!(
            err.contains("`#[port]` is not supported on interface members"),
            "{err}"
        );
    }

    #[test]
    fn rejects_raw_name_overrides() {
        let err = expand_derive(parse_quote! {
            struct C {
                clk: ClockPort,
                #[port(name = "r#in")]
                a: InputPort,
            }
        })
        .unwrap_err()
        .to_string();
        assert!(
            err.contains("port name must be written without the `r#` prefix"),
            "{err}"
        );

        let err = expand_derive(parse_quote! {
            struct C {
                clk: ClockPort,
                #[param(name = "r#type")]
                width: u64,
            }
        })
        .unwrap_err()
        .to_string();
        assert!(
            err.contains("parameter name must be written without the `r#` prefix"),
            "{err}"
        );
    }

    #[test]
    fn rejects_dotted_names() {
        let err = expand_derive(parse_quote! {
            struct C {
                clk: ClockPort,
                #[param(name = "A.B")]
                width: u64,
            }
        })
        .unwrap_err()
        .to_string();
        assert!(err.contains("parameter name cannot contain `.`"), "{err}");

        let err = expand_derive(parse_quote! {
            struct C {
                clk: ClockPort,
                #[port(name = "a.b")]
                d: InputPort,
            }
        })
        .unwrap_err()
        .to_string();
        assert!(err.contains("port name cannot contain `.`"), "{err}");
    }

    #[test]
    fn rejects_duplicate_port_names() {
        let err = expand_derive(parse_quote! {
            struct C {
                clk: ClockPort,
                #[port(name = "x")]
                a: InputPort,
                #[port(name = "x")]
                b: InputPort,
            }
        })
        .unwrap_err()
        .to_string();
        assert!(err.contains("duplicate port name `x`"), "{err}");

        // Groups share the connection-item namespace with loose ports.
        let err = expand_derive(parse_quote! {
            struct C {
                clk: ClockPort,
                #[interface]
                axi: AxiMon,
                #[port(name = "axi")]
                d: InputPort,
            }
        })
        .unwrap_err()
        .to_string();
        assert!(err.contains("duplicate port name `axi`"), "{err}");
    }

    #[test]
    fn rejects_string_returns() {
        let err = method_err(parse_quote! {
            fn f(&mut self, ctx: &mut SimCtx) -> Result<String> { todo!() }
        });
        assert!(err.contains("string return values are not supported"));
        let err = method_err(parse_quote! {
            fn f(&mut self, ctx: &mut SimCtx) -> Result<&str> { todo!() }
        });
        assert!(err.contains("string return values are not supported"));
    }

    #[test]
    fn rejects_option_returns() {
        let err = method_err(parse_quote! {
            fn f(&mut self, ctx: &mut SimCtx) -> Option<u64> { todo!() }
        });
        assert!(err.contains("not `Option`"));
    }

    #[test]
    fn accepts_result_alias_and_records_ret() {
        let mut func: syn::ImplItemFn = parse_quote! {
            fn f(&mut self, ctx: &mut SimCtx) -> MyResult<u32> { todo!() }
        };
        let spec = parse_method(&mut func).unwrap();
        assert_eq!(spec.ret.as_deref(), Some("u32"));

        let mut func: syn::ImplItemFn = parse_quote! {
            fn f(&mut self, ctx: &mut SimCtx) -> Result<()> { todo!() }
        };
        assert!(parse_method(&mut func).unwrap().ret.is_none());
    }

    #[test]
    fn accepts_bare_value_argument() {
        // A `Value` argument's width is inferred from the call site, so it
        // needs no declaration.
        let mut func: syn::ImplItemFn = parse_quote! {
            fn f(&mut self, ctx: &mut SimCtx, v: &Value) -> Result<()> { todo!() }
        };
        let spec = parse_method(&mut func).unwrap();
        assert_eq!(spec.args.len(), 1);
        assert_eq!(spec.args[0].ty, "value");
    }

    #[test]
    fn rejects_bare_value_return() {
        let err = method_err(parse_quote! {
            fn f(&mut self, ctx: &mut SimCtx) -> Result<Value> { todo!() }
        });
        assert!(
            err.contains("`Value` return must declare its width"),
            "{err}"
        );
    }

    #[test]
    fn accepts_value_return_with_declared_width() {
        let mut func: syn::ImplItemFn = parse_quote! {
            #[ret_width(WIDTH)]
            fn f(&mut self, ctx: &mut SimCtx, v: &Value) -> Result<Value> { todo!() }
        };
        let spec = parse_method(&mut func).unwrap();
        assert_eq!(spec.ret.as_deref(), Some("value"));
        // Width expressions are stored as structured JSON: a parameter name
        // is a quoted string.
        assert_eq!(spec.ret_width.as_deref(), Some(r#""WIDTH""#));
        assert_eq!(spec.args.len(), 1);
        assert_eq!(spec.args[0].ty, "value");
    }

    #[test]
    fn width_expression_is_emitted_as_structured_json() {
        let mut func: syn::ImplItemFn = parse_quote! {
            #[ret_width(WIDTH * 2 + 8)]
            fn f(&mut self, ctx: &mut SimCtx) -> Result<Value> { todo!() }
        };
        let spec = parse_method(&mut func).unwrap();
        assert_eq!(
            spec.ret_width.as_deref(),
            Some(r#"{"op":"+","lhs":{"op":"*","lhs":"WIDTH","rhs":2},"rhs":8}"#)
        );
    }

    #[test]
    fn raw_identifiers_are_unrawed_everywhere() {
        // Method and argument names drop `r#` in the manifest and dispatch,
        // matching the port/param fields.
        let mut func: syn::ImplItemFn = parse_quote! {
            fn r#move(&mut self, ctx: &mut SimCtx, r#ref: u64) -> Result<u64> { todo!() }
        };
        let spec = parse_method(&mut func).unwrap();
        let json = method_json(&spec);
        assert!(json.contains(r#""name":"move""#), "{json}");
        assert!(json.contains(r#""name":"ref""#), "{json}");

        // Width expressions reference the unrawed declaration names.
        let mut func: syn::ImplItemFn = parse_quote! {
            #[ret_width(r#virtual.WIDTH * r#override)]
            fn f(&mut self, ctx: &mut SimCtx) -> Result<Value> { todo!() }
        };
        let spec = parse_method(&mut func).unwrap();
        assert_eq!(
            spec.ret_width.as_deref(),
            Some(r#"{"op":"*","lhs":"virtual.WIDTH","rhs":"override"}"#)
        );
    }

    #[test]
    fn rejects_unsupported_width_operator() {
        let err = method_err(parse_quote! {
            #[ret_width(WIDTH % 2)]
            fn f(&mut self, ctx: &mut SimCtx) -> Result<Value> { todo!() }
        });
        assert!(err.contains("unsupported width operator"), "{err}");
    }

    #[test]
    fn rejects_non_ctx_second_argument() {
        let err = method_err(parse_quote! {
            fn helper(&mut self, x: u64) -> Result<u64> { todo!() }
        });
        assert!(err.contains("separate impl block"));
        let err = method_err(parse_quote! {
            fn f(&mut self) -> Result<u64> { todo!() }
        });
        assert!(err.contains("separate impl block"));
    }

    #[test]
    fn checks_hook_signatures() {
        let ok: syn::ImplItemFn = parse_quote! {
            fn on_clock(&mut self, ctx: &mut SimCtx) -> Result<()> { todo!() }
        };
        assert!(check_hook_signature(&ok).is_ok());

        let ok: syn::ImplItemFn = parse_quote! {
            fn on_build(&mut self, ctx: &mut veryl_component::BuildCtx) -> Result<()> { todo!() }
        };
        assert!(check_hook_signature(&ok).is_ok());

        let wrong_ctx: syn::ImplItemFn = parse_quote! {
            fn on_clock(&mut self, ctx: &mut BuildCtx) -> Result<()> { todo!() }
        };
        let err = check_hook_signature(&wrong_ctx).unwrap_err().to_string();
        assert!(err.contains("SimCtx"));

        let missing_ctx: syn::ImplItemFn = parse_quote! {
            fn on_init(&mut self) -> Result<()> { todo!() }
        };
        assert!(check_hook_signature(&missing_ctx).is_err());

        let extra_arg: syn::ImplItemFn = parse_quote! {
            fn on_reset(&mut self, ctx: &mut SimCtx, extra: u64) -> Result<()> { todo!() }
        };
        assert!(check_hook_signature(&extra_arg).is_err());
    }
}
