//! Procedural macros for the unified wire codec (`WireEncode`/`WireDecode`,
//! crate `lb-wire`).
//!
//! - [`macro@WireCodec`] — `#[derive(WireCodec)]` for named or tuple structs.
//!   Generates *only* the codec: `WireEncode` (field-order concatenation, with a
//!   summed `encoded_length`) and `WireDecode` (decode each field in order with a
//!   `()` context, then `Self { .. }` / `Self(..)`). The decode is infallible
//!   positional construction, so a newtype needing a fallible `try_from` keeps a
//!   hand-written impl. The well-known fixture is supplied separately by
//!   [`wire_fixtures!`]; because both codec traits require `WireExamples`, a
//!   derived type that lacks a `wire_fixtures!` does not compile.
//! - [`wire_fixtures!`] — the single source of fixtures. Emits the sealed
//!   `WireExamples` impl and a `#[cfg(test)]` round-trip test for any codec
//!   (derived, hand-written, primitive, or foreign). For codecs whose
//!   `WireDecode::Context` is not `()`, pass `context = <expr>` so the generated
//!   test can build a context.
//!
//! Generated code refers to the codec crate by the absolute path `::lb_wire::…`,
//! so the macros expand correctly in any crate that depends on `lb-wire` (the
//! crate itself uses `extern crate self as lb_wire;`).

use proc_macro::TokenStream;
use proc_macro2::{Span, TokenStream as TokenStream2};
use quote::quote;
use syn::{
    Data, DeriveInput, Expr, Fields, Ident, LitStr, Token, Type,
    parse::{Parse, ParseStream},
    parse_macro_input,
};

/// Derive `WireEncode` + `WireDecode` for a named or tuple struct with an
/// infallible positional decode and a `()` decode context.
///
/// Both codec traits require `WireExamples`, so a derived type must also pin its
/// well-known fixture with [`wire_fixtures!`] or it will not compile.
#[proc_macro_derive(WireCodec)]
pub fn derive_wire_codec(input: TokenStream) -> TokenStream {
    let parsed_input = parse_macro_input!(input as DeriveInput);
    match expand_derive(&parsed_input) {
        Ok(tokens) => tokens.into(),
        Err(err) => err.to_compile_error().into(),
    }
}

fn expand_derive(input: &DeriveInput) -> syn::Result<TokenStream2> {
    let ident = &input.ident;

    if !input.generics.params.is_empty() {
        return Err(syn::Error::new_spanned(
            &input.generics,
            "`#[derive(WireCodec)]` does not yet support generic types; use a \
             hand-written impl plus `wire_fixtures!`",
        ));
    }

    let Data::Struct(data) = &input.data else {
        return Err(syn::Error::new_spanned(
            ident,
            "`#[derive(WireCodec)]` can only be derived for structs (for now)",
        ));
    };

    let FieldLayout {
        field_types,
        encode_accessors,
        decode_bindings,
        constructor,
    } = field_layout(ident, &data.fields)?;

    Ok(quote! {
        #[automatically_derived]
        impl ::lb_wire::WireEncode for #ident {
            fn encoded_length(&self) -> usize {
                0usize #( + ::lb_wire::WireEncode::encoded_length(&#encode_accessors) )*
            }

            fn encode_into(&self, out: &mut ::std::vec::Vec<u8>) {
                #( ::lb_wire::WireEncode::encode_into(&#encode_accessors, out); )*
            }
        }

        #[automatically_derived]
        impl ::lb_wire::WireDecode for #ident {
            type Context = ();

            fn decode(
                input: &[u8],
                (): Self::Context,
            ) -> ::core::result::Result<(&[u8], Self), ::lb_wire::DecodeError> {
                #(
                    let (input, #decode_bindings) =
                        <#field_types as ::lb_wire::WireDecode>::decode(input, ())?;
                )*
                ::core::result::Result::Ok((input, #constructor))
            }
        }
    })
}

/// The per-field tokens the codec impls need: each field's type, the
/// `self.<field>` accessor `encode` reads, the binding `decode` introduces, and
/// how to rebuild `Self` from those bindings.
struct FieldLayout<'a> {
    field_types: Vec<&'a Type>,
    encode_accessors: Vec<TokenStream2>,
    decode_bindings: Vec<Ident>,
    constructor: TokenStream2,
}

/// Compute the [`FieldLayout`] for a struct. Handles named structs
/// (`Self { a, b }`) and tuple structs (`Self(a, b)`) alike; the decode is
/// always the infallible positional form, so a newtype that needs a fallible
/// `try_from` stays on `wire_fixtures!` for now.
fn field_layout<'a>(ident: &Ident, fields: &'a Fields) -> syn::Result<FieldLayout<'a>> {
    Ok(match fields {
        Fields::Named(named) => {
            let decode_bindings: Vec<Ident> = named
                .named
                .iter()
                .map(|field| field.ident.clone().expect("named field has an ident"))
                .collect();
            FieldLayout {
                field_types: named.named.iter().map(|field| &field.ty).collect(),
                encode_accessors: decode_bindings.iter().map(|id| quote!(self.#id)).collect(),
                constructor: quote!(Self { #(#decode_bindings),* }),
                decode_bindings,
            }
        }
        Fields::Unnamed(unnamed) => {
            let decode_bindings: Vec<Ident> = (0..unnamed.unnamed.len())
                .map(|i| Ident::new(&format!("field{i}"), Span::call_site()))
                .collect();
            FieldLayout {
                field_types: unnamed.unnamed.iter().map(|field| &field.ty).collect(),
                encode_accessors: (0..unnamed.unnamed.len())
                    .map(|i| {
                        let index = syn::Index::from(i);
                        quote!(self.#index)
                    })
                    .collect(),
                constructor: quote!(Self( #(#decode_bindings),* )),
                decode_bindings,
            }
        }
        Fields::Unit => {
            return Err(syn::Error::new_spanned(
                ident,
                "`#[derive(WireCodec)]` cannot be derived for unit structs",
            ));
        }
    })
}

/// A single parsed well-known fixture: a value expression and its canonical wire
/// bytes (decoded from hex at macro-expansion time).
struct Fixture {
    value: Expr,
    bytes: Vec<u8>,
}

/// Render a [`Fixture`] into a `WireFixture { .. }` literal. The bytes were
/// decoded at expansion time, so they are emitted as a borrowed `&'static`
/// slice — no runtime hex decoding.
fn fixture_tokens(fixture: &Fixture) -> TokenStream2 {
    let value = &fixture.value;
    let bytes = &fixture.bytes;
    quote! {
        ::lb_wire::WireFixture {
            value: #value,
            bytes: ::std::borrow::Cow::Borrowed(&[ #(#bytes),* ]),
        }
    }
}

/// Attach well-known fixtures (and a round-trip test) to a hand-written codec.
///
/// For primitives, foreign types, newtypes, and the element types of the generic
/// blanket impls — anything `#[derive(WireCodec)]` cannot reach. Takes one or
/// more `value => "hex"` pairs. For a codec whose `WireDecode::Context` is not
/// `()`, prefix the pairs with `context = <expr>,` so the generated round-trip
/// test can build a context (the expression must not reference `Self`).
///
/// ```ignore
/// wire_fixtures!(u32, 0x0403_0201_u32 => "01020304");
/// wire_fixtures!(u8, 0x07_u8 => "07", 0x00_u8 => "00");
/// wire_fixtures!(EncapsulatedPart, context = NonZeroU64::new(3).unwrap(), value => "…");
/// ```
#[proc_macro]
pub fn wire_fixtures(input: TokenStream) -> TokenStream {
    let WireFixtureInput {
        ty,
        context,
        fixtures,
    } = parse_macro_input!(input as WireFixtureInput);
    let fixture_exprs = fixtures.iter().map(fixture_tokens);

    let sanitized: String = quote!(#ty)
        .to_string()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    let test_mod = Ident::new(&format!("__wire_fixture_{sanitized}"), Span::call_site());

    let round_trip = match context {
        Some(context) => quote! {
            ::lb_wire::assert_wire_fixtures_with::<#ty>(|| #context);
        },
        None => quote! {
            ::lb_wire::assert_wire_fixtures::<#ty>();
        },
    };

    quote! {
        #[automatically_derived]
        impl ::lb_wire::sealed::Sealed for #ty {}

        #[automatically_derived]
        impl ::lb_wire::WireExamples for #ty {
            fn fixtures() -> ::lb_wire::WireFixtures<Self> {
                [ #(#fixture_exprs),* ].into()
            }
        }

        #[cfg(test)]
        mod #test_mod {
            #[allow(unused_imports)]
            use super::*;

            #[test]
            fn wire_fixtures_round_trip() {
                #round_trip
            }
        }
    }
    .into()
}

/// Parsed input of [`wire_fixtures!`]: `Type, [context = <expr>,] value => "hex",
/// …` — at least one `value => "hex"` pair.
struct WireFixtureInput {
    ty: Type,
    context: Option<Expr>,
    fixtures: Vec<Fixture>,
}

impl Parse for WireFixtureInput {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let ty: Type = input.parse()?;
        input.parse::<Token![,]>()?;

        // Optional `context = <expr>,` for codecs whose decode context is not `()`.
        let context = if input.peek(Ident) && input.peek2(Token![=]) && !input.peek2(Token![==]) {
            let keyword: Ident = input.parse()?;
            if keyword != "context" {
                return Err(syn::Error::new_spanned(
                    &keyword,
                    "expected `context = <expr>` or a `value => \"hex\"` pair",
                ));
            }
            input.parse::<Token![=]>()?;
            let expr: Expr = input.parse()?;
            input.parse::<Token![,]>()?;
            Some(expr)
        } else {
            None
        };

        let mut fixtures = Vec::new();
        while !input.is_empty() {
            let value: Expr = input.parse()?;
            input.parse::<Token![=>]>()?;
            let lit: LitStr = input.parse()?;
            let bytes = hex::decode(lit.value()).map_err(|err| {
                syn::Error::new(lit.span(), format!("`bytes` is not valid hex: {err}"))
            })?;
            fixtures.push(Fixture { value, bytes });

            if input.is_empty() {
                break;
            }
            input.parse::<Token![,]>()?; // separator (a trailing comma ends the loop)
        }

        if fixtures.is_empty() {
            return Err(input.error("`wire_fixtures!` needs at least one `value => \"hex\"` pair"));
        }
        Ok(Self {
            ty,
            context,
            fixtures,
        })
    }
}
