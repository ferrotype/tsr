use super::diagnostics::{accessibility_diagnostic, SymbolAccessibilityDiagnostic};
use ts_ast::{AstView, JsString, NodeId, SymbolFlags, SymbolId};
use ts_printer::emit_resolver::{
    DeclarationSymbolTracker, DeclarationTrackerEvent, SymbolAccessibility as A,
    SymbolAccessibilityResult,
};

/// Selector inputs are fixed by the declaration context; only these two native
/// accessibility dimensions vary during node serialization. Precomputing keeps
/// TrackSymbol's synchronous boolean independent of a second checker borrow.
/// Native selectors are lazy closures: a context may contain syntax only valid
/// for a diagnostic that is never requested. Preserve failures until selected.
#[derive(Clone)]
pub(super) struct Selector {
    variants: [Result<Option<SymbolAccessibilityDiagnostic>, ts_arena::Error>; 4],
}
impl Default for Selector {
    fn default() -> Self {
        Self {
            variants: [Ok(None); 4],
        }
    }
}
impl Selector {
    pub fn new(view: AstView<'_>, node: NodeId, name_context: bool) -> Self {
        let mut variants = std::array::from_fn(|_| Ok(None));
        for (index, variant) in variants.iter_mut().enumerate() {
            let mut result = SymbolAccessibilityResult::accessible();
            result.accessibility = if index & 2 == 0 {
                A::NotAccessible
            } else {
                A::CannotBeNamed
            };
            if index & 1 != 0 {
                result.error_module_name = JsString::from_bytes(b"module".as_slice());
            }
            *variant = accessibility_diagnostic(view, node, name_context, &result);
        }
        Self { variants }
    }
    pub fn fixed(info: SymbolAccessibilityDiagnostic) -> Self {
        Self {
            variants: [Ok(Some(info)); 4],
        }
    }
    fn select(
        &self,
        result: &SymbolAccessibilityResult,
    ) -> Result<Option<SymbolAccessibilityDiagnostic>, ts_arena::Error> {
        let index = usize::from(result.accessibility == A::CannotBeNamed) * 2
            + usize::from(!result.error_module_name.is_empty());
        self.variants[index]
    }
}

pub(super) enum Pending {
    SelectorError(ts_arena::Error),
    Accessibility(SymbolAccessibilityDiagnostic, SymbolAccessibilityResult),
    Report(DeclarationTrackerEvent),
}

#[derive(Default)]
pub(super) struct Tracker {
    pub selector: Selector,
    pub error_name: Option<NodeId>,
    pub late_marked: Vec<NodeId>,
    pub pending: Vec<Pending>,
    pub fallback: Vec<Option<NodeId>>,
    pub watched_class: Option<SymbolId>,
    pub class_tracked: bool,
}
impl Tracker {
    // port: tsc/internal/transformers/declarations/tracker.go:SymbolTrackerImpl.handleSymbolAccessibilityError
    pub fn accessibility(&mut self, result: SymbolAccessibilityResult) -> bool {
        match result.accessibility {
            A::Accessible => {
                for alias in result.aliases_to_make_visible {
                    if !self.late_marked.contains(&alias) {
                        self.late_marked.push(alias);
                    }
                }
            }
            A::NotResolved => {}
            A::NotAccessible | A::CannotBeNamed => match self.selector.select(&result) {
                Ok(Some(selected)) => {
                    self.pending.push(Pending::Accessibility(selected, result));
                    return true;
                }
                Err(error) => {
                    self.pending.push(Pending::SelectorError(error));
                    return true;
                }
                Ok(None) => {}
            },
        }
        false
    }
}
impl DeclarationSymbolTracker for Tracker {
    fn track_symbol_without_accessibility(&mut self, symbol: SymbolId) -> bool {
        if self.watched_class == Some(symbol) {
            self.class_tracked = true;
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
        if self.watched_class == Some(symbol) {
            self.class_tracked = true;
            return false;
        }
        self.accessibility(result)
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
        let parsed = ts_parser::parse_source_file(
            ts_jsstring::SourceText::from_loaded_bytes(b"Symbol();".as_slice()),
            ts_core::ScriptKind::TS,
            ts_ast::SourceFileParseOptions {
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
        // an accessibility error. A zero-argument Symbol call must be harmless.
        let mut tracker = Tracker {
            selector: Selector::new(view, call, false),
            ..Default::default()
        };
        assert!(!tracker.accessibility(SymbolAccessibilityResult::accessible()));
        assert!(tracker.pending.is_empty());
        let mut inaccessible = SymbolAccessibilityResult::accessible();
        inaccessible.accessibility = A::NotAccessible;
        assert!(tracker.accessibility(inaccessible));
        assert!(matches!(
            tracker.pending.as_slice(),
            [Pending::SelectorError(ts_arena::Error::InvalidGraph)]
        ));
        Ok(())
    }
}
