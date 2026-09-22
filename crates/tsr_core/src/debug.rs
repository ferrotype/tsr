//! Shared upstream debug failures. Message arguments retain whether their Go
//! value is a string: `fmt.Sprint` inserts a space only between two nonstrings.
//! Callers provide the display view of other values; this module does not
//! implement Go's general-purpose reflection formatter.

use std::fmt::{self, Display, Write};

/// One borrowed message operand. A Go Stringer remains a nonstring operand,
/// even though its display implementation writes text.
#[derive(Clone, Copy)]
pub enum Argument<'a> {
    String(&'a str),
    Value(&'a dyn Display),
    Nil,
}

impl Display for Argument<'_> {
    fn fmt(&self, output: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::String(text) => output.write_str(text),
            Self::Value(value) => value.fmt(output),
            Self::Nil => output.write_str("<nil>"),
        }
    }
}

/// The display views needed by `AssertNever`. `kind_string`, when present,
/// takes precedence over the ordinary Stringer/scalar display view.
#[derive(Clone, Copy)]
pub struct Member<'a> {
    value: Argument<'a>,
    kind_string: Option<&'a dyn Display>,
}

impl<'a> Member<'a> {
    pub fn new(value: Argument<'a>) -> Self {
        Self {
            value,
            kind_string: None,
        }
    }

    pub fn with_kind_string(value: Argument<'a>, kind_string: &'a dyn Display) -> Self {
        Self {
            value,
            kind_string: Some(kind_string),
        }
    }
}

fn sprint(message: &[Argument<'_>]) -> String {
    let mut output = String::new();
    let mut previous_was_string = true;
    for argument in message {
        let is_string = matches!(argument, Argument::String(_));
        if !previous_was_string && !is_string {
            output.push(' ');
        }
        write!(output, "{argument}").expect("debug argument formatting failed");
        previous_was_string = is_string;
    }
    output
}

/// Panics with the pinned debug prefix and optional reason.
// port: tsc/internal/debug/debug.go:Fail
#[cold]
#[track_caller]
pub fn fail(reason: &str) -> ! {
    match reason {
        "" => panic!("Debug failure."),
        reason => panic!("Debug failure. {reason}"),
    }
}

// port: tsc/internal/debug/debug.go:FailBadSyntaxKind
#[cold]
#[track_caller]
pub fn fail_bad_syntax_kind(kind_string: &dyn Display, message: &[Argument<'_>]) -> ! {
    let message = if message.is_empty() {
        "Unexpected node.".to_owned()
    } else {
        sprint(message)
    };
    fail(&format!("{message}\nNode {kind_string} was unexpected."));
}

// port: tsc/internal/debug/debug.go:AssertNever
#[cold]
#[track_caller]
pub fn assert_never(member: Member<'_>, message: &[Argument<'_>]) -> ! {
    let message = if message.is_empty() {
        "Illegal value:".to_owned()
    } else {
        sprint(message)
    };
    let detail: &dyn Display = match member.kind_string {
        Some(kind) => kind,
        None => &member.value,
    };
    fail(&format!("{message} {detail}"));
}

/// A true condition performs no formatting or allocation.
// port: tsc/internal/debug/debug.go:Assert
#[inline]
#[track_caller]
pub fn assert(value: bool, message: &[Argument<'_>]) {
    if !value {
        assert_slow(message);
    }
}

// port: tsc/internal/debug/debug.go:assertSlow
#[cold]
#[inline(never)]
#[track_caller]
fn assert_slow(message: &[Argument<'_>]) -> ! {
    if message.is_empty() {
        fail("False expression.");
    }
    fail(&format!("False expression: {}", sprint(message)));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::panic::{catch_unwind, AssertUnwindSafe};

    fn panic_message(run: impl FnOnce()) -> String {
        let payload = catch_unwind(AssertUnwindSafe(run)).unwrap_err();
        payload
            .downcast_ref::<String>()
            .cloned()
            .or_else(|| {
                payload
                    .downcast_ref::<&str>()
                    .map(|text| (*text).to_owned())
            })
            .expect("debug failures carry string payloads")
    }

    struct NeverFormat;
    impl Display for NeverFormat {
        fn fmt(&self, _: &mut fmt::Formatter<'_>) -> fmt::Result {
            panic!("the unused display view must not run")
        }
    }

    #[test]
    fn true_assert_does_not_format_its_message() {
        assert(true, &[Argument::Value(&NeverFormat)]);
    }

    #[test]
    fn kind_string_precedes_the_stringer_view() {
        let member = Member::with_kind_string(Argument::Value(&NeverFormat), &"Kind");
        assert_eq!(
            panic_message(|| assert_never(member, &[])),
            "Debug failure. Illegal value: Kind"
        );
    }

    #[test]
    fn sprint_spacing_and_present_empty_message_match_go() {
        assert_eq!(
            panic_message(|| assert(
                false,
                &[
                    Argument::String("a"),
                    Argument::Value(&1),
                    Argument::Value(&2),
                    Argument::String("b"),
                ]
            )),
            "Debug failure. False expression: a1 2b"
        );
        assert_eq!(
            panic_message(|| assert(false, &[Argument::String("")])),
            "Debug failure. False expression: "
        );
        assert_eq!(
            panic_message(|| assert(false, &[])),
            "Debug failure. False expression."
        );
    }
}
