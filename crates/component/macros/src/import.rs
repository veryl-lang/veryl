//! `import!`: expands a package or an interface marked `#[export(rust)]`
//! as a Rust module, from the description Veryl writes into the crate
//! (`veryl.types.json`).
//!
//! The description's types mirror `veryl_metadata::export_description`.

use crate::case;
use crate::modport::{self, Direction, FieldSpec};
use proc_macro2::{Literal, Span, TokenStream as TokenStream2};
use quote::quote;
use serde::Deserialize;
use std::collections::{BTreeMap, HashMap};
use syn::ext::IdentExt;
use syn::parse::{Parse, ParseStream};
use syn::{Ident, Path, Token};

const FILE: &str = "veryl.types.json";
const VERSION: u32 = 1;

#[derive(Deserialize)]
struct Description {
    version: u32,
    packages: BTreeMap<String, Package>,
    interfaces: BTreeMap<String, Interface>,
}

#[derive(Deserialize)]
struct Package {
    consts: Vec<Const>,
    types: Vec<TypeAlias>,
    enums: Vec<Enum>,
    structs: Vec<Struct>,
}

#[derive(Deserialize)]
struct Const {
    name: String,
    r#type: ExportType,
    value: ExportValue,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum ExportValue {
    Bits {
        width: u32,
        signed: bool,
        value: String,
    },
    Variant {
        package: String,
        r#enum: String,
        variant: String,
    },
    Struct {
        package: String,
        name: String,
        fields: Vec<FieldValue>,
    },
    Array(Vec<ExportValue>),
    String(String),
}

#[derive(Deserialize)]
struct FieldValue {
    name: String,
    value: ExportValue,
}

#[derive(Deserialize)]
struct TypeAlias {
    name: String,
    r#type: ExportType,
}

#[derive(Deserialize)]
struct Enum {
    name: String,
    width: u32,
    variants: Vec<Variant>,
}

#[derive(Deserialize)]
struct Variant {
    name: String,
    value: String,
}

#[derive(Deserialize)]
struct Struct {
    name: String,
    fields: Vec<StructField>,
}

#[derive(Deserialize)]
struct StructField {
    name: String,
    r#type: ExportType,
}

#[derive(Deserialize)]
struct Interface {
    modports: Vec<Modport>,
}

#[derive(Deserialize)]
struct Modport {
    name: String,
    members: Vec<Member>,
}

#[derive(Deserialize)]
struct Member {
    name: String,
    direction: Direction,
    r#type: ExportType,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum ExportType {
    Bits { width: u32, signed: bool },
    Clock,
    Reset,
    String,
    User { package: String, name: String },
    Array { len: u32, elem: Box<ExportType> },
}

/// `name` or `name, use(package = path, ..)`.
pub struct ImportInput {
    name: Ident,
    uses: Vec<(Ident, Path)>,
}

impl Parse for ImportInput {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let name = Ident::parse_any(input)?;
        let mut uses = Vec::new();
        if input.parse::<Option<Token![,]>>()?.is_some() && !input.is_empty() {
            input.parse::<Token![use]>()?;
            let content;
            syn::parenthesized!(content in input);
            while !content.is_empty() {
                let package = Ident::parse_any(&content)?;
                content.parse::<Token![=]>()?;
                let path: Path = content.parse()?;
                uses.push((package, path));
                if content.parse::<Option<Token![,]>>()?.is_none() {
                    break;
                }
            }
            input.parse::<Option<Token![,]>>()?;
        }
        Ok(Self { name, uses })
    }
}

pub fn expand(input: ImportInput) -> syn::Result<TokenStream2> {
    let span = input.name.span();
    let err = |msg: String| syn::Error::new(span, msg);

    let dir = std::env::var("CARGO_MANIFEST_DIR")
        .map_err(|_| err("`CARGO_MANIFEST_DIR` is not set: build the crate with cargo".into()))?;
    let path = std::path::Path::new(&dir).join(FILE);
    let text = std::fs::read_to_string(&path).map_err(|_| {
        err(format!(
            "`{}` is not found: run `veryl build` first to describe the packages and interfaces marked `#[export(rust)]`",
            path.display()
        ))
    })?;
    let description: Description = serde_json::from_str(&text)
        .map_err(|e| err(format!("`{}` cannot be read: {e}", path.display())))?;
    if description.version != VERSION {
        return Err(err(format!(
            "`{}` is of version {}, which this veryl-component does not read (version {VERSION}): update veryl or veryl-component",
            path.display(),
            description.version
        )));
    }

    let name = input.name.to_string().trim_start_matches("r#").to_string();
    let mut uses = HashMap::new();
    for (package, path) in &input.uses {
        let package = package.to_string().trim_start_matches("r#").to_string();
        uses.insert(package, path.clone());
    }

    let module = case::ident(&case::snake(&name));
    let ctx = Ctx {
        current: name.clone(),
        uses,
        span,
    };
    let body = if let Some(package) = description.packages.get(&name) {
        ctx.package(package)?
    } else if let Some(interface) = description.interfaces.get(&name) {
        ctx.interface(interface)?
    } else {
        let available: Vec<_> = description
            .packages
            .keys()
            .chain(description.interfaces.keys())
            .map(|x| format!("`{x}`"))
            .collect();
        return Err(err(format!(
            "`{name}` is not a package or an interface marked `#[export(rust)]` (described: {})",
            if available.is_empty() {
                "none".to_string()
            } else {
                available.join(", ")
            }
        )));
    };

    Ok(quote! {
        #[allow(clippy::all)]
        pub mod #module {
            // Rebuilds the crate when the description changes.
            const _: &[u8] = ::core::include_bytes!(::core::concat!(
                ::core::env!("CARGO_MANIFEST_DIR"),
                "/veryl.types.json"
            ));

            #body
        }
    })
}

/// The default value of a type: zeros, or an enum's variant of value 0.
/// An array is built element by element, since an array longer than 32 has
/// no `Default`.
fn default_expr(ty: &ExportType) -> TokenStream2 {
    match ty {
        ExportType::Array { elem, .. } => {
            let elem = default_expr(elem);
            quote!(::core::array::from_fn(|_| #elem))
        }
        _ => quote!(::core::default::Default::default()),
    }
}

struct Ctx {
    /// The package being expanded, whose types are named as they are.
    current: String,
    uses: HashMap<String, Path>,
    span: Span,
}

/// Names converted to Rust's conventions, checked for collisions.
#[derive(Default)]
struct Names(HashMap<String, String>);

impl Names {
    fn insert(&mut self, rust: &str, veryl: &str, span: Span) -> syn::Result<()> {
        match self.0.insert(rust.to_string(), veryl.to_string()) {
            Some(other) => Err(syn::Error::new(
                span,
                format!("`{other}` and `{veryl}` both become `{rust}` in Rust"),
            )),
            None => Ok(()),
        }
    }
}

impl Ctx {
    fn err(&self, msg: impl Into<String>) -> syn::Error {
        syn::Error::new(self.span, msg.into())
    }

    /// The Rust type of a description type, and its width as an
    /// expression.
    fn rust_type(&self, ty: &ExportType) -> syn::Result<(TokenStream2, TokenStream2)> {
        let krate = quote!(::veryl_component);
        Ok(match ty {
            ExportType::Bits { width, signed } => {
                let rust = match (*width, *signed) {
                    (1, false) => quote!(bool),
                    (..=8, false) => quote!(u8),
                    (..=16, false) => quote!(u16),
                    (..=32, false) => quote!(u32),
                    (..=64, false) => quote!(u64),
                    (..=128, false) => quote!(u128),
                    (_, false) => quote!(#krate::num_bigint::BigUint),
                    (..=8, true) => quote!(i8),
                    (..=16, true) => quote!(i16),
                    (..=32, true) => quote!(i32),
                    (..=64, true) => quote!(i64),
                    (..=128, true) => quote!(i128),
                    (_, true) => quote!(#krate::num_bigint::BigInt),
                };
                let width = Literal::u32_unsuffixed(*width);
                (rust, quote!(#width))
            }
            ExportType::User { package, name } => {
                let ty = case::ident(&case::upper_camel(name));
                let rust = if *package == self.current {
                    quote!(#ty)
                } else if let Some(path) = self.uses.get(package) {
                    quote!(#path::#ty)
                } else {
                    return Err(self.err(format!(
                        "`{}::{name}` is used: give where package `{package}` is imported with `use({package} = <path>)`",
                        package
                    )));
                };
                let width = quote!(<#rust as #krate::FixedWidth>::WIDTH);
                (rust, width)
            }
            ExportType::Array { len, elem } => {
                let (elem, width) = self.rust_type(elem)?;
                let len = Literal::usize_unsuffixed(*len as usize);
                (quote!([#elem; #len]), quote!((#len * #width)))
            }
            ExportType::String => (quote!(&'static str), quote!(0)),
            ExportType::Clock | ExportType::Reset => {
                return Err(self.err("a clock or a reset has no Rust type"));
            }
        })
    }

    fn package(&self, package: &Package) -> syn::Result<TokenStream2> {
        let mut items = Vec::new();
        let mut consts = Names::default();
        let mut types = Names::default();

        for x in &package.consts {
            let name = case::screaming_snake(&x.name);
            consts.insert(&name, &x.name, self.span)?;
            items.push(self.const_item(&case::ident(&name), &x.r#type, &x.value)?);
        }
        for x in &package.types {
            let name = case::upper_camel(&x.name);
            types.insert(&name, &x.name, self.span)?;
            let ident = case::ident(&name);
            let (ty, _) = self.rust_type(&x.r#type)?;
            items.push(quote!(pub type #ident = #ty;));
        }
        for x in &package.enums {
            let name = case::upper_camel(&x.name);
            types.insert(&name, &x.name, self.span)?;
            items.push(self.enum_item(&case::ident(&name), x)?);
        }
        for x in &package.structs {
            let name = case::upper_camel(&x.name);
            types.insert(&name, &x.name, self.span)?;
            items.push(self.struct_item(&case::ident(&name), x)?);
        }
        Ok(quote!(#(#items)*))
    }

    fn const_item(
        &self,
        ident: &Ident,
        ty: &ExportType,
        value: &ExportValue,
    ) -> syn::Result<TokenStream2> {
        let (rust, _) = self.rust_type(ty)?;
        let expr = self.value_expr(value)?;
        Ok(quote!(pub const #ident: #rust = #expr;))
    }

    /// The path of a type of an exported package.
    fn user_path(&self, package: &str, name: &str) -> syn::Result<TokenStream2> {
        self.rust_type(&ExportType::User {
            package: package.to_string(),
            name: name.to_string(),
        })
        .map(|(x, _)| x)
    }

    /// The expression of a constant's value.
    fn value_expr(&self, value: &ExportValue) -> syn::Result<TokenStream2> {
        Ok(match value {
            ExportValue::Bits {
                width,
                signed,
                value,
            } => {
                if *width > 128 {
                    return Err(self.err("a constant wider than 128 bits has no Rust constant"));
                }
                let err = || self.err(format!("`{value}` is not a value of {width} bits"));
                if *width == 1 && !*signed {
                    if value != "0" {
                        quote!(true)
                    } else {
                        quote!(false)
                    }
                } else if let Some(magnitude) = value.strip_prefix('-') {
                    let lit = Literal::u128_unsuffixed(magnitude.parse().map_err(|_| err())?);
                    quote!(-#lit)
                } else {
                    let lit = Literal::u128_unsuffixed(value.parse().map_err(|_| err())?);
                    quote!(#lit)
                }
            }
            ExportValue::Variant {
                package,
                r#enum,
                variant,
            } => {
                let path = self.user_path(package, r#enum)?;
                let variant = case::ident(&case::upper_camel(variant));
                quote!(#path::#variant)
            }
            ExportValue::Struct {
                package,
                name,
                fields,
            } => {
                let path = self.user_path(package, name)?;
                let mut items = Vec::new();
                for f in fields {
                    let field = case::ident(&case::snake(&f.name));
                    let value = self.value_expr(&f.value)?;
                    items.push(quote!(#field: #value));
                }
                quote!(#path { #(#items,)* })
            }
            ExportValue::String(x) => {
                let lit = Literal::string(x);
                quote!(#lit)
            }
            ExportValue::Array(elems) => {
                let elems = elems
                    .iter()
                    .map(|x| self.value_expr(x))
                    .collect::<syn::Result<Vec<_>>>()?;
                quote!([#(#elems,)*])
            }
        })
    }

    fn enum_item(&self, ident: &Ident, x: &Enum) -> syn::Result<TokenStream2> {
        let krate = quote!(::veryl_component);
        let mut names = Names::default();
        let mut variants = Vec::new();
        let mut values = Vec::new();
        for v in &x.variants {
            let name = case::upper_camel(&v.name);
            names.insert(&name, &v.name, self.span)?;
            let value: u128 = v
                .value
                .parse()
                .map_err(|_| self.err(format!("`{}` is not a value of `{ident}`", v.value)))?;
            variants.push(case::ident(&name));
            values.push(Literal::u128_unsuffixed(value));
        }
        let default = values
            .iter()
            .position(|x| x.to_string() == "0")
            .unwrap_or(0);
        let default = variants.get(default).map(|v| {
            quote! {
                impl ::core::default::Default for #ident {
                    fn default() -> Self {
                        Self::#v
                    }
                }
            }
        });
        if x.width > 128 {
            return Err(self.err(format!(
                "`{ident}` is wider than 128 bits, which no enum discriminant of Rust holds"
            )));
        }
        // The variants carry Veryl's values, as SystemVerilog's labels do.
        let repr = match x.width {
            0..=8 => quote!(u8),
            9..=16 => quote!(u16),
            17..=32 => quote!(u32),
            33..=64 => quote!(u64),
            _ => quote!(u128),
        };
        let width = Literal::u32_unsuffixed(x.width);
        let enum_name = ident.to_string();
        Ok(quote! {
            #[repr(#repr)]
            #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
            pub enum #ident {
                #(#variants = #values,)*
            }

            #default

            impl #krate::FixedWidth for #ident {
                const WIDTH: u32 = #width;
            }

            impl #krate::Packed for #ident {
                fn fits(width: u32) -> bool {
                    width == #width
                }

                fn unpack(words: &[u64], offset: u32, width: u32) -> #krate::Result<Self> {
                    let value = <#repr as #krate::Packed>::unpack(words, offset, width)?;
                    match value {
                        #(#values => ::core::result::Result::Ok(Self::#variants),)*
                        _ => #krate::bail!("{} is no value of enum `{}`", value, #enum_name),
                    }
                }

                fn pack(&self, words: &mut [u64], offset: u32, width: u32) {
                    <#repr as #krate::Packed>::pack(&(*self as #repr), words, offset, width);
                }
            }

            impl ::core::convert::From<#ident> for #krate::Value {
                fn from(x: #ident) -> Self {
                    #krate::Packed::to_value(&x, #width)
                }
            }
        })
    }

    fn struct_item(&self, ident: &Ident, x: &Struct) -> syn::Result<TokenStream2> {
        let krate = quote!(::veryl_component);
        let mut names = Names::default();
        let mut fields = Vec::new();
        let mut types = Vec::new();
        let mut widths = Vec::new();
        let mut veryl_names = Vec::new();
        let mut defaults = Vec::new();
        for f in &x.fields {
            let name = case::snake(&f.name);
            names.insert(&name, &f.name, self.span)?;
            let (ty, width) = self.rust_type(&f.r#type)?;
            fields.push(case::ident(&name));
            types.push(ty);
            widths.push(width);
            veryl_names.push(f.name.clone());
            defaults.push(default_expr(&f.r#type));
        }
        let struct_name = ident.to_string();
        Ok(quote! {
            #[derive(Clone, Debug, PartialEq, Eq, Hash)]
            pub struct #ident {
                #(pub #fields: #types,)*
            }

            impl ::core::default::Default for #ident {
                fn default() -> Self {
                    Self {
                        #(#fields: #defaults,)*
                    }
                }
            }

            impl #krate::FixedWidth for #ident {
                const WIDTH: u32 = 0 #(+ #widths)*;
            }

            impl #krate::Packed for #ident {
                fn fits(width: u32) -> bool {
                    width == <Self as #krate::FixedWidth>::WIDTH
                }

                #[allow(unused_variables, unused_mut)]
                fn unpack(words: &[u64], offset: u32, width: u32) -> #krate::Result<Self> {
                    // The first field is the most significant.
                    let mut pos = offset + <Self as #krate::FixedWidth>::WIDTH;
                    ::core::result::Result::Ok(Self {
                        #(#fields: {
                            pos -= #widths;
                            <#types as #krate::Packed>::unpack(words, pos, #widths).map_err(|e| {
                                e.context(::std::format!(
                                    "field `{}` of struct `{}`",
                                    #veryl_names,
                                    #struct_name
                                ))
                            })?
                        },)*
                    })
                }

                #[allow(unused_variables, unused_mut)]
                fn pack(&self, words: &mut [u64], offset: u32, width: u32) {
                    let mut pos = offset + <Self as #krate::FixedWidth>::WIDTH;
                    #(
                        pos -= #widths;
                        #krate::Packed::pack(&self.#fields, words, pos, #widths);
                    )*
                }
            }

            impl ::core::convert::From<#ident> for #krate::Value {
                fn from(x: #ident) -> Self {
                    #krate::Packed::to_value(&x, <#ident as #krate::FixedWidth>::WIDTH)
                }
            }
        })
    }

    fn interface(&self, interface: &Interface) -> syn::Result<TokenStream2> {
        let mut items = Vec::new();
        let mut types = Names::default();
        for x in &interface.modports {
            let name = case::upper_camel(&x.name);
            types.insert(&name, &x.name, self.span)?;
            let ident = case::ident(&name);

            let mut names = Names::default();
            let mut specs = Vec::new();
            for m in &x.members {
                // A clock or a reset is reached by the code built on it, not
                // read as a value.
                if matches!(m.r#type, ExportType::Clock | ExportType::Reset) {
                    continue;
                }
                let name = case::snake(&m.name);
                names.insert(&name, &m.name, self.span)?;
                let (ty, _) = self.rust_type(&m.r#type)?;
                specs.push(FieldSpec {
                    ident: case::ident(&name),
                    ty: syn::parse2(ty)?,
                    direction: m.direction,
                    name: m.name.clone(),
                });
            }

            let fields = specs.iter().map(|f| {
                let (ident, ty) = (&f.ident, &f.ty);
                quote!(pub #ident: #ty)
            });
            let doc = format!("The modport `{}`.", x.name);
            let item = syn::parse2(quote! {
                #[doc = #doc]
                pub struct #ident {
                    #(#fields,)*
                }
            })?;
            items.push(modport::expand(item, &specs));

            // The members' values, without the bound members.
            let name = ident.to_string();
            let idents: Vec<_> = specs.iter().map(|f| &f.ident).collect();
            let names: Vec<_> = specs.iter().map(|f| f.ident.to_string()).collect();
            items.push(quote! {
                impl ::core::fmt::Debug for #ident {
                    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
                        f.debug_struct(#name)
                            #(.field(#names, &self.#idents))*
                            .finish()
                    }
                }
            });
        }
        Ok(quote!(#(#items)*))
    }
}
