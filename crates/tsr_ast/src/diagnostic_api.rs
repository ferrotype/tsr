//! The later-phase diagnostic API of `tsc/internal/ast/diagnostic.go`.
//!
//! Ports of `tsc/internal/ast/diagnostic.go`, witnessed by the `diagnostics` group of the Phase 1
//! operation tables (`docs/PHASE1-mutation-witnesses.md`, section 9).
use crate::{AstView, Diagnostic, JsString, NodeId};
use std::sync::Arc;
use tsr_arena::Error;
use tsr_core::TextRange;
use tsr_diagnostics::AdHocMessage;
use tsr_locale::Locale;

/// Go's `RepopulateDiagnosticInfo`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RepopulateDiagnosticInfo {
    pub kind: i32,
    pub module_reference: JsString,
    pub mode: i32,
    pub package_name: JsString,
}

impl Diagnostic {
    /// port: tsc/internal/ast/diagnostic.go:Diagnostic.RepopulateInfo
    pub fn repopulate_info(&self) -> Option<Arc<RepopulateDiagnosticInfo>> {
        self.repopulate_info.clone()
    }

    /// port: tsc/internal/ast/diagnostic.go:Diagnostic.SetRepopulateInfo
    pub fn set_repopulate_info(&mut self, info: Option<Arc<RepopulateDiagnosticInfo>>) {
        self.repopulate_info = info;
    }

    /// port: tsc/internal/ast/diagnostic.go:NewDiagnosticFromSerialized
    #[allow(clippy::too_many_arguments)]
    pub fn from_serialized(
        file: Option<NodeId>,
        loc: TextRange,
        code: i32,
        category: i32,
        message_key: JsString,
        message_args: Vec<JsString>,
        message_chain: Vec<Arc<Self>>,
        related_information: Vec<Arc<Self>>,
        reports_unnecessary: bool,
        reports_deprecated: bool,
        skipped_on_no_emit: bool,
    ) -> Self {
        Self {
            file,
            loc,
            code,
            category,
            source: JsString::default(),
            message: None,
            message_text: JsString::default(),
            message_key,
            message_args,
            message_chain,
            related_information,
            reports_unnecessary,
            reports_deprecated,
            skipped_on_no_emit,
            ad_hoc_message: None,
            repopulate_info: None,
        }
    }

    /// port: tsc/internal/ast/diagnostic.go:NewDiagnosticFromText
    #[allow(clippy::too_many_arguments)]
    pub fn from_text(
        file: Option<NodeId>,
        loc: TextRange,
        code: i32,
        category: i32,
        text: &[u8],
        message_chain: Vec<Arc<Self>>,
        related_information: Vec<Arc<Self>>,
        reports_unnecessary: bool,
        reports_deprecated: bool,
    ) -> Self {
        Self {
            file,
            loc,
            code,
            category,
            source: JsString::default(),
            message: None,
            message_text: JsString::default(),
            message_key: JsString::default(),
            message_args: Vec::new(),
            message_chain,
            related_information,
            reports_unnecessary,
            reports_deprecated,
            skipped_on_no_emit: false,
            ad_hoc_message: Some(Arc::new(AdHocMessage::new(text))),
            repopulate_info: None,
        }
    }

    /// Go's `displayMessageArgs`: a virtual name a content mapper aliases is
    /// shown as its original spelling. `view` reads the diagnostic's file.
    /// port: tsc/internal/ast/diagnostic.go:Diagnostic.displayMessageArgs
    fn display_message_args(&self, view: Option<AstView<'_>>) -> Result<Vec<JsString>, Error> {
        let (Some(file), Some(view)) = (self.file, view) else {
            return Ok(self.message_args.clone());
        };
        if !self.source.is_empty() {
            return Ok(self.message_args.clone());
        }
        let state = view.source_file(file)?;
        let Some(segment) = alias_for_virtual_span(state.span_map(), self.loc) else {
            return Ok(self.message_args.clone());
        };
        let virtual_text = state.text().as_bytes();
        let original_text = state.original_text();
        let in_range = |start: i32, end: i32, length: usize| {
            start >= 0 && usize::try_from(end).is_ok_and(|end| end <= length)
        };
        if !in_range(
            segment.virtual_start,
            segment.virtual_end,
            virtual_text.len(),
        ) || !in_range(
            segment.original_start,
            segment.original_end,
            original_text.len(),
        ) {
            return Ok(self.message_args.clone());
        }
        let span = |text: &[u8], start: i32, end: i32| -> Vec<u8> {
            text[usize::try_from(start).unwrap_or(0)..usize::try_from(end).unwrap_or(0)].to_vec()
        };
        let virtual_name = span(virtual_text, segment.virtual_start, segment.virtual_end);
        let original_name = span(original_text, segment.original_start, segment.original_end);
        let mut result: Option<Vec<JsString>> = None;
        for (index, argument) in self.message_args.iter().enumerate() {
            if argument.as_bytes() != virtual_name.as_slice() {
                continue;
            }
            let result = result.get_or_insert_with(|| self.message_args.clone());
            result[index] = JsString::from_bytes(original_name.as_slice());
        }
        Ok(result.unwrap_or_else(|| self.message_args.clone()))
    }

    /// port: tsc/internal/ast/diagnostic.go:Diagnostic.Localize
    pub fn localize(&self, view: Option<AstView<'_>>, locale: &Locale) -> Result<Vec<u8>, Error> {
        if self.message.is_none() && self.ad_hoc_message.is_none() && !self.message_text.is_empty()
        {
            return Ok(self.message_text.as_bytes().to_vec());
        }
        let args = self.display_message_args(view)?;
        let args: Vec<&[u8]> = args.iter().map(JsString::as_bytes).collect();
        Ok(match &self.ad_hoc_message {
            Some(message) => message.localize(locale, &args),
            None => {
                tsr_diagnostics::localize(locale, self.message, self.message_key.as_bytes(), &args)
            }
        })
    }

    /// Go's `String()`: the message in the default locale.
    /// port: tsc/internal/ast/diagnostic.go:Diagnostic.String
    pub fn string(&self, view: Option<AstView<'_>>) -> Result<Vec<u8>, Error> {
        if self.message.is_none() && self.ad_hoc_message.is_none() && !self.message_text.is_empty()
        {
            return Ok(self.message_text.as_bytes().to_vec());
        }
        let args = self.display_message_args(view)?;
        let args: Vec<&[u8]> = args.iter().map(JsString::as_bytes).collect();
        let locale = Locale::default();
        Ok(match &self.ad_hoc_message {
            Some(message) => message.localize(&locale, &args),
            None => {
                tsr_diagnostics::localize(&locale, self.message, self.message_key.as_bytes(), &args)
            }
        })
    }
}

/// Go's `SpanMap.AliasForVirtualSpan`: the alias segment spanning exactly
/// `loc`, if any.
fn alias_for_virtual_span(
    segments: Option<&[crate::SpanSegment]>,
    loc: TextRange,
) -> Option<crate::SpanSegment> {
    /// Go's `spanmap.KindAlias`.
    const KIND_ALIAS: i32 = 2;
    let segments = segments?;
    // Go converts the position to a TextPos (int32).
    #[allow(clippy::cast_possible_truncation)]
    let (index, inside) = crate::span_map::segment_at(segments, loc.pos() as i32);
    if !inside {
        return None;
    }
    let segment = segments[index?];
    (segment.kind == KIND_ALIAS
        && loc.pos() == i64::from(segment.virtual_start)
        && loc.end() == i64::from(segment.virtual_end))
    .then_some(segment)
}

/// Go's `DiagnosticsCollection`, with the file buckets keyed by path.
#[derive(Debug, Default)]
pub struct DiagnosticsCollection {
    non_file: Vec<Arc<Diagnostic>>,
    files: std::collections::HashMap<Vec<u8>, Vec<Arc<Diagnostic>>>,
}

impl DiagnosticsCollection {
    /// Go's `Add`, without the location index (its home is the program's
    /// include collection in tsr_compiler): `path` is the file's path.
    pub fn add(&mut self, diagnostic: Arc<Diagnostic>, path: Option<&[u8]>) {
        match path {
            Some(path) => self
                .files
                .entry(path.to_vec())
                .or_default()
                .push(diagnostic),
            None => self.non_file.push(diagnostic),
        }
    }

    /// port: tsc/internal/ast/diagnostic.go:DiagnosticsCollection.GetDiagnostics
    pub fn get_diagnostics<'a>(
        &self,
        file_name: &impl Fn(NodeId) -> Result<&'a [u8], Error>,
    ) -> Result<Vec<Arc<Diagnostic>>, Error> {
        let mut diagnostics = self.non_file.clone();
        for bucket in self.files.values() {
            diagnostics.extend(bucket.iter().cloned());
        }
        let mut failure = None;
        tsr_core::sort_like_go(&mut diagnostics, &mut |left, right| {
            crate::compare_diagnostics(left, right, file_name).unwrap_or_else(|error| {
                failure.get_or_insert(error);
                std::cmp::Ordering::Equal
            })
        });
        match failure {
            Some(error) => Err(error),
            None => Ok(diagnostics),
        }
    }
}
