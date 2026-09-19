//! Parser diagnostic records. File links are identities under the AST owner;
//! message chains retain diagnostics without retaining their containing file.

use crate::{JsString, NodeId};
use std::sync::Arc;
use ts_core::TextRange;
use ts_diagnostics::Message;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diagnostic {
    pub file: Option<NodeId>,
    pub loc: TextRange,
    pub code: i32,
    /// Go's category is an open int32, including serialized unknown values.
    pub category: i32,
    pub source: JsString,
    pub message: Option<&'static Message>,
    pub message_text: JsString,
    pub message_key: JsString,
    pub message_args: Vec<JsString>,
    pub message_chain: Vec<Arc<Self>>,
    pub related_information: Vec<Arc<Self>>,
    pub reports_unnecessary: bool,
    pub reports_deprecated: bool,
    pub skipped_on_no_emit: bool,
}

impl Diagnostic {
    /// Arguments have already crossed the source operation's StringifyArgs
    /// boundary. Preserve their bytes, including scanner spellings and aliases.
    /// port: tsc/internal/ast/diagnostic.go:NewDiagnostic
    pub fn new(
        file: Option<NodeId>,
        loc: TextRange,
        message: &'static Message,
        args: Vec<JsString>,
    ) -> Self {
        Self {
            file,
            loc,
            code: message.code,
            category: message.category as i32,
            source: JsString::default(),
            message: Some(message),
            message_text: JsString::default(),
            message_key: JsString::from_bytes(message.key.as_bytes()),
            message_args: args,
            message_chain: Vec::new(),
            related_information: Vec::new(),
            reports_unnecessary: message.reports_unnecessary,
            reports_deprecated: message.reports_deprecated,
            skipped_on_no_emit: false,
        }
    }

    /// port: tsc/internal/ast/diagnostic.go:NewExternalDiagnostic
    pub fn external(
        file: Option<NodeId>,
        loc: TextRange,
        source: JsString,
        category: i32,
        code: i32,
        message_text: JsString,
    ) -> Self {
        Self {
            file,
            loc,
            code,
            category,
            source,
            message: None,
            message_text,
            message_key: JsString::default(),
            message_args: Vec::new(),
            message_chain: Vec::new(),
            related_information: Vec::new(),
            reports_unnecessary: false,
            reports_deprecated: false,
            skipped_on_no_emit: false,
        }
    }

    /// port: tsc/internal/ast/diagnostic.go:NewCompilerDiagnostic
    pub fn compiler(message: &'static Message, args: Vec<JsString>) -> Self {
        Self::new(None, TextRange::new(-1, -1), message, args)
    }

    /// port: tsc/internal/ast/diagnostic.go:NewDiagnosticChain
    pub fn chain(chain: Option<Arc<Self>>, message: &'static Message, args: Vec<JsString>) -> Self {
        if let Some(chain) = chain {
            let mut result = Self::new(chain.file, chain.loc, message, args);
            result
                .related_information
                .clone_from(&chain.related_information);
            result.message_chain.push(chain);
            result
        } else {
            Self::new(None, TextRange::default(), message, args)
        }
    }

    pub fn message_identity(&self) -> &[u8] {
        if !self.message_text.is_empty() {
            return self.message_text.as_bytes();
        }
        if self.code == -1 {
            if let Some(message) = self.message {
                return message.text.as_bytes();
            }
        }
        self.message_key.as_bytes()
    }
}
