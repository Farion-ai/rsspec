//! Proc-macro layer for rsspec — implicit fixture parameters.
//!
//! `describe!` parses its whole block. `before_all!(name: T = expr)` declares a
//! named, typed fixture (stored by raw type); `it!("…", { … })` bodies — and
//! nested `context!` blocks and `after_*!`/`just_before_each!` hooks — reference
//! those fixtures by bare name, with the `&T` read injected by this macro. The
//! parameter is implicit; the return type is explicit.

use proc_macro::TokenStream;
use proc_macro2::{Span, TokenStream as TokenStream2};
use quote::quote;
use syn::parse::{Parse, ParseStream};
use syn::punctuated::Punctuated;
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

// `context!`/`when!` (and their `f`/`x` variants) are interchangeable container
// entry points — aliases for `describe!`/`fdescribe!`/`xdescribe!`. Nested inside
// a `describe!` they are parsed as tokens (see `lower_stmt`); these handle the
// top-level invocations.
#[proc_macro]
pub fn context(input: TokenStream) -> TokenStream {
    container_entry(input, false, false)
}
#[proc_macro]
pub fn fcontext(input: TokenStream) -> TokenStream {
    container_entry(input, true, false)
}
#[proc_macro]
pub fn xcontext(input: TokenStream) -> TokenStream {
    container_entry(input, false, true)
}
#[proc_macro]
pub fn when(input: TokenStream) -> TokenStream {
    container_entry(input, false, false)
}
#[proc_macro]
pub fn fwhen(input: TokenStream) -> TokenStream {
    container_entry(input, true, false)
}
#[proc_macro]
pub fn xwhen(input: TokenStream) -> TokenStream {
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
        "after_each" => lower_hook(mac, &fixtures[..], "after_each"),
        "after_all" => lower_hook(mac, &fixtures[..], "after_all"),
        "just_before_each" => lower_hook(mac, &fixtures[..], "just_before_each"),
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

/// A `|param: &Ty|` read head, shared by the explicit before/hook forms.
struct ReadHead {
    param: Ident,
    in_ty: Type,
}

fn parse_read_head(input: ParseStream) -> syn::Result<ReadHead> {
    input.parse::<Token![|]>()?;
    let param = input.parse()?;
    input.parse::<Token![:]>()?;
    input.parse::<Token![&]>()?;
    let in_ty = input.parse()?;
    input.parse::<Token![|]>()?;
    Ok(ReadHead { param, in_ty })
}

enum BeforeArgs {
    /// `name: T = expr` — implicit declaration (the primary form).
    Named { name: Ident, ty: Type, expr: Expr },
    /// `|outer: &U| -> T { … }` — explicit read of an enclosing fixture, returns T.
    ReadReturn {
        param: Ident,
        in_ty: Type,
        out_ty: Type,
        body: Block,
    },
    /// `|outer: &U| { … }` — explicit read for side effects, returns ().
    ReadVoid {
        param: Ident,
        in_ty: Type,
        body: Block,
    },
    /// `{ … }` — a plain side-effect hook (no fixture), returns (); in-scope
    /// fixtures are read implicitly, like an `it!` block.
    Block(Block),
}

impl Parse for BeforeArgs {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        if input.peek(Token![|]) {
            let ReadHead { param, in_ty } = parse_read_head(input)?;
            if input.peek(Token![->]) {
                input.parse::<Token![->]>()?;
                let out_ty = input.parse()?;
                let body = input.parse()?;
                Ok(BeforeArgs::ReadReturn {
                    param,
                    in_ty,
                    out_ty,
                    body,
                })
            } else {
                Ok(BeforeArgs::ReadVoid {
                    param,
                    in_ty,
                    body: input.parse()?,
                })
            }
        } else if input.peek(syn::token::Brace) {
            Ok(BeforeArgs::Block(input.parse()?))
        } else {
            let name = input.parse()?;
            input.parse::<Token![:]>()?;
            let ty = input.parse()?;
            input.parse::<Token![=]>()?;
            let expr = input.parse()?;
            Ok(BeforeArgs::Named { name, ty, expr })
        }
    }
}

/// A best-effort syntactic key for a fixture type, used for the compile-time
/// same-type collision hint. This is token text, not type identity — `Vec<u8>`
/// and `std::vec::Vec<u8>` key differently. The exact check is the runtime
/// `TypeId` guard in `rsspec`'s `store_scope_setup_value`; this only catches the
/// common same-spelling case early, with a nicer diagnostic.
fn type_key(ty: &Type) -> String {
    quote!(#ty).to_string()
}

fn lower_before(
    mac: &Macro,
    fixtures: &mut Vec<Fixture>,
    scope: bool,
) -> syn::Result<TokenStream2> {
    let reg = if scope {
        quote!(before_all)
    } else {
        quote!(before_each)
    };
    match parse2::<BeforeArgs>(mac.tokens.clone())? {
        BeforeArgs::Named { name, ty, expr } => {
            // Best-effort compile-time hint: two in-scope fixtures whose types are
            // written the same way can't be told apart by an implicit read. The
            // exact backstop is the runtime `TypeId` guard (store_scope_setup_value);
            // this fires early with a clearer diagnostic for the common case.
            let key = type_key(&ty);
            if let Some(prev) = fixtures.iter().find(|f| type_key(&f.ty) == key) {
                let prev = &prev.name;
                return Err(syn::Error::new_spanned(
                    &ty,
                    format!(
                        "rsspec: a fixture of type `{key}` (`{prev}`) is already in \
                         scope; implicit reads can't disambiguate two fixtures of the \
                         same type — give this one a distinct type (a newtype works)"
                    ),
                ));
            }
            let out = if let Expr::Async(async_block) = &expr {
                // `name: T = async { … }` — drive the future on the suite runtime
                // and store its `T` output. Enclosing fixtures named in the body
                // are cloned out synchronously *before* the `async` block (a `&T`
                // can't be held across `.await`), then owned across awaits.
                let stmts = &async_block.block.stmts;
                let body = quote! { #(#stmts)* };
                let binds = clone_binds(&fixtures[..], &body);
                let areg = if scope {
                    quote!(async_before_all)
                } else {
                    quote!(async_before_each)
                };
                quote! {
                    ::rsspec::__rt::#areg::<_, _, #ty>(move || {
                        #binds
                        async move { #body }
                    });
                }
            } else {
                // The expr may read earlier fixtures by bare name — inject those reads.
                let body = wrap_reads(&fixtures[..], quote! { #expr });
                quote! {
                    ::rsspec::__rt::#reg(move || -> #ty { #body });
                }
            };
            fixtures.push(Fixture { name, ty });
            Ok(out)
        }
        // Explicit read forms pass straight through to the runtime's hook
        // dispatch: the reference is named, so there's no implicit injection and
        // no tracked fixture (downstream reads it explicitly too).
        BeforeArgs::ReadReturn {
            param,
            in_ty,
            out_ty,
            body,
        } => Ok(quote! {
            ::rsspec::__rt::#reg(move |#param: &#in_ty| -> #out_ty #body);
        }),
        BeforeArgs::ReadVoid { param, in_ty, body } => Ok(quote! {
            ::rsspec::__rt::#reg(move |#param: &#in_ty| #body);
        }),
        // Plain side-effect hook; in-scope fixtures read implicitly, returns ().
        BeforeArgs::Block(body) => {
            let stmts = &body.stmts;
            let wrapped = wrap_reads(&fixtures[..], quote! { #(#stmts)* });
            Ok(quote! {
                ::rsspec::__rt::#reg(move || { #wrapped });
            })
        }
    }
}

// ---- after_each! / after_all! / just_before_each! --------------------------

enum HookArgs {
    /// `|outer: &T| { … }` — explicit read; passes straight through.
    Read {
        param: Ident,
        ty: Box<Type>,
        body: Block,
    },
    /// `async { … }` — driven on the suite runtime; in-scope fixtures cloned in.
    Async(Block),
    /// `{ … }` — implicit fixtures resolved inside via injected reads.
    Block(Block),
}

impl Parse for HookArgs {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        if input.peek(Token![|]) {
            let ReadHead { param, in_ty } = parse_read_head(input)?;
            Ok(HookArgs::Read {
                param,
                ty: Box::new(in_ty),
                body: input.parse()?,
            })
        } else if input.peek(Token![async]) {
            input.parse::<Token![async]>()?;
            Ok(HookArgs::Async(input.parse()?))
        } else {
            Ok(HookArgs::Block(input.parse()?))
        }
    }
}

fn lower_hook(mac: &Macro, fixtures: &[Fixture], ctor: &str) -> syn::Result<TokenStream2> {
    let ctor_ident = Ident::new(ctor, Span::call_site());
    match parse2::<HookArgs>(mac.tokens.clone())? {
        HookArgs::Read { param, ty, body } => Ok(quote! {
            ::rsspec::__rt::#ctor_ident(move |#param: &#ty| #body);
        }),
        // `async { … }` — drive on the suite runtime. Enclosing fixtures named in
        // the body are cloned out synchronously before the `async` block (a `&T`
        // can't be held across `.await`), mirroring `before_all!(… = async …)`.
        HookArgs::Async(body) => {
            let stmts = &body.stmts;
            let body_ts = quote! { #(#stmts)* };
            let binds = clone_binds(fixtures, &body_ts);
            let areg = Ident::new(&format!("async_{ctor}"), Span::call_site());
            Ok(quote! {
                ::rsspec::__rt::#areg(move || {
                    #binds
                    async move { #body_ts }
                });
            })
        }
        HookArgs::Block(body) => {
            let stmts = &body.stmts;
            let wrapped = wrap_reads(fixtures, quote! { #(#stmts)* });
            Ok(quote! {
                ::rsspec::__rt::#ctor_ident(move || { #wrapped });
            })
        }
    }
}

// ---- it! / fit! / xit! + decorators ----------------------------------------

enum Decorator {
    Tags(Vec<Expr>),
    Retries(Expr),
    Timeout(Expr),
    MustPass(Expr),
}

/// The three `it!` body forms.
enum ItBody {
    /// `{ … }` — implicit fixtures resolved inside via injected reads.
    Block(Block),
    /// `|v: &T| …` — explicit read; the runtime hands `&T` in, no injection.
    Closure {
        param: Ident,
        ty: Box<Type>,
        body: Box<Expr>,
    },
    /// `async { … }` — lowered to `__rt::async_test`; named fixtures are cloned
    /// in (like async `before_all!`), so they must be `Clone` and the owned
    /// clone is moved into the future rather than a borrow held across `.await`.
    /// Requires the consumer to enable `rsspec`'s `tokio` feature (the only place
    /// `__rt::async_test` exists); without it the arm fails to resolve in the
    /// generated code.
    Async(Block),
}

struct ItArgs {
    name: LitStr,
    body: ItBody,
    decorators: Vec<Decorator>,
}

impl Parse for ItArgs {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let name = input.parse()?;
        input.parse::<Token![,]>()?;
        let body = if input.peek(Token![async]) {
            input.parse::<Token![async]>()?;
            ItBody::Async(input.parse()?)
        } else if input.peek(Token![|]) {
            input.parse::<Token![|]>()?;
            let param: Ident = input.parse()?;
            input.parse::<Token![:]>()?;
            input.parse::<Token![&]>()?;
            let ty: Type = input.parse()?;
            input.parse::<Token![|]>()?;
            ItBody::Closure {
                param,
                ty: Box::new(ty),
                body: Box::new(input.parse()?),
            }
        } else {
            ItBody::Block(input.parse()?)
        };
        let mut decorators = Vec::new();
        while input.peek(Token![,]) {
            input.parse::<Token![,]>()?;
            if input.is_empty() {
                break; // trailing comma
            }
            let key: Ident = input.parse()?;
            input.parse::<Token![=]>()?;
            let dec = match key.to_string().as_str() {
                "tags" => {
                    let content;
                    syn::bracketed!(content in input);
                    let items: Punctuated<Expr, Token![,]> =
                        content.parse_terminated(Expr::parse, Token![,])?;
                    Decorator::Tags(items.into_iter().collect())
                }
                "retries" => Decorator::Retries(input.parse()?),
                "timeout" => Decorator::Timeout(input.parse()?),
                "must_pass_repeatedly" => Decorator::MustPass(input.parse()?),
                other => {
                    return Err(syn::Error::new_spanned(
                        &key,
                        format!(
                            "rsspec: unknown decorator `{other}` \
                             (expected tags/retries/timeout/must_pass_repeatedly)"
                        ),
                    ))
                }
            };
            decorators.push(dec);
        }
        Ok(Self {
            name,
            body,
            decorators,
        })
    }
}

fn lower_it(mac: &Macro, fixtures: &[Fixture], ctor: &str) -> syn::Result<TokenStream2> {
    let ItArgs {
        name,
        body,
        decorators,
    } = parse2(mac.tokens.clone())?;
    let ctor = Ident::new(ctor, Span::call_site());
    let test_body = match body {
        ItBody::Block(b) => {
            let stmts = &b.stmts;
            let wrapped = wrap_reads(fixtures, quote! { #(#stmts)* });
            quote! { move || { #wrapped } }
        }
        ItBody::Closure { param, ty, body } => {
            // Explicit read: the runtime injects `&ty` through the closure param,
            // so the body sees only `param` — no implicit-fixture wrapping.
            quote! { move |#param: &#ty| { #body } }
        }
        ItBody::Async(b) => {
            // Named fixtures are cloned in (like async `before_all!`): the owned
            // clone is moved into the future, so a `&T` is never held across
            // `.await`. The referenced fixtures must be `Clone`.
            let stmts = &b.stmts;
            let body = quote! { #(#stmts)* };
            let binds = clone_binds(fixtures, &body);
            quote! {
                ::rsspec::__rt::async_test(move || {
                    #binds
                    async move { #body }
                })
            }
        }
    };
    let chain: TokenStream2 = decorators
        .iter()
        .map(|d| match d {
            Decorator::Tags(items) => quote! { .labels(&[#(#items),*]) },
            Decorator::Retries(n) => quote! { .retries(#n) },
            Decorator::Timeout(ms) => quote! { .timeout(#ms) },
            Decorator::MustPass(n) => quote! { .must_pass_repeatedly(#n) },
        })
        .collect();
    Ok(quote! {
        ::rsspec::__rt::#ctor(#name, #test_body)#chain;
    })
}

// ---- read injection ---------------------------------------------------------

/// Wrap `inner` in one `with_fixture::<T,_>(|name| …)` for each in-scope fixture
/// the body actually names (innermost = `inner`). The closure parameter reuses the
/// fixture's declared ident span, so the body's bare `name` resolves to it.
/// Fixtures the body never mentions are skipped — a spec reads only what it uses —
/// and the `let _ = &name;` guard still silences a name that appears only as a
/// like-named method or field.
fn wrap_reads(fixtures: &[Fixture], inner: TokenStream2) -> TokenStream2 {
    let mut acc = inner;
    for f in fixtures.iter().rev() {
        if !mentions_ident(&acc, &f.name) {
            continue;
        }
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

/// `let name = fixture_cloned::<T>();` bindings for each in-scope fixture the
/// `async` body names — injected before the `async` block so the owned clone
/// (not a borrow) is moved into the future and may live across `.await`. The
/// referenced fixtures must be `Clone`; unmentioned ones are skipped.
fn clone_binds(fixtures: &[Fixture], body: &TokenStream2) -> TokenStream2 {
    let mut binds = TokenStream2::new();
    for f in fixtures {
        if !mentions_ident(body, &f.name) {
            continue;
        }
        let name = &f.name;
        let ty = &f.ty;
        binds.extend(quote! {
            let #name = ::rsspec::__rt::fixture_cloned::<#ty>();
        });
    }
    binds
}

/// Whether `tokens` contains the identifier `name` anywhere, recursing into
/// groups. Drives `wrap_reads`' decision to inject a fixture read only when the
/// body refers to it.
fn mentions_ident(tokens: &TokenStream2, name: &Ident) -> bool {
    tokens.clone().into_iter().any(|tt| match tt {
        proc_macro2::TokenTree::Ident(id) => id == *name,
        proc_macro2::TokenTree::Group(g) => mentions_ident(&g.stream(), name),
        _ => false,
    })
}
