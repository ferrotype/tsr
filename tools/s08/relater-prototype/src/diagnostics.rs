//! Native diagnostic identity and linked relation-error chains. The relation
//! builds these records directly from pinned messages, never by parsing text.

use std::rc::Rc;
use tsr_diagnostics::{self as d, Message};

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DiagnosticLocation {
    pub file: Option<String>,
    pub pos: isize,
    pub end: isize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Diagnostic {
    pub location: DiagnosticLocation,
    pub message: &'static Message,
    pub args: Vec<String>,
    pub chain: Vec<Self>,
    pub related: Vec<Self>,
}

impl Diagnostic {
    pub(crate) fn new(
        location: DiagnosticLocation,
        message: &'static Message,
        args: Vec<String>,
    ) -> Self {
        Self {
            location,
            message,
            args,
            chain: Vec::new(),
            related: Vec::new(),
        }
    }

    /// Debugging only. Native serialization uses the message key and arguments;
    /// its `messageText` remains empty for compiler-produced diagnostics.
    pub fn display_text(&self) -> String {
        let mut result = String::new();
        let mut rest = self.message.text;
        while let Some(start) = rest.find('{') {
            result.push_str(&rest[..start]);
            rest = &rest[start..];
            if let Some(end) = rest.find('}') {
                if let Ok(index) = rest[1..end].parse::<usize>() {
                    if let Some(arg) = self.args.get(index) {
                        result.push_str(arg);
                        rest = &rest[end + 1..];
                        continue;
                    }
                }
            }
            result.push('{');
            rest = &rest[1..];
        }
        result.push_str(rest);
        for child in &self.chain {
            result.push_str(" | ");
            result.push_str(&child.display_text());
        }
        result
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Entry {
    message: &'static Message,
    args: Vec<String>,
    next: Option<Rc<Self>>,
}

/// Persistent snapshots preserve error state when reporting collapses existing
/// chain entries, as Go's saved `*ErrorChain` does. A vector-length snapshot
/// cannot restore a chain after that transformation.
#[derive(Clone, Debug, Default)]
pub(crate) struct ErrorChain(Option<Rc<Entry>>);

impl PartialEq for ErrorChain {
    fn eq(&self, other: &Self) -> bool {
        match (&self.0, &other.0) {
            (Some(left), Some(right)) => Rc::ptr_eq(left, right),
            (None, None) => true,
            _ => false,
        }
    }
}
impl Eq for ErrorChain {}

impl ErrorChain {
    fn entry(&self, mut index: usize) -> Option<&Entry> {
        let mut entry = self.0.as_deref();
        while index != 0 {
            entry = entry?.next.as_deref();
            index -= 1;
        }
        entry
    }

    fn code(&self, index: usize) -> Option<i32> {
        self.entry(index).map(|entry| entry.message.code)
    }

    pub(crate) fn args_match(&self, args: &[Option<&str>]) -> bool {
        self.entry(0).is_some_and(|entry| {
            args.iter().enumerate().all(|(index, arg)| {
                arg.is_none_or(|arg| entry.args.get(index).is_some_and(|value| value == arg))
            })
        })
    }

    pub(crate) fn suppress_relation(&self, source: &str, target: &str) -> bool {
        match self.code(0) {
            Some(2353 | 2561) => true,
            Some(2859 | 4104 | 2739 | 2740) => self.args_match(&[Some(source), Some(target)]),
            Some(2741) => self.args_match(&[None, Some(source), Some(target)]),
            _ => false,
        }
    }

    /// port: tsc/internal/checker/relater.go:reportError
    pub(crate) fn report(&mut self, mut message: &'static Message, mut args: Vec<String>) {
        if message == d::Types_of_property_0_are_incompatible {
            if matches!(self.code(0), Some(2353 | 2561)) {
                return;
            }
            let property = args.first().map_or("", String::as_str);
            let property = property_name(property);
            let argument = match self.code(1) {
                Some(2204) => Some(format!("{property}()")),
                Some(2205) => Some(format!("new {property}()")),
                Some(2202) => Some(format!("{property}(...)")),
                Some(2203) => Some(format!("new {property}(...)")),
                _ => None,
            };
            if let Some(argument) = argument {
                message = d::The_types_returned_by_0_are_incompatible_between_these_types;
                args[0] = argument;
                self.0 = self.entry(1).and_then(|entry| entry.next.clone());
            }
            if matches!(self.code(1), Some(2326 | 2200 | 2201)) {
                let head = property_name(&args[0]);
                let tail = property_name(&self.entry(1).expect("matched entry").args[0]);
                let argument = add_to_dotted_name(&head, &tail);
                self.0 = self.entry(1).and_then(|entry| entry.next.clone());
                if message == d::Types_of_property_0_are_incompatible {
                    message = d::The_types_of_0_are_incompatible_between_these_types;
                }
                self.report(message, vec![argument]);
                return;
            }
        }
        self.0 = Some(Rc::new(Entry {
            message,
            args,
            next: self.0.clone(),
        }));
    }

    /// port: tsc/internal/checker/relater.go:createDiagnosticChainFromErrorChain
    pub(crate) fn diagnostic(
        &self,
        location: &DiagnosticLocation,
        related: &[Diagnostic],
    ) -> Option<Diagnostic> {
        fn build(
            entry: Option<&Entry>,
            location: &DiagnosticLocation,
            related: &[Diagnostic],
        ) -> Option<Diagnostic> {
            let entry = entry?;
            let next = build(entry.next.as_deref(), location, related);
            if entry.message.elided_in_compatibility_pyramid {
                return next;
            }
            let mut diagnostic =
                Diagnostic::new(location.clone(), entry.message, entry.args.clone());
            diagnostic.related = related.to_vec();
            diagnostic.chain.extend(next);
            Some(diagnostic)
        }
        build(self.0.as_deref(), location, related)
    }
}

fn property_name(argument: &str) -> String {
    if argument.starts_with(['\'', '"', '`']) {
        format!("[{argument}]")
    } else {
        argument.to_owned()
    }
}

/// port: tsc/internal/checker/relater.go:addToDottedName
fn add_to_dotted_name(head: &str, tail: &str) -> String {
    let head = if head.starts_with("new ") {
        format!("({head})")
    } else {
        head.to_owned()
    };
    let mut position = 0;
    loop {
        if tail[position..].starts_with('(') {
            position += 1;
        } else if tail[position..].starts_with("new ") {
            position += 4;
        } else {
            break;
        }
    }
    let (prefix, suffix) = tail.split_at(position);
    let separator = if suffix.starts_with('[') { "" } else { "." };
    format!("{prefix}{head}{separator}{suffix}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{flags, Checker, Mode, FALSE};

    fn named_object(
        checker: &Checker,
        name: &str,
        property: &str,
        ty: &Rc<crate::TypeCell>,
    ) -> Rc<crate::TypeCell> {
        let property: Rc<str> = property.into();
        let ty = Rc::downgrade(ty);
        checker.graph.allocate_full(
            flags::OBJECT,
            crate::object_flags::ANONYMOUS,
            name.into(),
            None,
            Some(checker.graph.len() as u64 + 1),
            None,
            false,
            Vec::new(),
            true,
            Some(Box::new(move |_, _| {
                Ok(crate::Structure {
                    members: vec![crate::Member {
                        name: property.clone(),
                        name_type: None,
                        optional: false,
                        readonly: false,
                        class_member: true,
                        r#type: ty.clone().into(),
                    }],
                    ..Default::default()
                })
            })),
        )
    }

    fn location(pos: isize, end: isize) -> DiagnosticLocation {
        DiagnosticLocation {
            file: Some("/fixture.ts".into()),
            pos,
            end,
        }
    }

    #[test]
    fn recursive_mismatch_preserves_native_identity_location_and_chain_order() {
        // Frozen recursive-mismatch observation: A's name is 97..98. The
        // declared types are Left/Right, so the diagnostic uses those names.
        let checker = Checker::new();
        let string = checker.graph.primitive(flags::STRING, "string");
        let number = checker.graph.primitive(flags::NUMBER, "number");
        let left = named_object(&checker, "Left", "value", &string);
        let right = named_object(&checker, "Right", "value", &number);
        assert_eq!(
            checker
                .check_type_related_to_at(&left, &right, Mode::Assignable, Some(location(97, 98)))
                .unwrap(),
            (FALSE, false)
        );
        let diagnostics = checker.structured_diagnostics();
        let root = &diagnostics[0];
        assert_eq!(root.location, location(97, 98));
        assert_eq!(root.message.code, 2322);
        assert_eq!(root.message.category as i32, 1);
        assert_eq!(root.message.key, "Type_0_is_not_assignable_to_type_1_2322");
        assert_eq!(root.args, ["Left", "Right"]);
        let property = &root.chain[0];
        assert_eq!(property.message.code, 2326);
        assert_eq!(property.args, ["value"]);
        let primitive = &property.chain[0];
        assert_eq!(primitive.message.code, 2322);
        assert_eq!(primitive.args, ["string", "number"]);
        assert_eq!(primitive.location, root.location);
        assert!(primitive.chain.is_empty());
        assert!(root.related.is_empty());
    }

    #[test]
    fn missing_member_elides_redundant_head_and_preserves_declaration_related_info() {
        let checker = Checker::new();
        let string = checker.graph.primitive(flags::STRING, "string");
        let source = named_object(&checker, "Source", "common", &string);
        let target = named_object(&checker, "Target", "required", &string);
        checker.register_member_declaration(&target, "required", location(24, 40));
        checker
            .check_type_related_to_at(&source, &target, Mode::Assignable, Some(location(5, 6)))
            .unwrap();
        let diagnostics = checker.structured_diagnostics();
        let diagnostic = &diagnostics[0];
        assert_eq!(diagnostic.message.code, 2741);
        assert_eq!(diagnostic.args, ["required", "Source", "Target"]);
        assert!(diagnostic.chain.is_empty());
        assert_eq!(diagnostic.related.len(), 1);
        assert_eq!(diagnostic.related[0].message.code, 2728);
        assert_eq!(diagnostic.related[0].args, ["required"]);
        assert_eq!(diagnostic.related[0].location, location(24, 40));
    }

    #[test]
    fn property_path_collapse_retains_native_snapshot_and_leaf() {
        let mut chain = ErrorChain::default();
        chain.report(
            d::Type_0_is_not_assignable_to_type_1,
            vec!["string".into(), "number".into()],
        );
        chain.report(
            d::Types_of_property_0_are_incompatible,
            vec!["value".into()],
        );
        chain.report(
            d::Type_0_is_not_assignable_to_type_1,
            vec!["InnerA".into(), "InnerB".into()],
        );
        let saved = chain.clone();
        chain.report(d::Types_of_property_0_are_incompatible, vec!["next".into()]);
        let diagnostic = chain.diagnostic(&location(5, 6), &[]).unwrap();
        assert_eq!(diagnostic.message.code, 2200);
        assert_eq!(diagnostic.args, ["next.value"]);
        assert_eq!(diagnostic.chain[0].args, ["string", "number"]);
        let saved_diagnostic = saved.diagnostic(&location(5, 6), &[]).unwrap();
        assert_eq!(saved_diagnostic.message.code, 2322);
        assert_eq!(saved_diagnostic.chain[0].args, ["value"]);
    }

    #[test]
    fn signature_pyramid_elision_keeps_elaboration_and_named_return_path() {
        let mut chain = ErrorChain::default();
        chain.report(
            d::Type_0_is_not_assignable_to_type_1,
            vec!["string".into(), "number".into()],
        );
        chain.report(
            d::Call_signatures_with_no_arguments_have_incompatible_return_types_0_and_1,
            vec!["string".into(), "number".into()],
        );
        // Elided signature entry disappears without dropping its leaf.
        assert_eq!(
            chain.diagnostic(&location(5, 6), &[]).unwrap().message.code,
            2322
        );
        chain.report(
            d::Type_0_is_not_assignable_to_type_1,
            vec!["F".into(), "G".into()],
        );
        chain.report(
            d::Types_of_property_0_are_incompatible,
            vec!["invoke".into()],
        );
        let diagnostic = chain.diagnostic(&location(5, 6), &[]).unwrap();
        assert_eq!(diagnostic.message.code, 2201);
        assert_eq!(diagnostic.args, ["invoke()"]);
        assert_eq!(diagnostic.chain[0].args, ["string", "number"]);
    }

    #[test]
    fn localized_debug_render_does_not_expand_placeholders_inside_arguments() {
        let diagnostic = Diagnostic::new(
            location(0, 1),
            d::Type_0_is_not_assignable_to_type_1,
            vec!["{1}".into(), "number".into()],
        );
        assert_eq!(
            diagnostic.display_text(),
            "Type '{1}' is not assignable to type 'number'."
        );
    }
}
