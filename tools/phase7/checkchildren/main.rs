//! Compiler-private checkchildren probe, built only with the pinned tooling nightly.
#![feature(rustc_private)]
#![forbid(unsafe_code)]

extern crate rustc_driver;
extern crate rustc_hir;
extern crate rustc_interface;
extern crate rustc_middle;
extern crate rustc_span;

use rustc_driver::{Callbacks, Compilation};
use rustc_hir::def::{DefKind, Res};
use rustc_hir::intravisit::{self, Visitor};
use rustc_hir::{Arm, Block, Expr, ExprKind, HirId, Pat, PatKind, QPath, Stmt, StmtKind};
use rustc_middle::ty::TyCtxt;
use rustc_span::def_id::LocalDefId;
use rustc_span::Pos;
use std::collections::{HashMap, HashSet};
use std::process::ExitCode;

const WORKERS: [&str; 2] = ["check_source_element_worker", "check_expression_worker"];

#[derive(Clone, Copy)]
enum TreeNode<'a> {
    Expr(&'a Expr<'a>),
    Stmt(&'a Stmt<'a>),
    Block(&'a Block<'a>),
    Arm(&'a Arm<'a>),
}
impl TreeNode<'_> {
    fn id(self) -> HirId {
        match self {
            Self::Expr(value) => value.hir_id,
            Self::Stmt(value) => value.hir_id,
            Self::Block(value) => value.hir_id,
            Self::Arm(value) => value.hir_id,
        }
    }
}

#[derive(Default)]
struct Tree<'a> {
    nodes: HashMap<HirId, TreeNode<'a>>,
    parents: HashMap<HirId, HirId>,
    stack: Vec<HirId>,
    calls: Vec<&'a Expr<'a>>,
    returns: Vec<&'a Expr<'a>>,
}
impl<'a> Tree<'a> {
    fn enter(&mut self, node: TreeNode<'a>) {
        self.nodes.insert(node.id(), node);
        if let Some(&parent) = self.stack.last() {
            self.parents.insert(node.id(), parent);
        }
        self.stack.push(node.id());
    }
    fn contains(&self, ancestor: HirId, mut child: HirId) -> bool {
        loop {
            if ancestor == child {
                return true;
            }
            let Some(&parent) = self.parents.get(&child) else {
                return false;
            };
            child = parent;
        }
    }
    fn checks_after(&self, ret: HirId, checks: &[&'a Expr<'a>]) -> Vec<&'a Expr<'a>> {
        let mut result = Vec::new();
        let mut current = ret;
        while let Some(&parent) = self.parents.get(&current) {
            match self.nodes[&parent] {
                TreeNode::Block(block) => {
                    if let Some(index) = block.stmts.iter().position(|s| s.hir_id == current) {
                        for sibling in &block.stmts[index + 1..] {
                            result.extend(
                                checks
                                    .iter()
                                    .copied()
                                    .filter(|call| self.contains(sibling.hir_id, call.hir_id)),
                            );
                        }
                        if let Some(tail) = block.expr {
                            result.extend(
                                checks
                                    .iter()
                                    .copied()
                                    .filter(|call| self.contains(tail.hir_id, call.hir_id)),
                            );
                        }
                    }
                }
                TreeNode::Expr(Expr {
                    kind: ExprKind::Loop(body, ..),
                    ..
                }) if body.hir_id == current => {
                    // Falling through revisits the loop, including the lowered
                    // while condition. Sibling match arms are never successors.
                    result.extend(
                        checks
                            .iter()
                            .copied()
                            .filter(|call| self.contains(body.hir_id, call.hir_id)),
                    );
                }
                _ => {}
            }
            current = parent;
        }
        result
    }
    fn absent_guard(
        &self,
        tcx: TyCtxt<'_>,
        owner: LocalDefId,
        ret: HirId,
        child: &Expr<'_>,
    ) -> bool {
        let mut current = ret;
        while let Some(&parent) = self.parents.get(&current) {
            if let TreeNode::Expr(Expr {
                kind: ExprKind::If(condition, yes, no),
                ..
            }) = self.nodes[&parent]
            {
                if let ExprKind::MethodCall(method, receiver, args, _) = condition.kind {
                    let absent = method.ident.name.as_str() == "is_none" && current == yes.hir_id
                        || method.ident.name.as_str() == "is_some"
                            && no.is_some_and(|no| current == no.hir_id);
                    if absent
                        && args.is_empty()
                        && same_expr(receiver, child)
                        && tcx
                            .typeck(owner)
                            .expr_ty(receiver)
                            .peel_refs()
                            .ty_adt_def()
                            .is_some_and(|adt| Some(adt.did()) == tcx.lang_items().option_type())
                    {
                        return true;
                    }
                }
            }
            if let TreeNode::Stmt(Stmt {
                kind: StmtKind::Let(local),
                ..
            }) = self.nodes[&parent]
            {
                // The bound child exists only on the successful Some pattern path.
                // Match the binding DefId, so a different child's check still fails.
                if local.els.is_some_and(|els| self.contains(els.hir_id, ret))
                    && option_binding(tcx, owner, local.pat).is_some_and(|binding| {
                        matches!(child.kind, ExprKind::Path(QPath::Resolved(_, path))
                            if matches!(path.res, Res::Local(id) if id == binding))
                    })
                {
                    return true;
                }
            }
            current = parent;
        }
        false
    }
}
impl<'a> Visitor<'a> for Tree<'a> {
    fn visit_expr(&mut self, expr: &'a Expr<'a>) {
        self.enter(TreeNode::Expr(expr));
        match expr.kind {
            ExprKind::MethodCall(..) => self.calls.push(expr),
            ExprKind::Ret(_) if !expr.span.from_expansion() => self.returns.push(expr),
            _ => {}
        }
        // Default intravisit does not enter nested closure/item bodies. Each
        // has its own control flow, just like the Go analyzer's FuncLit rule.
        intravisit::walk_expr(self, expr);
        self.stack.pop();
    }
    fn visit_stmt(&mut self, stmt: &'a Stmt<'a>) {
        self.enter(TreeNode::Stmt(stmt));
        intravisit::walk_stmt(self, stmt);
        self.stack.pop();
    }
    fn visit_block(&mut self, block: &'a Block<'a>) {
        self.enter(TreeNode::Block(block));
        intravisit::walk_block(self, block);
        self.stack.pop();
    }
    fn visit_arm(&mut self, arm: &'a Arm<'a>) {
        self.enter(TreeNode::Arm(arm));
        intravisit::walk_arm(self, arm);
        self.stack.pop();
    }
}

fn same_expr(left: &Expr<'_>, right: &Expr<'_>) -> bool {
    match (left.kind, right.kind) {
        (ExprKind::Path(QPath::Resolved(_, l)), ExprKind::Path(QPath::Resolved(_, r))) => {
            l.res == r.res
        }
        (ExprKind::Field(l, ln), ExprKind::Field(r, rn)) => ln.name == rn.name && same_expr(l, r),
        (ExprKind::MethodCall(lm, l, la, _), ExprKind::MethodCall(rm, r, ra, _)) => {
            lm.ident.name == rm.ident.name
                && same_expr(l, r)
                && la.len() == ra.len()
                && la.iter().zip(ra).all(|(l, r)| same_expr(l, r))
        }
        (ExprKind::Call(l, la), ExprKind::Call(r, ra)) => {
            same_expr(l, r)
                && la.len() == ra.len()
                && la.iter().zip(ra).all(|(l, r)| same_expr(l, r))
        }
        (ExprKind::Unary(lop, l), ExprKind::Unary(rop, r)) => lop == rop && same_expr(l, r),
        (ExprKind::Lit(l), ExprKind::Lit(r)) => l.node == r.node,
        (ExprKind::Index(l, li, _), ExprKind::Index(r, ri, _)) => {
            same_expr(l, r) && same_expr(li, ri)
        }
        _ => false,
    }
}
fn self_call<'a>(expr: &'a Expr<'a>) -> Option<(rustc_span::Symbol, &'a [Expr<'a>])> {
    if let ExprKind::MethodCall(method, receiver, args, _) = expr.kind {
        if let ExprKind::Path(QPath::Resolved(_, path)) = receiver.kind {
            if path
                .segments
                .last()
                .is_some_and(|s| s.ident.name.as_str() == "self")
            {
                return Some((method.ident.name, args));
            }
        }
    }
    None
}
fn is_child_check(name: &str) -> bool {
    // Unlike its Rust spelling, this ports checkTypeRelatedToAndOptionallyElaborate
    // (call_errors.rs), which relates already checked types rather than children.
    if name == "check_expression_related_with_elaboration" {
        return false;
    }
    name.starts_with("check_expression")
        || name.starts_with("check_source_element")
        || name == "resolve_call"
}
fn option_binding(tcx: TyCtxt<'_>, owner: LocalDefId, pattern: &Pat<'_>) -> Option<HirId> {
    let PatKind::TupleStruct(ref path, fields, _) = pattern.kind else {
        return None;
    };
    let id = tcx
        .typeck(owner)
        .qpath_res(path, pattern.hir_id)
        .opt_def_id()?;
    if Some(tcx.parent(id)) != tcx.lang_items().option_some_variant() || fields.len() != 1 {
        return None;
    }
    let mut field = &fields[0];
    while let PatKind::Ref(inner, _, _) = field.kind {
        field = inner;
    }
    if let PatKind::Binding(_, id, _, None) = field.kind {
        Some(id)
    } else {
        None
    }
}
fn is_error_return(tcx: TyCtxt<'_>, owner: LocalDefId, expr: &Expr<'_>) -> bool {
    // Returning a failed Result retires/refuses the operation. Go's checker
    // has no corresponding error return. Such paths cannot promise diagnostics.
    let ExprKind::Ret(Some(value)) = expr.kind else {
        return false;
    };
    let ExprKind::Call(callee, _) = value.kind else {
        return false;
    };
    let ExprKind::Path(ref path) = callee.kind else {
        return false;
    };
    let Some(id) = tcx
        .typeck(owner)
        .qpath_res(path, callee.hir_id)
        .opt_def_id()
    else {
        return false;
    };
    Some(tcx.parent(id)) == tcx.lang_items().result_err_variant()
}

fn return_allowance(tcx: TyCtxt<'_>, expr: &Expr<'_>) -> Result<Option<String>, ()> {
    let location = tcx.sess.source_map().lookup_char_pos(expr.span.lo());
    if location.line < 2 {
        return Ok(None);
    }
    // A preceding-line allowance applies only to a return beginning its line.
    // A second return on the same line must not inherit the first's allowance.
    if !location
        .file
        .get_line(location.line - 1)
        .is_some_and(|line| {
            line.chars()
                .take(location.col.to_usize())
                .all(char::is_whitespace)
        })
    {
        return Ok(None);
    }
    let Some(line) = location.file.get_line(location.line - 2) else {
        return Ok(None);
    };
    let Some(reason) = line.trim().strip_prefix("// checkchildren-allow:") else {
        return Ok(None);
    };
    let reason = reason.trim();
    if reason.is_empty() {
        Err(())
    } else {
        Ok(Some(reason.to_owned()))
    }
}

#[derive(Default)]
struct CheckChildren {
    failed: bool,
    enabled: bool,
}
impl Callbacks for CheckChildren {
    fn after_analysis<'tcx>(
        &mut self,
        _: &rustc_interface::interface::Compiler,
        tcx: TyCtxt<'tcx>,
    ) -> Compilation {
        if !self.enabled {
            return Compilation::Continue;
        }
        let mut handlers = HashSet::<LocalDefId>::new();
        let mut workers = 0;
        for owner in tcx.hir_body_owners() {
            if !matches!(tcx.def_kind(owner), DefKind::Fn | DefKind::AssocFn) {
                continue;
            }
            if WORKERS.contains(&tcx.item_name(owner.to_def_id()).as_str()) {
                workers += 1;
                let mut tree = Tree::default();
                tree.visit_body(tcx.hir_body_owned_by(owner));
                for call in tree.calls {
                    if self_call(call).is_some() {
                        if let Some(id) = tcx
                            .typeck(owner)
                            .type_dependent_def_id(call.hir_id)
                            .and_then(|id| id.as_local())
                        {
                            handlers.insert(id);
                        }
                    }
                }
            }
        }
        if workers == 0 {
            eprintln!("checkchildren: no dispatch workers found; refusing an empty lint run");
            self.failed = true;
            return Compilation::Stop;
        }
        let mut handlers: Vec<_> = handlers.into_iter().collect();
        handlers.sort_by_key(|&id| tcx.def_path_str(id));
        let mut findings = 0;
        let mut allowances = 0;
        let mut invalid_allowances = 0;
        for &handler in &handlers {
            let Some(body) = tcx.hir_maybe_body_owned_by(handler) else {
                continue;
            };
            let mut tree = Tree::default();
            tree.visit_body(body);
            let checks: Vec<_> = tree
                .calls
                .iter()
                .copied()
                .filter(|call| {
                    self_call(call).is_some_and(|(name, _)| is_child_check(name.as_str()))
                })
                .collect();
            for ret in &tree.returns {
                let allowance = match return_allowance(tcx, ret) {
                    Ok(allowance) => allowance,
                    Err(()) => {
                        invalid_allowances += 1;
                        eprintln!(
                            "{}: checkchildren: empty allowance reason",
                            tcx.sess.source_map().span_to_diagnostic_string(ret.span)
                        );
                        continue;
                    }
                };
                if is_error_return(tcx, handler, ret) {
                    continue;
                }
                let violation = tree
                    .checks_after(ret.hir_id, &checks)
                    .into_iter()
                    .find(|call| {
                        let (_, args) = self_call(call).unwrap();
                        !args
                            .first()
                            .is_some_and(|child| tree.absent_guard(tcx, handler, ret.hir_id, child))
                    });
                if let Some(call) = violation {
                    if let Some(reason) = allowance {
                        allowances += 1;
                        tcx.dcx().note(format!(
                            "checkchildren allowance at {}: {}",
                            tcx.sess.source_map().span_to_diagnostic_string(ret.span),
                            reason
                        ));
                        continue;
                    }
                    findings += 1;
                    let sm = tcx.sess.source_map();
                    eprintln!(
                        "{}: checkchildren: {} returns before a child check at {}",
                        sm.span_to_diagnostic_string(ret.span),
                        tcx.def_path_str(handler),
                        sm.span_to_diagnostic_string(call.span)
                    );
                }
            }
        }
        tcx.dcx().note(format!(
            "checkchildren: {workers} workers, {} dispatched methods, {findings} early returns, {allowances} reviewed allowances, {invalid_allowances} invalid allowances",
            handlers.len()
        ));
        self.failed = findings != 0 || invalid_allowances != 0;
        Compilation::Continue
    }
}
fn main() -> ExitCode {
    let mut args: Vec<_> = std::env::args().collect();
    // Cargo's RUSTC_WORKSPACE_WRAPPER adds the actual rustc as argv[1].
    if args.get(1).is_some_and(|arg| {
        std::path::Path::new(arg)
            .file_stem()
            .is_some_and(|name| name == "rustc")
    }) {
        args.remove(0);
    }
    let enabled = args.windows(2).any(|pair| {
        pair[0] == "--crate-name"
            && (pair[1] == "tsr_checker" || pair[1] == "checkchildren_fixture")
    });
    let mut callbacks = CheckChildren {
        enabled,
        ..Default::default()
    };
    let result =
        rustc_driver::catch_with_exit_code(|| rustc_driver::run_compiler(&args, &mut callbacks));
    if result == ExitCode::SUCCESS && callbacks.failed {
        ExitCode::from(1)
    } else {
        result
    }
}
