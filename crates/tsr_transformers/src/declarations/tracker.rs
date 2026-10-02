use super::diagnostics::{
    throw_diagnostic, Failure, GetSymbolAccessibilityDiagnostic, SymbolAccessibilityDiagnostic,
};
use tsr_ast::{AstView, JsString, NodeId, SymbolFlags, SymbolId};
use tsr_printer::emit_resolver::{
    DeclarationSymbolTracker, DeclarationTrackerEvent, SymbolAccessibility as A,
    SymbolAccessibilityResult,
};

/// The installed `getSymbolAccessibilityDiagnostic`. The pin's closure reads
/// syntax when an inaccessible symbol is reported, which happens inside a
/// node-builder call that holds the checker; here the closure is called, when
/// it is installed, for the only two result dimensions it reads
/// (`CannotBeNamed`, and whether a module name is present), and its answers
/// are kept. A failure, or the pin's panic, is kept too and raised only if
/// that answer is selected.
#[derive(Clone)]
pub(super) struct Selector {
    variants: [Result<Option<SymbolAccessibilityDiagnostic>, Failure>; 4],
}
impl Selector {
    pub fn new(view: AstView<'_>, getter: GetSymbolAccessibilityDiagnostic) -> Self {
        let variants = std::array::from_fn(|index| {
            let mut result = SymbolAccessibilityResult::accessible();
            result.accessibility = if index & 2 == 0 {
                A::NotAccessible
            } else {
                A::CannotBeNamed
            };
            if index & 1 != 0 {
                result.error_module_name = JsString::from_bytes(b"module".as_slice());
            }
            getter.evaluate(view, &result)
        });
        Self { variants }
    }
    /// `throwDiagnostic`, the source file's context.
    pub fn throw() -> Self {
        Self {
            variants: std::array::from_fn(|_| Err(throw_diagnostic())),
        }
    }
    pub fn fixed(info: SymbolAccessibilityDiagnostic) -> Self {
        Self {
            variants: std::array::from_fn(|_| Ok(Some(info))),
        }
    }
    fn select(
        &self,
        result: &SymbolAccessibilityResult,
    ) -> Result<Option<SymbolAccessibilityDiagnostic>, Failure> {
        let index = usize::from(result.accessibility == A::CannotBeNamed) * 2
            + usize::from(!result.error_module_name.is_empty());
        self.variants[index].clone()
    }
}
impl Default for Selector {
    fn default() -> Self {
        Self::throw()
    }
}

/// A report made while a resolver call held the tracker. The transformer
/// applies them in order once the call returns, so fallback-stack pushes and
/// pops interleave with the reports exactly as they were made.
pub(super) enum Pending {
    SelectorFailure(Failure),
    Accessibility(SymbolAccessibilityDiagnostic, SymbolAccessibilityResult),
    Report(DeclarationTrackerEvent),
}

/// `SymbolTrackerImpl` with the shared state's accessibility context
/// (`getSymbolAccessibilityDiagnostic`, `errorNameNode`,
/// `lateMarkedStatements`). The reports that write diagnostics are the
/// transformer's (`reports.rs`), applied from `pending`.
#[derive(Default)]
pub(super) struct Tracker {
    pub selector: Selector,
    pub error_name: Option<NodeId>,
    pub late_marked: Vec<NodeId>,
    pub pending: Vec<Pending>,
    pub fallback_stack: Vec<Option<NodeId>>,
    pub watched_class_symbol: Option<SymbolId>,
    pub class_symbol_tracked: bool,
}
impl Tracker {
    // port: tsc/internal/transformers/declarations/tracker.go:NewSymbolTracker
    pub fn new() -> Self {
        Self::default()
    }
    // port: tsc/internal/transformers/declarations/tracker.go:SymbolTrackerImpl.PushErrorFallbackNode
    pub fn push_error_fallback_node(&mut self, node: Option<NodeId>) {
        self.fallback_stack.push(node);
    }
    // port: tsc/internal/transformers/declarations/tracker.go:SymbolTrackerImpl.PopErrorFallbackNode
    pub fn pop_error_fallback_node(&mut self) {
        let len = self
            .fallback_stack
            .len()
            .checked_sub(1)
            .expect("runtime error: slice bounds out of range [:-1]");
        self.fallback_stack.truncate(len);
    }
    // port: tsc/internal/transformers/declarations/tracker.go:SymbolTrackerImpl.errorFallbackNode
    pub fn error_fallback_node(&self) -> Option<NodeId> {
        self.fallback_stack.last().copied().flatten()
    }
    // port: tsc/internal/transformers/declarations/tracker.go:SymbolTrackerImpl.errorLocation
    pub fn error_location(&self) -> Option<NodeId> {
        self.error_name.or_else(|| self.error_fallback_node())
    }
    // port: tsc/internal/transformers/declarations/tracker.go:SymbolTrackerImpl.handleSymbolAccessibilityError
    pub fn handle_symbol_accessibility_error(&mut self, result: SymbolAccessibilityResult) -> bool {
        if result.accessibility == A::Accessible {
            // Add aliases back onto the possible imports list if they're not there so we can try them again with updated visibility info
            for alias in result.aliases_to_make_visible {
                if !self.late_marked.contains(&alias) {
                    self.late_marked.push(alias);
                }
            }
            // The checker should issue errors on unresolvable names, skip the declaration emit error for using a private/unreachable name for those
        } else if result.accessibility != A::NotResolved {
            // Report error
            match self.selector.select(&result) {
                Ok(Some(info)) => {
                    self.pending.push(Pending::Accessibility(info, result));
                    return true;
                }
                Ok(None) => {}
                Err(failure) => {
                    self.pending.push(Pending::SelectorFailure(failure));
                    return true;
                }
            }
        }
        false
    }
}
impl DeclarationSymbolTracker for Tracker {
    fn track_symbol_without_accessibility(&mut self, symbol: SymbolId, flags: SymbolFlags) -> bool {
        if flags & tsr_ast::symbol_flags::TYPE_PARAMETER != 0 {
            return true;
        }
        if self.watched_class_symbol == Some(symbol) {
            self.class_symbol_tracked = true;
            true
        } else {
            false
        }
    }
    // port: tsc/internal/transformers/declarations/tracker.go:SymbolTrackerImpl.TrackSymbol
    fn track_symbol(
        &mut self,
        symbol: SymbolId,
        _enclosing: Option<NodeId>,
        _meaning: SymbolFlags,
        result: SymbolAccessibilityResult,
    ) -> bool {
        // When watching for a class expression symbol, record its usage without
        // reporting accessibility errors — the caller will handle visibility by
        // wrapping the class in a namespace.
        if self.watched_class_symbol == Some(symbol) {
            self.class_symbol_tracked = true;
            return false;
        }
        self.handle_symbol_accessibility_error(result)
    }
    fn report(&mut self, event: DeclarationTrackerEvent) {
        self.pending.push(Pending::Report(event));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ordinary_call_context_defers_an_unused_accessibility_selector(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let parsed = tsr_parser::parse_source_file(
            tsr_jsstring::SourceText::from_loaded_bytes(b"Symbol();".as_slice()),
            tsr_core::ScriptKind::TS,
            tsr_ast::SourceFileParseOptions {
                file_name: JsString::from_bytes(b"/case.ts".as_slice()),
                path: JsString::from_bytes(b"/case.ts".as_slice()),
                ..Default::default()
            },
        );
        let view = parsed.view();
        let statements = view.node(parsed.root())?.statement_list().unwrap();
        let statement = view
            .node_slice(view.list(statements)?.nodes())?
            .get(0)
            .unwrap()
            .unwrap();
        let call = view.node(statement)?.expression().unwrap();
        // Native installs a closure for this ordinary call but never evaluates
        // the Object.defineProperty-specific argument lookup unless it reports
        // an accessibility error. A zero-argument Symbol call must be harmless
        // until then; selecting it is the pin's index panic.
        let getter =
            super::super::diagnostics::create_get_symbol_accessibility_diagnostic_for_node(
                view, call,
            )?;
        let mut tracker = Tracker {
            selector: Selector::new(view, getter),
            ..Default::default()
        };
        assert!(!tracker.handle_symbol_accessibility_error(SymbolAccessibilityResult::accessible()));
        assert!(tracker.pending.is_empty());
        let mut inaccessible = SymbolAccessibilityResult::accessible();
        inaccessible.accessibility = A::NotAccessible;
        assert!(tracker.handle_symbol_accessibility_error(inaccessible));
        assert!(matches!(
            tracker.pending.as_slice(),
            [Pending::SelectorFailure(Failure::Panic(message))]
                if message == "runtime error: index out of range [1] with length 0"
        ));
        Ok(())
    }
}
