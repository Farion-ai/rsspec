//! Proc-macro layer for rsspec — implicit fixture parameters.
//!
//! `describe!` parses its whole block. `before_all!(name: T = expr)` declares a
//! named, typed fixture (stored by raw type); `it!("…", { … })` bodies reference
//! those fixtures by bare name, with the `&T` read injected by this macro — the
//! parameter is implicit, the return type is explicit.

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::parse::{Parse, ParseStream};
use syn::{parse2, parse_macro_input, Block, Expr, Ident, LitStr, Macro, Stmt, Token, Type};

/// A fixture in scope: its user-written name (carrying its span) and declared type.
struct Fixture {
    name: Ident,
    ty: Type,
}

struct DescribeInput {
    name: LitStr,
    body: Block,
}

impl Parse for DescribeInput {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let name = input.parse()?;
        input.parse::<Token![,]>()?;
        let body = input.parse()?;
        Ok(Self { name, body })
    }
}

/// `describe!("name", { before_all!(…); it!(…); … })`.
#[proc_macro]
pub fn describe(input: TokenStream) -> TokenStream {
    let DescribeInput { name, body } = parse_macro_input!(input as DescribeInput);
    let mut fixtures: Vec<Fixture> = Vec::new();
    let mut lowered = TokenStream2::new();
    for stmt in &body.stmts {
        match lower_stmt(stmt, &mut fixtures) {
            Ok(ts) => lowered.extend(ts),
            Err(e) => return e.to_compile_error().into(),
        }
    }
    quote! {
        ::rsspec::__rt::describe(#name, false, false, || {
            #lowered
        });
    }
    .into()
}

fn lower_stmt(stmt: &Stmt, fixtures: &mut Vec<Fixture>) -> syn::Result<TokenStream2> {
    let mac = match stmt {
        Stmt::Macro(m) => &m.mac,
        Stmt::Expr(Expr::Macro(m), _) => &m.mac,
        other => {
            return Err(syn::Error::new_spanned(
                other,
                "rsspec: expected a before_all!/before_each!/it! call",
            ))
        }
    };
    let which = mac
        .path
        .get_ident()
        .map(Ident::to_string)
        .unwrap_or_default();
    match which.as_str() {
        "before_all" => lower_before(mac, fixtures, true),
        "before_each" => lower_before(mac, fixtures, false),
        "it" => lower_it(mac, &fixtures[..]),
        other => Err(syn::Error::new_spanned(
            &mac.path,
            format!("rsspec: unsupported `{other}!` here (MVP: before_all!/before_each!/it!)"),
        )),
    }
}

struct BeforeArgs {
    name: Ident,
    ty: Type,
    expr: Expr,
}

impl Parse for BeforeArgs {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let name = input.parse()?;
        input.parse::<Token![:]>()?;
        let ty = input.parse()?;
        input.parse::<Token![=]>()?;
        let expr = input.parse()?;
        Ok(Self { name, ty, expr })
    }
}

fn lower_before(
    mac: &Macro,
    fixtures: &mut Vec<Fixture>,
    scope: bool,
) -> syn::Result<TokenStream2> {
    let BeforeArgs { name, ty, expr } = parse2(mac.tokens.clone())?;
    let body = wrap_reads(&fixtures[..], quote! { #expr });
    let reg = if scope {
        quote! { before_all }
    } else {
        quote! { before_each }
    };
    let out = quote! {
        ::rsspec::__rt::#reg(move || -> #ty { #body });
    };
    fixtures.push(Fixture { name, ty });
    Ok(out)
}

struct ItArgs {
    name: LitStr,
    body: Block,
}

impl Parse for ItArgs {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let name = input.parse()?;
        input.parse::<Token![,]>()?;
        let body = input.parse()?;
        Ok(Self { name, body })
    }
}

fn lower_it(mac: &Macro, fixtures: &[Fixture]) -> syn::Result<TokenStream2> {
    let ItArgs { name, body } = parse2(mac.tokens.clone())?;
    let stmts = &body.stmts;
    let wrapped = wrap_reads(fixtures, quote! { #(#stmts)* });
    Ok(quote! {
        ::rsspec::__rt::it(#name, move || { #wrapped });
    })
}

/// Wrap `inner` in one `with_fixture::<T,_>(|name| …)` per in-scope fixture
/// (innermost = `inner`). The closure parameter reuses the fixture's declared
/// ident span, so the body's bare `name` resolves to it.
fn wrap_reads(fixtures: &[Fixture], inner: TokenStream2) -> TokenStream2 {
    let mut acc = inner;
    for f in fixtures.iter().rev() {
        let name = &f.name;
        let ty = &f.ty;
        acc = quote! {
            ::rsspec::__rt::with_fixture::<#ty, _>(|#name| {
                let _ = &#name;
                #acc
            })
        };
    }
    acc
}
