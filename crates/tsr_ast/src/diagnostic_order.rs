//! Pinned diagnostic ordering and equality. File names are borrowed from the
//! caller's retained source files; diagnostic node IDs do not acquire owners.
use crate::{Diagnostic, NodeId};
use std::{cmp::Ordering, sync::Arc};

/// The source helper orders by FileName(), not the normalized source-file Path.
/// port: tsc/internal/ast/diagnostic.go:getDiagnosticPath
fn diagnostic_path<'a, E>(
    diagnostic: &Diagnostic,
    file_name: &impl Fn(NodeId) -> Result<&'a [u8], E>,
) -> Result<&'a [u8], E> {
    diagnostic.file.map_or(Ok(b""), file_name)
}

/// Flags and related information are deliberately absent from this comparison.
/// port: tsc/internal/ast/diagnostic.go:EqualDiagnosticsNoRelatedInfo
pub fn equal_diagnostics_no_related_info<'a, E>(
    left: &Diagnostic,
    right: &Diagnostic,
    file_name: &impl Fn(NodeId) -> Result<&'a [u8], E>,
) -> Result<bool, E> {
    if std::ptr::eq(left, right) {
        return Ok(true);
    }
    Ok(
        diagnostic_path(left, file_name)? == diagnostic_path(right, file_name)?
            && left.loc == right.loc
            && left.code == right.code
            && left.category == right.category
            && left.source == right.source
            && left.message_identity() == right.message_identity()
            && left.message_args == right.message_args
            && equal_chains(&left.message_chain, &right.message_chain),
    )
}

/// Equality observes chain codes even though `compare_diagnostics` does not.
/// Sorting equality therefore cannot replace this source equality operation.
/// port: tsc/internal/ast/diagnostic.go:EqualDiagnostics
pub fn equal_diagnostics<'a, E>(
    left: &Diagnostic,
    right: &Diagnostic,
    file_name: &impl Fn(NodeId) -> Result<&'a [u8], E>,
) -> Result<bool, E> {
    if std::ptr::eq(left, right) {
        return Ok(true);
    }
    if !equal_diagnostics_no_related_info(left, right, file_name)?
        || left.related_information.len() != right.related_information.len()
    {
        return Ok(false);
    }
    for (left, right) in left
        .related_information
        .iter()
        .zip(&right.related_information)
    {
        if !equal_diagnostics(left, right, file_name)? {
            return Ok(false);
        }
    }
    Ok(true)
}

fn equal_chains(left: &[Arc<Diagnostic>], right: &[Arc<Diagnostic>]) -> bool {
    left.len() == right.len()
        && left
            .iter()
            .zip(right)
            .all(|(left, right)| equal_message_chain(left, right))
}

/// port: tsc/internal/ast/diagnostic.go:equalMessageChain
fn equal_message_chain(left: &Diagnostic, right: &Diagnostic) -> bool {
    std::ptr::eq(left, right)
        || (left.code == right.code
            && left.message_args == right.message_args
            && equal_chains(&left.message_chain, &right.message_chain))
}

/// Larger chains sort first, including recursively at the first different depth.
/// port: tsc/internal/ast/diagnostic.go:compareMessageChainSize
fn compare_message_chain_size(left: &[Arc<Diagnostic>], right: &[Arc<Diagnostic>]) -> Ordering {
    let order = right.len().cmp(&left.len());
    if order != Ordering::Equal {
        return order;
    }
    for (left, right) in left.iter().zip(right) {
        let order = compare_message_chain_size(&left.message_chain, &right.message_chain);
        if order != Ordering::Equal {
            return order;
        }
    }
    Ordering::Equal
}

/// Called only after equal recursive shapes have been established. The pinned
/// source compares arguments here; chain codes and message text are ignored.
/// port: tsc/internal/ast/diagnostic.go:compareMessageChainContent
fn compare_message_chain_content(left: &[Arc<Diagnostic>], right: &[Arc<Diagnostic>]) -> Ordering {
    for (left, right) in left.iter().zip(right) {
        let order = left.message_args.cmp(&right.message_args);
        if order != Ordering::Equal {
            return order;
        }
        let order = compare_message_chain_content(&left.message_chain, &right.message_chain);
        if order != Ordering::Equal {
            return order;
        }
    }
    Ordering::Equal
}

/// port: tsc/internal/ast/diagnostic.go:compareRelatedInfo
fn compare_related_info<'a, E>(
    left: &[Arc<Diagnostic>],
    right: &[Arc<Diagnostic>],
    file_name: &impl Fn(NodeId) -> Result<&'a [u8], E>,
) -> Result<Ordering, E> {
    let order = right.len().cmp(&left.len());
    if order != Ordering::Equal {
        return Ok(order);
    }
    for (left, right) in left.iter().zip(right) {
        let order = compare_diagnostics(left, right, file_name)?;
        if order != Ordering::Equal {
            return Ok(order);
        }
    }
    Ok(Ordering::Equal)
}

/// Compare diagnostics by the pinned compiler's source order. The callback
/// resolves only visited non-nil file identities and may return borrowed bytes.
/// port: tsc/internal/ast/diagnostic.go:CompareDiagnostics
pub fn compare_diagnostics<'a, E>(
    left: &Diagnostic,
    right: &Diagnostic,
    file_name: &impl Fn(NodeId) -> Result<&'a [u8], E>,
) -> Result<Ordering, E> {
    if std::ptr::eq(left, right) {
        return Ok(Ordering::Equal);
    }
    let order = diagnostic_path(left, file_name)?.cmp(diagnostic_path(right, file_name)?);
    if order != Ordering::Equal {
        return Ok(order);
    }
    // Pos/End and int32 codes widen before subtraction in the 64-bit Go source.
    // Ordering their values preserves the sign without arithmetic overflow.
    let order = left
        .loc
        .pos()
        .cmp(&right.loc.pos())
        .then_with(|| left.loc.end().cmp(&right.loc.end()))
        .then_with(|| left.code.cmp(&right.code))
        .then_with(|| left.category.cmp(&right.category))
        .then_with(|| left.source.cmp(&right.source))
        .then_with(|| left.message_identity().cmp(right.message_identity()))
        .then_with(|| left.message_args.cmp(&right.message_args))
        .then_with(|| compare_message_chain_size(&left.message_chain, &right.message_chain))
        .then_with(|| compare_message_chain_content(&left.message_chain, &right.message_chain));
    if order != Ordering::Equal {
        return Ok(order);
    }
    compare_related_info(
        &left.related_information,
        &right.related_information,
        file_name,
    )
}

#[cfg(test)]
#[path = "diagnostic_order_tests.rs"]
mod tests;
