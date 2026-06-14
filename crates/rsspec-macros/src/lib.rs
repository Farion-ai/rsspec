//! Proc-macro layer for rsspec — implicit fixture parameters.
//!
//! `describe!` parses its whole block. `before_all!(name: T = expr)` declares a
//! named, typed fixture (stored by raw type); `it!("…", { … })` bodies — and
//! nested `context!` blocks — reference those fixtures by bare name, with the
//! `&T` read injected by this macro. The parameter is implicit; the return type
//! is explicit. Nested containers inherit the enclosing scope's fixtures.

use proc_macro::TokenStream;
use proc_macro2::{Span, TokenStream as TokenStream2};
use quote::quote;
use syn::parse::{Parse, ParseStream};
use syn::{parse2, parse_macro_input, Block, Expr, Ident, LitStr, Macro, Stmt, Token, Type};

/// A fixture in scope: its user-written name (carrying its span) and declared type.
#[derive(Clone)]
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

/// `describe!("name", { … })` — a group of specs and fixtures.
#[proc_macro]
pub fn describe(input: TokenStream) -> TokenStream {
    container_entry(input, false, false)
}
/// Focused group — only focused groups/specs run.
#[proc_macro]
pub fn fdescribe(input: TokenStream) -> TokenStream {
    container_entry(input, true, false)
}
/// Pending group — registered but never executed.
#[proc_macro]
pub fn xdescribe(input: TokenStream) -> TokenStream {
    container_entry(input, false, true)
}

fn container_entry(input: TokenStream, focused: bool, pending: bool) -> TokenStream {
    let DescribeInput { name, body } = parse_macro_input!(input as DescribeInput);
    match lower_block(&name, &body, focused, pending, &[]) {
        Ok(ts) => ts.into(),
        Err(e) => e.to_compile_error().into(),
    }
}

/// Lower a container block to `__rt::describe(name, …, || { … })`, threading the
/// `inherited` in-scope fixtures in so nested specs can read enclosing fixtures.
fn lower_block(
    name: &LitStr,
    body: &Block,
    focused: bool,
    pending: bool,
    inherited: &[Fixture],
) -> syn::Result<TokenStream2> {
    let mut fixtures = inherited.to_vec();
    let mut lowered = TokenStream2::new();
    for stmt in &body.stmts {
        lowered.extend(lower_stmt(stmt, &mut fixtures)?);
    }
    Ok(quote! {
        ::rsspec::__rt::describe(#name, #focused, #pending, || {
            #lowered
        });
    })
}

fn lower_stmt(stmt: &Stmt, fixtures: &mut Vec<Fixture>) -> syn::Result<TokenStream2> {
    let mac = match stmt {
        Stmt::Macro(m) => &m.mac,
        Stmt::Expr(Expr::Macro(m), _) => &m.mac,
        other => {
            return Err(syn::Error::new_spanned(
                other,
                "rsspec: expected a before_all!/it!/context!/… call",
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
        "it" | "specify" => lower_it(mac, &fixtures[..], "it"),
        "fit" | "fspecify" => lower_it(mac, &fixtures[..], "fit"),
        "xit" | "xspecify" => lower_it(mac, &fixtures[..], "xit"),
        "describe" | "context" | "when" => lower_nested(mac, &fixtures[..], false, false),
        "fdescribe" | "fcontext" | "fwhen" => lower_nested(mac, &fixtures[..], true, false),
        "xdescribe" | "xcontext" | "xwhen" => lower_nested(mac, &fixtures[..], false, true),
        other => Err(syn::Error::new_spanned(
            &mac.path,
            format!("rsspec: unsupported `{other}!` in a describe block"),
        )),
    }
}

fn lower_nested(
    mac: &Macro,
    inherited: &[Fixture],
    focused: bool,
    pending: bool,
) -> syn::Result<TokenStream2> {
    let DescribeInput { name, body } = parse2(mac.tokens.clone())?;
    lower_block(&name, &body, focused, pending, inherited)
}

// ---- before_all! / before_each! --------------------------------------------

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
    // The expr may read earlier fixtures by bare name — inject those reads.
    let body = wrap_reads(&fixtures[..], quote! { #expr });
    let reg = if scope {
        quote!(before_all)
    } else {
        quote!(before_each)
    };
    let out = quote! {
        ::rsspec::__rt::#reg(move || -> #ty { #body });
    };
    fixtures.push(Fixture { name, ty });
    Ok(out)
}

// ---- it! / fit! / xit! ------------------------------------------------------

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

fn lower_it(mac: &Macro, fixtures: &[Fixture], ctor: &str) -> syn::Result<TokenStream2> {
    let ItArgs { name, body } = parse2(mac.tokens.clone())?;
    let ctor = Ident::new(ctor, Span::call_site());
    let stmts = &body.stmts;
    let wrapped = wrap_reads(fixtures, quote! { #(#stmts)* });
    Ok(quote! {
        ::rsspec::__rt::#ctor(#name, move || { #wrapped });
    })
}

// ---- read injection ---------------------------------------------------------

/// Wrap `inner` in one `with_fixture::<T,_>(|name| …)` per in-scope fixture
/// (innermost = `inner`). The closure parameter reuses the fixture's declared
/// ident span, so the body's bare `name` resolves to it. A body that doesn't use
/// a given fixture just ignores the binding (`let _ = &name;` silences it).
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
