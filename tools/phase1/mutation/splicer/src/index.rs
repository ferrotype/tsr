//! A syn index of one file: every function with a body, every match arm and
//! statement inside one, and every `fn` inside a `macro_rules!` body.
//!
//! Positions are proc-macro2 line/column pairs (1-based lines, character
//! columns). The index only records where things are and what their types say;
//! choosing and rendering mutants happens in `operators` and `plan`.

use std::collections::HashMap;

use proc_macro2::{Delimiter, LineColumn, Span, TokenStream, TokenTree};
use quote::ToTokens;
use syn::spanned::Spanned;
use syn::visit::{self, Visit};
use syn::{AttrStyle, Attribute, Block, Expr, FnArg, Pat, Signature, Stmt, Visibility};

/// A position as (line, character column).
pub type Pos = (usize, usize);

fn pos(at: LineColumn) -> Pos {
    (at.line, at.column)
}

/// Token text with every whitespace character removed: the form return and
/// parameter types are classified in.
pub fn norm(tokens: &impl ToTokens) -> String {
    squash(&tokens.to_token_stream().to_string())
}

pub fn squash(text: &str) -> String {
    text.chars().filter(|c| !c.is_whitespace()).collect()
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Receiver {
    None,
    Value,
    Ref,
    RefMut,
}

#[derive(Clone, Debug)]
pub struct Param {
    pub name: String,
    pub mutable: bool,
    pub ty: String,
}

#[derive(Clone, Debug)]
pub struct FnInfo {
    pub name: String,
    pub fn_line: usize,
    /// First line of the item: outer attributes, visibility or signature.
    pub start_line: usize,
    pub end_line: usize,
    pub has_body: bool,
    /// Just inside the body: after `{`, or after the last inner attribute.
    pub open: Pos,
    /// The body's closing `}`.
    pub close: Pos,
    /// Normalized return type; `None` for unit.
    pub ret: Option<String>,
    /// The return type as spaced token text, valid Rust for a closure's return
    /// annotation (the normalized form drops the spaces `dyn X`/`&'a mut T` need).
    pub ret_text: Option<String>,
    pub receiver: Receiver,
    pub params: Vec<Param>,
    /// Normalized self type of the enclosing impl, empty outside impls.
    pub self_ty: String,
    pub cfg_test: bool,
    /// The body's single tail expression, normalized, when the body is only a
    /// literal or path (possibly wrapped in `Ok`/`Some`/`!`/`-`).
    pub tail_const: Option<String>,
    /// Names of `self.name(..)` calls in the body.
    pub self_calls: Vec<String>,
    /// Names of every method called in the body, whatever the receiver.
    pub method_calls: Vec<String>,
    /// The body assigns `self.token`: a parser token scanner.
    pub assigns_self_token: bool,
    /// A `const fn`: no runtime switch may run in it.
    pub is_const: bool,
    pub macro_body: bool,
}

#[derive(Clone, Debug)]
pub struct ArmInfo {
    pub start: Pos,
    pub end_line: usize,
    pub body_start: Pos,
    pub body_end: Pos,
    /// The arm's value type, when the match sits in the function's tail.
    pub value_ty: Option<String>,
    pub body_const: Option<String>,
    pub function: usize,
}

#[derive(Clone, Debug)]
pub enum StmtKind {
    /// `if COND { .. }` (not `if let`) as a statement.
    If {
        cond: (Pos, Pos),
    },
    /// `let x = if COND { .. } else { .. };`
    LetIf {
        cond: (Pos, Pos),
    },
    /// `EXPR;`, a unit statement: a call, assignment or macro whose value is
    /// discarded, or a loop.
    Unit {
        span: (Pos, Pos),
    },
    Other(&'static str),
}

#[derive(Clone, Debug)]
pub struct StmtInfo {
    pub start: Pos,
    pub end_line: usize,
    pub kind: StmtKind,
    pub function: usize,
}

#[derive(Default)]
pub struct FileIndex {
    pub fns: Vec<FnInfo>,
    pub arms: Vec<ArmInfo>,
    pub stmts: Vec<StmtInfo>,
}

pub fn index_file(text: &str) -> Result<FileIndex, String> {
    let file = syn::parse_file(text).map_err(|error| {
        let at = error.span().start();
        format!("{error} at {}:{}", at.line, at.column)
    })?;
    let mut indexer = Indexer::default();
    indexer.visit_file(&file);
    let Indexer {
        mut index,
        arm_types,
        ..
    } = indexer;
    for arm in &mut index.arms {
        arm.value_ty = arm_types.get(&arm.start).cloned();
    }
    Ok(index)
}

#[derive(Clone, Default)]
struct Context {
    self_ty: String,
    cfg_test: bool,
}

#[derive(Default)]
struct Indexer {
    index: FileIndex,
    contexts: Vec<Context>,
    fn_stack: Vec<usize>,
    arm_types: HashMap<Pos, String>,
}

/// Whether a `cfg` predicate compiles only under test: `test` itself or an
/// `all(..)` with `test` among its operands. `any(test, feature = ..)` is not
/// test-only code and is kept.
pub fn test_only_predicate(predicate: &str) -> bool {
    let predicate = squash(predicate);
    if predicate == "test" {
        return true;
    }
    let Some(inner) = predicate
        .strip_prefix("all(")
        .and_then(|rest| rest.strip_suffix(')'))
    else {
        return false;
    };
    top_level_args(inner).iter().any(|arg| arg == "test")
}

/// Splits `a,b<c,d>,(e,f)` at top-level commas.
pub fn top_level_args(text: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut depth = 0i32;
    let mut current = String::new();
    let mut previous = ' ';
    for c in text.chars() {
        match c {
            '<' | '(' | '[' | '{' => depth += 1,
            '>' if previous != '-' => depth -= 1,
            ')' | ']' | '}' => depth -= 1,
            ',' if depth == 0 => {
                args.push(std::mem::take(&mut current));
                previous = c;
                continue;
            }
            _ => {}
        }
        current.push(c);
        previous = c;
    }
    if !current.is_empty() {
        args.push(current);
    }
    args
}

fn test_only_attrs(attrs: &[Attribute]) -> bool {
    attrs.iter().any(|attr| {
        if attr.path().is_ident("test") {
            return true;
        }
        if !attr.path().is_ident("cfg") {
            return false;
        }
        match &attr.meta {
            syn::Meta::List(list) => test_only_predicate(&list.tokens.to_string()),
            _ => false,
        }
    })
}

fn start_of(attrs: &[Attribute], rest: Span) -> Pos {
    attrs
        .iter()
        .filter(|attr| matches!(attr.style, AttrStyle::Outer))
        .map(|attr| pos(attr.span().start()))
        .chain(std::iter::once(pos(rest.start())))
        .min()
        .expect("at least one position")
}

/// A literal or path, optionally under `!`/`-` or one `Ok(..)`/`Some(..)`.
fn const_expr(expr: &Expr) -> Option<String> {
    match expr {
        Expr::Lit(_) | Expr::Path(_) => Some(norm(expr)),
        Expr::Unary(unary) => const_expr(&unary.expr).map(|_| norm(expr)),
        Expr::Paren(paren) => const_expr(&paren.expr),
        Expr::Call(call) if call.args.len() == 1 && is_ok_or_some(&call.func) => {
            const_expr(&call.args[0]).map(|_| norm(expr))
        }
        _ => None,
    }
}

fn is_ok_or_some(func: &Expr) -> bool {
    matches!(func, Expr::Path(path) if path.path.is_ident("Ok") || path.path.is_ident("Some"))
}

fn is_self(expr: &Expr) -> bool {
    matches!(expr, Expr::Path(path) if path.path.is_ident("self"))
}

/// The first generic argument of `Head<..>` when `ty`'s head is `head`.
pub fn first_arg<'t>(ty: &'t str, head: &str) -> Option<&'t str> {
    let open = ty.find('<')?;
    let path = &ty[..open];
    let last = path.rsplit("::").next().unwrap_or(path);
    if last != head || !ty.ends_with('>') {
        return None;
    }
    let inner = &ty[open + 1..ty.len() - 1];
    let args = top_level_args(inner);
    let first = args.first()?;
    let start = inner.find(first.as_str())?;
    Some(&inner[start..start + first.len()])
}

fn arm_start(arm: &syn::Arm) -> Pos {
    start_of(&arm.attrs, arm.pat.span())
}

/// Records the value type of every arm of a match reachable from a function's
/// tail through blocks, parentheses, `if` branches, `Ok(..)`/`Some(..)` and
/// nested arm bodies.
fn typed_arms(expr: &Expr, ty: &str, out: &mut HashMap<Pos, String>) {
    match expr {
        Expr::Match(matched) => {
            for arm in &matched.arms {
                out.insert(arm_start(arm), ty.to_owned());
                typed_arms(&arm.body, ty, out);
            }
        }
        Expr::Block(block) => typed_tail(&block.block, ty, out),
        Expr::Paren(paren) => typed_arms(&paren.expr, ty, out),
        Expr::If(branch) => {
            typed_tail(&branch.then_branch, ty, out);
            if let Some((_, other)) = &branch.else_branch {
                typed_arms(other, ty, out);
            }
        }
        Expr::Call(call) if call.args.len() == 1 => {
            let Expr::Path(path) = &*call.func else {
                return;
            };
            let inner = if path.path.is_ident("Ok") {
                first_arg(ty, "Result")
            } else if path.path.is_ident("Some") {
                first_arg(ty, "Option")
            } else {
                None
            };
            if let Some(inner) = inner {
                typed_arms(&call.args[0], inner, out);
            }
        }
        _ => {}
    }
}

fn typed_tail(block: &Block, ty: &str, out: &mut HashMap<Pos, String>) {
    if let Some(Stmt::Expr(tail, None)) = block.stmts.last() {
        typed_arms(tail, ty, out);
    }
}

fn receiver_of(sig: &Signature) -> Receiver {
    match sig.receiver() {
        None => Receiver::None,
        Some(receiver) if receiver.colon_token.is_some() => {
            let ty = norm(&receiver.ty);
            if ty.starts_with("&mut") {
                Receiver::RefMut
            } else if ty.starts_with('&') {
                Receiver::Ref
            } else {
                Receiver::Value
            }
        }
        Some(receiver) => match (&receiver.reference, &receiver.mutability) {
            (Some(_), Some(_)) => Receiver::RefMut,
            (Some(_), None) => Receiver::Ref,
            (None, _) => Receiver::Value,
        },
    }
}

/// The receiver of a macro-body `fn` from its squashed parameter text.
fn macro_receiver(text: &str) -> Receiver {
    let Some(rest) = text.strip_prefix('&') else {
        return if text.starts_with("self") || text.starts_with("mutself") {
            Receiver::Value
        } else {
            Receiver::None
        };
    };
    let rest = match rest.strip_prefix('\'') {
        Some(lifetime) => lifetime.trim_start_matches(|c: char| c.is_alphanumeric() || c == '_'),
        None => rest,
    };
    if rest.starts_with("mutself") {
        Receiver::RefMut
    } else if rest.starts_with("self") {
        Receiver::Ref
    } else {
        Receiver::None
    }
}

fn params_of(sig: &Signature) -> Vec<Param> {
    sig.inputs
        .iter()
        .filter_map(|input| {
            let FnArg::Typed(typed) = input else {
                return None;
            };
            let Pat::Ident(ident) = &*typed.pat else {
                return None;
            };
            if ident.by_ref.is_some() || ident.subpat.is_some() {
                return None;
            }
            Some(Param {
                name: ident.ident.to_string(),
                mutable: ident.mutability.is_some(),
                ty: norm(&typed.ty),
            })
        })
        .collect()
}

impl Indexer {
    fn context(&self) -> Context {
        self.contexts.last().cloned().unwrap_or_default()
    }

    fn record_fn(
        &mut self,
        attrs: &[Attribute],
        vis: Option<&Visibility>,
        sig: &Signature,
        block: Option<&Block>,
    ) -> usize {
        let context = self.context();
        let mut start = start_of(attrs, sig.span());
        if let Some(vis) = vis {
            if !matches!(vis, Visibility::Inherited) {
                start = start.min(pos(vis.span().start()));
            }
        }
        let (open, close, end_line, tail_const) = match block {
            Some(block) => {
                let mut open = pos(block.brace_token.span.open().end());
                for attr in attrs {
                    if matches!(attr.style, AttrStyle::Inner(_)) {
                        open = open.max(pos(attr.span().end()));
                    }
                }
                let close = pos(block.brace_token.span.close().start());
                let tail_const = match block.stmts.as_slice() {
                    [Stmt::Expr(tail, None)] => const_expr(tail),
                    _ => None,
                };
                (open, close, close.0, tail_const)
            }
            None => ((0, 0), (0, 0), pos(sig.span().end()).0, None),
        };
        let (ret, ret_text) = match &sig.output {
            syn::ReturnType::Default => (None, None),
            syn::ReturnType::Type(_, ty) if norm(ty) == "()" => (None, None),
            syn::ReturnType::Type(_, ty) => {
                (Some(norm(ty)), Some(ty.to_token_stream().to_string()))
            }
        };
        if let (Some(ret), Some(block)) = (&ret, block) {
            typed_tail(block, ret, &mut self.arm_types);
        }
        self.index.fns.push(FnInfo {
            name: sig.ident.to_string(),
            fn_line: sig.fn_token.span.start().line,
            start_line: start.0,
            end_line,
            has_body: block.is_some(),
            open,
            close,
            ret,
            ret_text,
            receiver: receiver_of(sig),
            params: params_of(sig),
            self_ty: context.self_ty,
            cfg_test: context.cfg_test || test_only_attrs(attrs),
            tail_const,
            self_calls: Vec::new(),
            method_calls: Vec::new(),
            assigns_self_token: false,
            is_const: sig.constness.is_some(),
            macro_body: false,
        });
        self.index.fns.len() - 1
    }

    fn scan_macro(&mut self, stream: TokenStream, cfg_test: bool) {
        let tokens: Vec<TokenTree> = stream.into_iter().collect();
        for (at, token) in tokens.iter().enumerate() {
            match token {
                TokenTree::Ident(ident) if ident == "fn" => {
                    if let Some(TokenTree::Ident(name)) = tokens.get(at + 1) {
                        self.record_macro_fn(&tokens, at, &name.to_string(), cfg_test);
                    }
                }
                TokenTree::Group(group) => self.scan_macro(group.stream(), cfg_test),
                _ => {}
            }
        }
    }

    /// A `fn` inside a `macro_rules!` body: syn sees only tokens, so the header
    /// is read token by token up to the body's brace group.
    fn record_macro_fn(&mut self, tokens: &[TokenTree], at: usize, name: &str, cfg_test: bool) {
        let fn_line = tokens[at].span().start().line;
        let mut params = None;
        let mut ret = TokenStream::new();
        let mut arrow = false;
        let mut body = None;
        let mut index = at + 2;
        while index < tokens.len() {
            match &tokens[index] {
                TokenTree::Group(group) if group.delimiter() == Delimiter::Brace => {
                    body = Some(group.clone());
                    break;
                }
                TokenTree::Group(group)
                    if group.delimiter() == Delimiter::Parenthesis
                        && params.is_none()
                        && !arrow =>
                {
                    params = Some(group.clone());
                }
                TokenTree::Punct(punct) if punct.as_char() == ';' => break,
                TokenTree::Punct(punct)
                    if punct.as_char() == '-'
                        && matches!(tokens.get(index + 1), Some(TokenTree::Punct(next)) if next.as_char() == '>') =>
                {
                    arrow = true;
                    index += 1;
                }
                token if arrow => ret.extend(std::iter::once(token.clone())),
                _ => {}
            }
            index += 1;
        }
        let Some(body) = body else {
            return;
        };
        let receiver = params.map_or(Receiver::None, |params| {
            macro_receiver(&squash(&params.stream().to_string()))
        });
        let ret_text =
            Some(ret.to_string()).filter(|text| !squash(text).is_empty() && squash(text) != "()");
        let ret = ret_text.as_deref().map(squash);
        let is_const = tokens[..at]
            .iter()
            .rev()
            .take_while(|token| matches!(token, TokenTree::Ident(ident) if ident != "fn"))
            .any(|token| matches!(token, TokenTree::Ident(ident) if ident == "const"));
        let open = pos(body.span_open().end());
        let close = pos(body.span_close().start());
        self.index.fns.push(FnInfo {
            name: name.to_owned(),
            fn_line,
            start_line: fn_line,
            end_line: close.0,
            has_body: true,
            open,
            close,
            ret,
            ret_text,
            receiver,
            params: Vec::new(),
            self_ty: String::new(),
            cfg_test,
            tail_const: None,
            self_calls: Vec::new(),
            method_calls: Vec::new(),
            assigns_self_token: false,
            is_const,
            macro_body: true,
        });
    }

    fn with_context<T>(&mut self, context: Context, run: impl FnOnce(&mut Self) -> T) -> T {
        self.contexts.push(context);
        let saved = std::mem::take(&mut self.fn_stack);
        let result = run(self);
        self.fn_stack = saved;
        self.contexts.pop();
        result
    }

    fn in_fn(&mut self, index: usize, run: impl FnOnce(&mut Self)) {
        let mut context = self.context();
        context.cfg_test = self.index.fns[index].cfg_test;
        self.contexts.push(context);
        self.fn_stack.push(index);
        run(self);
        self.fn_stack.pop();
        self.contexts.pop();
    }
}

impl<'ast> Visit<'ast> for Indexer {
    fn visit_item_mod(&mut self, item: &'ast syn::ItemMod) {
        let context = Context {
            self_ty: String::new(),
            cfg_test: self.context().cfg_test || test_only_attrs(&item.attrs),
        };
        self.with_context(context, |this| visit::visit_item_mod(this, item));
    }

    fn visit_item_impl(&mut self, item: &'ast syn::ItemImpl) {
        let context = Context {
            self_ty: norm(&item.self_ty),
            cfg_test: self.context().cfg_test || test_only_attrs(&item.attrs),
        };
        self.with_context(context, |this| visit::visit_item_impl(this, item));
    }

    fn visit_item_trait(&mut self, item: &'ast syn::ItemTrait) {
        let context = Context {
            self_ty: String::new(),
            cfg_test: self.context().cfg_test || test_only_attrs(&item.attrs),
        };
        self.with_context(context, |this| visit::visit_item_trait(this, item));
    }

    fn visit_item_fn(&mut self, item: &'ast syn::ItemFn) {
        // A free function has no self type even when nested inside an impl method.
        let context = Context {
            self_ty: String::new(),
            cfg_test: self.context().cfg_test,
        };
        let index = self.with_context(context, |this| {
            this.record_fn(&item.attrs, Some(&item.vis), &item.sig, Some(&item.block))
        });
        self.in_fn(index, |this| visit::visit_item_fn(this, item));
    }

    fn visit_impl_item_fn(&mut self, item: &'ast syn::ImplItemFn) {
        let index = self.record_fn(&item.attrs, Some(&item.vis), &item.sig, Some(&item.block));
        self.in_fn(index, |this| visit::visit_impl_item_fn(this, item));
    }

    fn visit_trait_item_fn(&mut self, item: &'ast syn::TraitItemFn) {
        let index = self.record_fn(&item.attrs, None, &item.sig, item.default.as_ref());
        self.in_fn(index, |this| visit::visit_trait_item_fn(this, item));
    }

    fn visit_item_macro(&mut self, item: &'ast syn::ItemMacro) {
        if item.mac.path.is_ident("macro_rules") {
            let cfg_test = self.context().cfg_test || test_only_attrs(&item.attrs);
            self.scan_macro(item.mac.tokens.clone(), cfg_test);
        }
        visit::visit_item_macro(self, item);
    }

    fn visit_expr_method_call(&mut self, call: &'ast syn::ExprMethodCall) {
        if let Some(&function) = self.fn_stack.last() {
            let name = call.method.to_string();
            if is_self(&call.receiver) {
                self.index.fns[function].self_calls.push(name.clone());
            }
            self.index.fns[function].method_calls.push(name);
        }
        visit::visit_expr_method_call(self, call);
    }

    fn visit_expr_assign(&mut self, assign: &'ast syn::ExprAssign) {
        if let (Some(&function), Expr::Field(field)) = (self.fn_stack.last(), &*assign.left) {
            if is_self(&field.base)
                && matches!(&field.member, syn::Member::Named(name) if name == "token")
            {
                self.index.fns[function].assigns_self_token = true;
            }
        }
        visit::visit_expr_assign(self, assign);
    }

    fn visit_arm(&mut self, arm: &'ast syn::Arm) {
        if let Some(&function) = self.fn_stack.last() {
            let body = arm.body.span();
            let end = arm
                .comma
                .as_ref()
                .map_or(body.end(), |comma| comma.span.end());
            self.index.arms.push(ArmInfo {
                start: arm_start(arm),
                end_line: end.line,
                body_start: pos(body.start()),
                body_end: pos(body.end()),
                value_ty: None,
                body_const: const_expr(&arm.body),
                function,
            });
        }
        visit::visit_arm(self, arm);
    }

    fn visit_stmt(&mut self, stmt: &'ast Stmt) {
        if let Some(&function) = self.fn_stack.last() {
            let span = stmt.span();
            let condition = |cond: &Expr| (pos(cond.span().start()), pos(cond.span().end()));
            let kind = match stmt {
                Stmt::Expr(Expr::If(branch), _) if !matches!(*branch.cond, Expr::Let(_)) => {
                    StmtKind::If {
                        cond: condition(&branch.cond),
                    }
                }
                Stmt::Local(local) => match local.init.as_ref() {
                    Some(init) if init.diverge.is_none() => match &*init.expr {
                        Expr::If(branch) if !matches!(*branch.cond, Expr::Let(_)) => {
                            StmtKind::LetIf {
                                cond: condition(&branch.cond),
                            }
                        }
                        _ => StmtKind::Other("let"),
                    },
                    _ => StmtKind::Other("let"),
                },
                // A jump skipped would fall through to code typed for its
                // absence.
                Stmt::Expr(Expr::Return(_) | Expr::Break(_) | Expr::Continue(_), _) => {
                    StmtKind::Other("expression")
                }
                Stmt::Expr(Expr::ForLoop(_) | Expr::While(_), _) | Stmt::Expr(_, Some(_)) => {
                    StmtKind::Unit {
                        span: (pos(span.start()), pos(span.end())),
                    }
                }
                Stmt::Expr(..) => StmtKind::Other("expression"),
                Stmt::Macro(_) => StmtKind::Other("macro"),
                Stmt::Item(_) => StmtKind::Other("item"),
            };
            if !matches!(stmt, Stmt::Item(_)) {
                self.index.stmts.push(StmtInfo {
                    start: pos(span.start()),
                    end_line: span.end().line,
                    kind,
                    function,
                });
            }
        }
        visit::visit_stmt(self, stmt);
    }
}

#[cfg(test)]
mod tests {
    use super::{first_arg, index_file, test_only_predicate, top_level_args, Receiver, StmtKind};

    const SAMPLE: &str = r"
impl<F: ParserFactory> Parser<'_, F> {
    /// port: tsc/p.go:Parser.a
    #[inline]
    pub(crate) fn parse_a(&mut self, flag: bool, mut count: u32) -> Option<NodeId> {
        self.next_token();
        match self.token {
            K::A => Some(self.b()),
            // port: tsc/p.go:arm
            _ => None,
        }
    }
}
fn facts(node: &Node) -> u32 {
    match node.kind() {
        K::A => NONE,
        K::B => Ok(match x { 1 => true, _ => false }),
    }
}
fn result() -> Result<bool, Error> {
    Ok(match x { 1 => true, _ => false })
}
fn statements(kind: K) {
    // port: tsc/p.go:stmt
    if matches!(kind, K::A) { return; }
    let other = if kind == K::A { 1 } else { 2 };
    if let Some(x) = y {}
}
macro_rules! reads {
    ($node_id:ty) => {
        /// port: tsc/a.go:Node.Expression
        $visibility fn expression(&self) -> Option<$node_id> {
            None
        }
    };
}
#[cfg(all(test, unix))]
mod tests {
    fn helper() -> bool { true }
}
";

    #[test]
    fn functions_arms_statements_and_macro_bodies_are_indexed() {
        let index = index_file(SAMPLE).unwrap();
        let parse = &index.fns[0];
        assert_eq!(parse.name, "parse_a");
        assert_eq!(
            (parse.fn_line, parse.start_line),
            (5, 3),
            "the doc marker is an attribute"
        );
        assert_eq!(parse.self_ty, "Parser<'_,F>");
        assert_eq!(parse.receiver, Receiver::RefMut);
        assert_eq!(parse.ret.as_deref(), Some("Option<NodeId>"));
        assert_eq!(parse.params.len(), 2);
        assert!(parse.params[1].mutable);
        assert_eq!(parse.self_calls, ["next_token", "b"]);
        let line5 = SAMPLE.lines().nth(4).unwrap();
        assert_eq!(parse.open.0, 5);
        assert!(line5
            .chars()
            .take(parse.open.1)
            .collect::<String>()
            .ends_with('{'));
        assert_eq!(parse.open.1, line5.chars().count());
        assert_eq!(parse.close.0, 12);

        let arms: Vec<_> = index
            .arms
            .iter()
            .map(|arm| (arm.start.0, arm.value_ty.clone()))
            .collect();
        assert!(arms.contains(&(8, Some("Option<NodeId>".to_owned()))));
        assert!(arms.contains(&(10, Some("Option<NodeId>".to_owned()))));
        assert!(arms.contains(&(16, Some("u32".to_owned()))));
        assert!(arms.contains(&(17, Some("u32".to_owned()))));
        let result_arms: Vec<_> = index
            .arms
            .iter()
            .filter(|arm| arm.start.0 == 21)
            .map(|arm| arm.value_ty.clone())
            .collect();
        assert_eq!(
            result_arms,
            [Some("bool".to_owned()), Some("bool".to_owned())]
        );
        let nested: Vec<_> = index
            .arms
            .iter()
            .filter(|arm| arm.start.0 == 17 && arm.start.1 > 20)
            .collect();
        assert_eq!(nested.len(), 2);
        assert!(
            nested.iter().all(|arm| arm.value_ty.is_none()),
            "an Ok(..) arm in a u32 match is not typed"
        );
        assert_eq!(
            index
                .arms
                .iter()
                .find(|arm| arm.start.0 == 16)
                .unwrap()
                .body_const
                .as_deref(),
            Some("NONE")
        );

        let kinds: Vec<_> = index
            .stmts
            .iter()
            .filter(|stmt| stmt.start.0 >= 25 && stmt.start.1 == 4)
            .map(|stmt| (stmt.start.0, &stmt.kind))
            .collect();
        assert!(matches!(kinds[0], (25, StmtKind::If { .. })));
        assert!(matches!(kinds[1], (26, StmtKind::LetIf { .. })));
        assert!(
            matches!(kinds[2], (27, StmtKind::Other("expression"))),
            "`if let` is not negated"
        );
        assert_eq!(kinds.len(), 3);

        let expression = index.fns.iter().find(|f| f.name == "expression").unwrap();
        assert!(expression.macro_body);
        assert_eq!(expression.ret.as_deref(), Some("Option<$node_id>"));
        assert_eq!(expression.receiver, Receiver::Ref);
        assert_eq!((expression.fn_line, expression.close.0), (32, 34));

        let helper = index.fns.iter().find(|f| f.name == "helper").unwrap();
        assert!(helper.cfg_test);
        assert!(!parse.cfg_test);
    }

    #[test]
    fn predicates_and_generic_arguments() {
        assert!(test_only_predicate("test"));
        assert!(test_only_predicate("all(test, not(miri))"));
        assert!(!test_only_predicate("any(test, feature = \"harness\")"));
        assert!(!test_only_predicate("not(test)"));
        assert_eq!(
            top_level_args("a,B<c,d>,(e,f),g->h"),
            ["a", "B<c,d>", "(e,f)", "g->h"]
        );
        assert_eq!(
            first_arg("Result<Option<NodeId>,Error>", "Result"),
            Some("Option<NodeId>")
        );
        assert_eq!(first_arg("crate::Result<bool>", "Result"), Some("bool"));
        assert_eq!(first_arg("Option<NodeId>", "Result"), None);
    }
}
