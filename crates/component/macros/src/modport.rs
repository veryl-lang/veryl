//! `#[modport]`: a struct declaring the members of an interface's modport
//! as value fields, implementing `Modport`. `import!` generates the same for
//! each modport of an exported interface.

use proc_macro2::{Span, TokenStream as TokenStream2};
use quote::{format_ident, quote};
use serde::Deserialize;
use syn::ext::IdentExt;
use syn::spanned::Spanned;
use syn::{Field, Fields, Ident, ItemStruct, LitStr, Type};

/// The direction of a modport member, from the component's side. It is
/// also the direction `veryl.types.json` describes.
#[derive(Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    Input,
    Output,
}

pub struct FieldSpec {
    pub ident: Ident,
    pub ty: Type,
    pub direction: Direction,
    /// The modport member the field binds to.
    pub name: String,
}

/// The name of the hidden field holding the bound members.
const HIDDEN: &str = "__veryl_modport";

pub fn expand_attribute(attr: TokenStream2, item: TokenStream2) -> syn::Result<TokenStream2> {
    if !attr.is_empty() {
        return Err(syn::Error::new(
            attr.span(),
            "`#[modport]` takes no arguments",
        ));
    }
    let mut item: ItemStruct = syn::parse2(item)?;
    if !item.generics.params.is_empty() {
        return Err(syn::Error::new(
            item.generics.span(),
            "a modport struct cannot be generic",
        ));
    }
    let Fields::Named(fields) = &mut item.fields else {
        return Err(syn::Error::new(
            item.span(),
            "a modport struct needs named fields",
        ));
    };
    let mut specs = Vec::new();
    for field in fields.named.iter_mut() {
        specs.push(take_direction(field)?);
    }
    Ok(expand(item, &specs))
}

/// Removes the direction attribute (`#[input]` / `#[output]`, optionally with
/// `(name = "..")` naming the member) of a field.
fn take_direction(field: &mut Field) -> syn::Result<FieldSpec> {
    let ident = field.ident.clone().expect("named field");
    let mut direction = None;
    let mut name = None;
    let mut rest = Vec::new();
    for attr in field.attrs.drain(..) {
        let this = if attr.path().is_ident("input") {
            Direction::Input
        } else if attr.path().is_ident("output") {
            Direction::Output
        } else {
            rest.push(attr);
            continue;
        };
        if direction.replace(this).is_some() {
            return Err(syn::Error::new(attr.span(), "a field takes one direction"));
        }
        if let syn::Meta::List(_) = attr.meta {
            attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("name") {
                    let value: LitStr = meta.value()?.parse()?;
                    name = Some(value.value());
                    Ok(())
                } else {
                    Err(meta.error("expected `name = \"..\"`"))
                }
            })?;
        } else {
            attr.meta.require_path_only()?;
        }
    }
    field.attrs = rest;
    let Some(direction) = direction else {
        return Err(syn::Error::new(
            field.span(),
            "the field needs `#[input]` or `#[output]`",
        ));
    };
    Ok(FieldSpec {
        name: name.unwrap_or_else(|| ident.unraw().to_string()),
        ident,
        ty: field.ty.clone(),
        direction,
    })
}

/// The struct with its hidden field, and the implementation of `Modport`.
pub fn expand(mut item: ItemStruct, specs: &[FieldSpec]) -> TokenStream2 {
    let krate = quote!(::veryl_component);
    let name = item.ident.clone();
    let members = format_ident!("__VerylModport{}", name.unraw());
    let hidden = Ident::new(HIDDEN, Span::call_site());

    if let Fields::Named(fields) = &mut item.fields {
        fields.named.push(syn::parse_quote! {
            #[doc(hidden)]
            #hidden: #members
        });
    }

    let member_fields = specs.iter().map(|f| {
        let (ident, ty) = (&f.ident, &f.ty);
        match f.direction {
            Direction::Input => quote!(#ident: #krate::InputSignal<#ty>),
            Direction::Output => quote!(#ident: #krate::OutputSignal<#ty>),
        }
    });
    let binds = specs.iter().map(|f| {
        let (ident, ty, path) = (&f.ident, &f.ty, &f.name);
        match f.direction {
            Direction::Input => quote!(#ident: #krate::InputSignal::<#ty>::member(source, #path)?),
            Direction::Output => {
                quote!(#ident: #krate::OutputSignal::<#ty>::member(source, #path)?)
            }
        }
    });
    let defaults = specs.iter().map(|f| {
        let ident = &f.ident;
        quote!(#ident: ::core::default::Default::default())
    });
    let loads = specs
        .iter()
        .filter(|f| f.direction == Direction::Input)
        .map(|f| {
            let ident = &f.ident;
            quote!(self.#ident = self.#hidden.#ident.read(ctx)?;)
        });
    let stores = specs
        .iter()
        .filter(|f| f.direction == Direction::Output)
        .map(|f| {
            let ident = &f.ident;
            quote!(self.#hidden.#ident.store(ctx, &self.#ident);)
        });
    let methods = specs.iter().map(|f| {
        let ident = &f.ident;
        let field = ident.unraw();
        match f.direction {
            Direction::Input => {
                let load = format_ident!("load_{}", field);
                let doc = format!("Reads `{field}` alone into its field.");
                quote! {
                    #[doc = #doc]
                    #[inline]
                    pub fn #load(
                        &mut self,
                        ctx: &mut #krate::SimCtx,
                    ) -> #krate::Result<()> {
                        self.#ident = self.#hidden.#ident.read(ctx)?;
                        ::core::result::Result::Ok(())
                    }
                }
            }
            Direction::Output => {
                let force = format_ident!("force_{}", field);
                let assume = format_ident!("assume_{}", field);
                let force_doc = format!("Drives `{field}` at once, whatever was last driven.");
                let assume_doc =
                    format!("Takes `{field}` as the value last driven, without driving it.");
                quote! {
                    #[doc = #force_doc]
                    #[inline]
                    pub fn #force(&mut self, ctx: &mut #krate::SimCtx) {
                        self.#hidden.#ident.force(ctx, &self.#ident);
                    }

                    #[doc = #assume_doc]
                    #[inline]
                    pub fn #assume(&mut self) {
                        self.#hidden.#ident.assume(&self.#ident);
                    }
                }
            }
        }
    });
    let input_only = if specs.iter().all(|f| f.direction == Direction::Input) {
        quote!(impl #krate::InputOnly for #name {})
    } else {
        quote!()
    };

    quote! {
        #item

        #[doc(hidden)]
        #[allow(non_camel_case_types)]
        struct #members {
            #(#member_fields,)*
        }

        // The bound members say nothing a reader of the struct wants.
        impl ::core::fmt::Debug for #members {
            fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
                f.write_str("..")
            }
        }

        impl #krate::Modport for #name {
            fn bind<M: #krate::MemberSource + ?::core::marker::Sized>(
                source: &M,
            ) -> #krate::Result<Self> {
                ::core::result::Result::Ok(Self {
                    #(#defaults,)*
                    #hidden: #members { #(#binds,)* },
                })
            }

            #[allow(unused_variables)]
            fn load(&mut self, ctx: &mut #krate::SimCtx) -> #krate::Result<()> {
                #(#loads)*
                ::core::result::Result::Ok(())
            }

            #[allow(unused_variables)]
            fn store(&mut self, ctx: &mut #krate::SimCtx) {
                #(#stores)*
            }
        }

        #input_only

        impl #name {
            #(#methods)*
        }
    }
}
