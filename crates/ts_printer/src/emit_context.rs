//! Emit flags, original-node links and generated-name identities retained for a
//! transformation. Factory hooks share these tables with their context; no
//! table lock is held while invoking factory or checker code.

use crate::EmitFlags;
use hashbrown::HashMap;
use std::sync::{
    atomic::{AtomicU32, Ordering},
    Arc, Mutex, MutexGuard,
};
use ts_ast::{node_flags, Factory, FactoryHooks, FactoryMethods, JsString, NodeAccess, NodeId};

/// Native `GeneratedIdentifierFlags` (`generatedidentifierflags.go`).
pub mod generated_identifier_flags {
    pub type Flags = u32;
    pub const NONE: Flags = 0;
    pub const AUTO: Flags = 1;
    pub const LOOP: Flags = 2;
    pub const UNIQUE: Flags = 3;
    pub const NODE: Flags = 4;
    pub const KIND_MASK: Flags = 7;
    pub const RESERVED_IN_NESTED_SCOPES: Flags = 1 << 3;
    pub const OPTIMISTIC: Flags = 1 << 4;
    pub const FILE_LEVEL: Flags = 1 << 5;
    pub const ALLOW_NAME_SUBSTITUTION: Flags = 1 << 6;
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AutoGenerateOptions {
    pub flags: generated_identifier_flags::Flags,
    pub prefix: JsString,
    pub suffix: JsString,
}

/// Identity is independent of the identifier's placeholder text. Clones share
/// this id; separate requests with the same text do not.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct AutoGenerateId(u32);
impl AutoGenerateId {
    pub fn get(self) -> u32 {
        self.0
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AutoGenerateInfo {
    pub flags: generated_identifier_flags::Flags,
    pub id: AutoGenerateId,
    pub prefix: JsString,
    pub suffix: JsString,
    pub node: Option<NodeId>,
}

/// `SynthesizedComment` retains output-only comments separately from source ranges.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SynthesizedComment {
    pub kind: ts_ast::SyntaxKind,
    pub loc: ts_core::TextRange,
    pub has_leading_new_line: bool,
    pub has_trailing_new_line: bool,
    pub text: JsString,
}

#[derive(Debug, Default)]
struct SideTables {
    emit_flags: HashMap<NodeId, EmitFlags, std::hash::RandomState>,
    original: HashMap<NodeId, NodeId, std::hash::RandomState>,
    comment_ranges: HashMap<NodeId, ts_core::TextRange, std::hash::RandomState>,
    leading_comments: HashMap<NodeId, Vec<SynthesizedComment>, std::hash::RandomState>,
    auto_generate: HashMap<NodeId, AutoGenerateInfo, std::hash::RandomState>,
}
impl SideTables {
    fn set_original(&mut self, node: NodeId, original: NodeId) {
        if let Some(existing) = self.original.get(&node) {
            assert_eq!(*existing, original, "original node already set");
            return;
        }
        assert_ne!(node, original, "original node must differ from its update");
        self.original.insert(node, original);
        if let Some(flags) = self.emit_flags.get(&original).copied() {
            self.emit_flags.insert(node, flags);
        }
        if let Some(range) = self.comment_ranges.get(&original).copied() {
            self.comment_ranges.insert(node, range);
        }
    }
}

/// Clones share non-owning metadata; allocation owners remain the caller's responsibility.
#[derive(Clone, Debug, Default)]
pub struct EmitContext {
    tables: Arc<Mutex<SideTables>>,
}

impl EmitContext {
    pub fn new() -> Self {
        Self::default()
    }
    fn tables(&self) -> MutexGuard<'_, SideTables> {
        self.tables.lock().expect("emit context metadata poisoned")
    }

    /// The builder and context form one pair. Moving both is supported; giving
    /// hooks from a different context would separate the metadata from its AST.
    pub fn factory_hooks(&self) -> Arc<dyn FactoryHooks> {
        Arc::new(EmitHooks(Arc::clone(&self.tables)))
    }

    /// Prune a released allocation frame against the caller's retained owners.
    /// Keys are non-owning: the caller must retain every live key and every
    /// original/generated-name target reachable from it. No entry is changed
    /// if a live key would be left with an unretained metadata dependency.
    pub fn retain_metadata(
        &mut self,
        mut retained: impl FnMut(NodeId) -> bool,
    ) -> Result<(), crate::Error> {
        let mut tables = self.tables();
        if tables
            .original
            .iter()
            .any(|(&key, &value)| retained(key) && !retained(value))
            || tables
                .auto_generate
                .iter()
                .any(|(&key, info)| retained(key) && info.node.is_some_and(|node| !retained(node)))
        {
            return Err(crate::Error::MissingNode(
                "retained emit metadata dependency",
            ));
        }
        tables.emit_flags.retain(|&key, _| retained(key));
        tables.original.retain(|&key, _| retained(key));
        tables.comment_ranges.retain(|&key, _| retained(key));
        tables.leading_comments.retain(|&key, _| retained(key));
        tables.auto_generate.retain(|&key, _| retained(key));
        Ok(())
    }

    /// Structural bytes of the side tables: table allocations and comment
    /// list capacities.
    pub fn structural_bytes(&self) -> usize {
        self.structural_bytes_with(&mut ts_arena::StorageCensus::default())
    }
    pub fn structural_bytes_with(&self, census: &mut ts_arena::StorageCensus) -> usize {
        let tables = self.tables();
        let header = census.allocation(
            Arc::as_ptr(&self.tables) as usize,
            16 + size_of::<Mutex<SideTables>>(),
        );
        if header == 0 {
            return 0;
        }
        let text_bytes = tables
            .leading_comments
            .values()
            .flatten()
            .map(|comment| census.text(comment.text.backing_bytes()))
            .sum::<usize>()
            + tables
                .auto_generate
                .values()
                .map(|info| {
                    census.text(info.prefix.backing_bytes())
                        + census.text(info.suffix.backing_bytes())
                })
                .sum::<usize>();
        header
            + text_bytes
            + tables.emit_flags.allocation_size()
            + tables.original.allocation_size()
            + tables.comment_ranges.allocation_size()
            + tables.leading_comments.allocation_size()
            + tables
                .leading_comments
                .values()
                .map(|comments| comments.capacity() * size_of::<SynthesizedComment>())
                .sum::<usize>()
            + tables.auto_generate.allocation_size()
    }

    pub fn metadata_entries(&self) -> usize {
        let tables = self.tables();
        tables.emit_flags.len()
            + tables.original.len()
            + tables.comment_ranges.len()
            + tables.leading_comments.len()
            + tables.auto_generate.len()
    }

    // port: tsc/internal/printer/emitcontext.go:EmitContext.EmitFlags
    pub fn emit_flags(&self, node: NodeId) -> EmitFlags {
        self.tables().emit_flags.get(&node).copied().unwrap_or(0)
    }
    // port: tsc/internal/printer/emitcontext.go:EmitContext.SetEmitFlags
    pub fn set_emit_flags(&mut self, node: NodeId, flags: EmitFlags) {
        self.tables().emit_flags.insert(node, flags);
    }
    // port: tsc/internal/printer/emitcontext.go:EmitContext.AddEmitFlags
    pub fn add_emit_flags(&mut self, node: NodeId, flags: EmitFlags) {
        *self.tables().emit_flags.entry(node).or_insert(0) |= flags;
    }
    // port: tsc/internal/printer/emitcontext.go:EmitContext.SetOriginal
    pub fn set_original(&mut self, node: NodeId, original: NodeId) {
        self.set_original_ex(node, original, false);
    }
    // port: tsc/internal/printer/emitcontext.go:EmitContext.SetOriginalEx
    pub fn set_original_ex(&mut self, node: NodeId, original: NodeId, allow_overwrite: bool) {
        let mut tables = self.tables();
        if allow_overwrite && tables.original.contains_key(&node) {
            // The pin copies emit metadata on the first assignment only.
            // Changing provenance later must preserve the clone's own flags.
            tables.original.insert(node, original);
        } else {
            tables.set_original(node, original);
        }
    }
    // port: tsc/internal/printer/emitcontext.go:EmitContext.Original
    pub fn original(&self, node: NodeId) -> Option<NodeId> {
        self.tables().original.get(&node).copied()
    }
    // port: tsc/internal/printer/emitcontext.go:EmitContext.MostOriginal
    pub fn most_original(&self, mut node: NodeId) -> NodeId {
        let tables = self.tables();
        while let Some(original) = tables.original.get(&node) {
            node = *original;
        }
        node
    }
    // port: tsc/internal/printer/emitcontext.go:EmitContext.HasAutoGenerateInfo
    pub fn has_auto_generate_info(&self, node: NodeId) -> bool {
        self.tables().auto_generate.contains_key(&node)
    }
    // port: tsc/internal/printer/emitcontext.go:EmitContext.GetAutoGenerateInfo
    pub fn auto_generate_info(&self, node: NodeId) -> Option<AutoGenerateInfo> {
        self.tables().auto_generate.get(&node).cloned()
    }

    // port: tsc/internal/printer/emitcontext.go:EmitContext.CommentRange
    pub fn comment_range(&self, node: NodeId) -> Option<ts_core::TextRange> {
        self.tables().comment_ranges.get(&node).copied()
    }
    pub fn comment_range_of(&self, factory: &dyn Factory, node: NodeId) -> ts_core::TextRange {
        self.comment_range(node)
            .unwrap_or_else(|| factory.node(node).range())
    }
    // port: tsc/internal/printer/emitcontext.go:EmitContext.SetCommentRange
    pub fn set_comment_range(&mut self, node: NodeId, range: ts_core::TextRange) {
        self.tables().comment_ranges.insert(node, range);
    }
    // port: tsc/internal/printer/emitcontext.go:EmitContext.AssignCommentRange
    pub fn assign_comment_range(&mut self, factory: &dyn Factory, to: NodeId, from: NodeId) {
        let range = self.comment_range_of(factory, from);
        self.set_comment_range(to, range);
    }

    // port: tsc/internal/printer/emitcontext.go:EmitContext.AddSyntheticLeadingComment
    pub fn add_synthetic_leading_comment(
        &mut self,
        node: NodeId,
        kind: ts_ast::SyntaxKind,
        text: JsString,
        has_trailing_new_line: bool,
    ) -> NodeId {
        self.tables()
            .leading_comments
            .entry(node)
            .or_default()
            .push(SynthesizedComment {
                kind,
                loc: ts_core::TextRange::new(-1, -1),
                has_leading_new_line: false,
                has_trailing_new_line,
                text,
            });
        node
    }
    // port: tsc/internal/printer/emitcontext.go:EmitContext.GetSyntheticLeadingComments
    pub fn synthetic_leading_comments(&self, node: NodeId) -> Vec<SynthesizedComment> {
        self.tables()
            .leading_comments
            .get(&node)
            .cloned()
            .unwrap_or_default()
    }

    // port: tsc/internal/printer/emitcontext.go:EmitContext.GetNodeForGeneratedName
    pub fn node_for_generated_name(&self, factory: &dyn Factory, name: NodeId) -> NodeId {
        if let Some(info) = self.auto_generate_info(name) {
            if info.flags & generated_identifier_flags::KIND_MASK
                == generated_identifier_flags::NODE
            {
                return self.generated_name_root(
                    factory,
                    info.node.expect("node-generated name has a source"),
                    info.id,
                );
            }
        }
        name
    }
    // port: tsc/internal/printer/emitcontext.go:EmitContext.getNodeForGeneratedNameWorker
    fn generated_name_root(
        &self,
        factory: &dyn Factory,
        mut node: NodeId,
        id: AutoGenerateId,
    ) -> NodeId {
        use generated_identifier_flags as g;
        let mut original = self.original(node);
        while let Some(next) = original {
            node = next;
            if matches!(
                factory.node(node).kind().known(),
                Some(ts_ast::SyntaxKind::Identifier | ts_ast::SyntaxKind::PrivateIdentifier)
            ) {
                let Some(info) = self.auto_generate_info(node) else {
                    break;
                };
                if info.flags & g::KIND_MASK == g::NODE {
                    if info.id != id {
                        break;
                    }
                    original = info.node;
                    continue;
                }
            }
            original = self.original(node);
        }
        node
    }
    // port: tsc/internal/printer/factory.go:NodeFactory.NewUniqueNameEx
    pub fn new_unique_name_ex(
        &mut self,
        factory: &mut dyn Factory,
        text: JsString,
        options: AutoGenerateOptions,
    ) -> NodeId {
        self.new_generated_identifier(
            factory,
            generated_identifier_flags::UNIQUE,
            text,
            None,
            options,
        )
    }
    // port: tsc/internal/printer/factory.go:NodeFactory.NewGeneratedNameForNodeEx
    pub fn new_generated_name_for_node_ex(
        &mut self,
        factory: &mut dyn Factory,
        node: NodeId,
        mut options: AutoGenerateOptions,
    ) -> NodeId {
        if !options.prefix.is_empty() || !options.suffix.is_empty() {
            options.flags |= generated_identifier_flags::OPTIMISTIC;
        }
        self.new_generated_identifier(
            factory,
            generated_identifier_flags::NODE,
            JsString::default(),
            Some(node),
            options,
        )
    }
    // port: tsc/internal/printer/factory.go:NodeFactory.newGeneratedIdentifier
    fn new_generated_identifier(
        &mut self,
        factory: &mut dyn Factory,
        kind: generated_identifier_flags::Flags,
        text: JsString,
        source: Option<NodeId>,
        options: AutoGenerateOptions,
    ) -> NodeId {
        use generated_identifier_flags as g;
        static NEXT_ID: AtomicU32 = AtomicU32::new(0);
        // Exhaustion must not alias a retained generated name. Unlike a source
        // integer, this counter is an internal identity and cannot wrap safely.
        let id = AutoGenerateId(
            NEXT_ID
                .try_update(Ordering::Relaxed, Ordering::Relaxed, |id| id.checked_add(1))
                .expect("generated-name identity space exhausted")
                + 1,
        );
        let text = if text.is_empty() {
            let base = match source {
                None => JsString::from_bytes(format!("(auto@{})", id.get()).into_bytes()),
                Some(node) => {
                    let read = factory.node(node);
                    if let Some(data) = read.as_identifier() {
                        data.text_owned()
                    } else if let Some(data) = read.as_private_identifier() {
                        data.text_owned()
                    } else {
                        let root = self.generated_name_root(factory, node, id);
                        JsString::from_bytes(
                            format!("(generated@{})", factory.node(root).runtime_id()).into_bytes(),
                        )
                    }
                }
            };
            let mut bytes = options
                .prefix
                .as_bytes()
                .strip_prefix(b"#")
                .unwrap_or(options.prefix.as_bytes())
                .to_vec();
            bytes.extend_from_slice(
                base.as_bytes()
                    .strip_prefix(b"#")
                    .unwrap_or(base.as_bytes()),
            );
            bytes.extend_from_slice(
                options
                    .suffix
                    .as_bytes()
                    .strip_prefix(b"#")
                    .unwrap_or(options.suffix.as_bytes()),
            );
            JsString::from_bytes(bytes)
        } else {
            text
        };
        let node = factory.new_identifier(text);
        self.tables().auto_generate.insert(
            node,
            AutoGenerateInfo {
                id,
                flags: kind | (options.flags & !g::KIND_MASK),
                prefix: options.prefix,
                suffix: options.suffix,
                node: source,
            },
        );
        node
    }
}

struct EmitHooks(Arc<Mutex<SideTables>>);
impl FactoryHooks for EmitHooks {
    // port: tsc/internal/printer/emitcontext.go:EmitContext.onCreate
    fn on_create(&self, factory: &mut dyn Factory, node: NodeId) {
        factory.add_node_flags(node, node_flags::SYNTHESIZED);
    }
    // port: tsc/internal/printer/emitcontext.go:EmitContext.onUpdate
    fn on_update(&self, _factory: &mut dyn Factory, node: NodeId, original: NodeId) {
        self.0
            .lock()
            .expect("emit context metadata poisoned")
            .set_original(node, original);
    }
    // port: tsc/internal/printer/emitcontext.go:EmitContext.onClone
    fn on_clone(&self, _factory: &mut dyn Factory, node: NodeId, original: NodeId) {
        let mut tables = self.0.lock().expect("emit context metadata poisoned");
        tables.set_original(node, original);
        // Only identifiers/private identifiers can have this entry. Copying by
        // presence preserves that invariant without another AST lookup.
        if let Some(info) = tables.auto_generate.get(&original).cloned() {
            tables.auto_generate.insert(node, info);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ts_ast::AstBuilder;
    #[test]
    fn census_charges_shared_comment_and_generated_name_text_once() {
        let counters = ts_arena::Counters::new();
        let mut ast = AstBuilder::new(
            ts_jsstring::SourceText::from_loaded_bytes(b"".as_slice()),
            &counters,
        );
        let mut emit = EmitContext::new();
        let text = JsString::from_bytes(b"shared comment and prefix".as_slice());
        let node = emit.new_unique_name_ex(
            &mut ast,
            JsString::from_bytes(b"name".as_slice()),
            AutoGenerateOptions {
                prefix: text.clone(),
                suffix: text.clone(),
                ..AutoGenerateOptions::default()
            },
        );
        emit.add_synthetic_leading_comment(
            node,
            ts_ast::SyntaxKind::SingleLineCommentTrivia,
            text.clone(),
            false,
        );
        let all = emit.structural_bytes();
        let mut census = ts_arena::StorageCensus::default();
        let text_bytes = census.text(text.backing_bytes());
        assert_eq!(emit.structural_bytes_with(&mut census), all - text_bytes);
        assert_eq!(emit.clone().structural_bytes_with(&mut census), 0);
    }

    #[test]
    fn generated_name_identity_survives_clone_and_update_hooks() {
        let counters = ts_arena::Counters::new();
        let mut emit = EmitContext::new();
        let mut ast = AstBuilder::with_hooks(
            ts_jsstring::SourceText::from_loaded_bytes(&b""[..]),
            &counters,
            emit.factory_hooks(),
        );
        let options = AutoGenerateOptions {
            flags: generated_identifier_flags::OPTIMISTIC,
            prefix: JsString::from_bytes(b"pre".as_slice()),
            suffix: JsString::from_bytes(b"post".as_slice()),
        };
        let name = emit.new_unique_name_ex(
            &mut ast,
            JsString::from_bytes(b"_default".as_slice()),
            options.clone(),
        );
        let different = emit.new_unique_name_ex(
            &mut ast,
            JsString::from_bytes(b"_default".as_slice()),
            options,
        );
        emit.set_emit_flags(name, crate::emit_flags::NO_COMMENTS);
        let cloned = ast.clone_identifier(name);
        let cloned_again = ast.clone_identifier(cloned);
        assert_eq!(
            emit.auto_generate_info(name),
            emit.auto_generate_info(cloned_again)
        );
        assert_ne!(
            emit.auto_generate_info(name).unwrap().id,
            emit.auto_generate_info(different).unwrap().id
        );
        assert_eq!(emit.most_original(cloned_again), name);
        assert_eq!(
            emit.emit_flags(cloned_again),
            crate::emit_flags::NO_COMMENTS
        );
        assert_eq!(ast.node(name).as_identifier().unwrap().text(), b"_default");
        assert_ne!(ast.node(cloned).flags() & node_flags::SYNTHESIZED, 0);
        let statement = ast.new_expression_statement(Some(name));
        emit.set_emit_flags(statement, crate::emit_flags::SINGLE_LINE);
        let updated = ast.update_expression_statement(statement, Some(different));
        assert_eq!(emit.original(updated), Some(statement));
        assert_eq!(emit.emit_flags(updated), crate::emit_flags::SINGLE_LINE);
        assert!(emit.auto_generate_info(updated).is_none());
    }
}

#[cfg(test)]
mod original_overwrite_tests {
    use super::*;
    use ts_ast::{AstBuilder, FactoryMethods};
    #[test]
    fn replacing_original_keeps_local_emit_metadata() {
        let mut emit = EmitContext::new();
        let mut ast = AstBuilder::with_hooks(
            ts_jsstring::SourceText::default(),
            &ts_arena::Counters::new(),
            emit.factory_hooks(),
        );
        let first = ast.new_identifier(JsString::from_bytes(b"a".as_slice()));
        let second = ast.new_identifier(JsString::from_bytes(b"b".as_slice()));
        let result = ast.new_identifier(JsString::from_bytes(b"c".as_slice()));
        emit.set_emit_flags(first, crate::emit_flags::SINGLE_LINE);
        emit.set_emit_flags(second, crate::emit_flags::NO_ASCII_ESCAPING);
        emit.set_original(result, first);
        assert_eq!(emit.emit_flags(result), crate::emit_flags::SINGLE_LINE);
        emit.set_emit_flags(result, crate::emit_flags::NO_COMMENTS);
        emit.set_original_ex(result, second, true);
        assert_eq!(emit.original(result), Some(second));
        assert_eq!(emit.emit_flags(result), crate::emit_flags::NO_COMMENTS);
    }
}

#[cfg(test)]
mod retention_tests {
    use super::*;
    use ts_ast::{AstBuilder, FactoryMethods};

    #[test]
    fn metadata_pruning_requires_original_and_generated_name_dependencies() {
        let mut emit = EmitContext::new();
        let shared = emit.clone();
        let mut ast = AstBuilder::with_hooks(
            ts_jsstring::SourceText::default(),
            &ts_arena::Counters::new(),
            emit.factory_hooks(),
        );
        let source = ast.new_identifier(JsString::from_bytes(b"source".as_slice()));
        let generated =
            emit.new_generated_name_for_node_ex(&mut ast, source, AutoGenerateOptions::default());
        let cloned = ast.clone_identifier(generated);
        let garbage = ast.new_identifier(JsString::from_bytes(b"garbage".as_slice()));
        emit.set_emit_flags(garbage, crate::emit_flags::NO_COMMENTS);
        emit.set_comment_range(cloned, ts_core::TextRange::new(1, 2));
        emit.add_synthetic_leading_comment(
            cloned,
            ts_ast::SyntaxKind::MultiLineCommentTrivia,
            JsString::from_bytes(b"keep".as_slice()),
            false,
        );
        // A live key requires the complete original chain AND the generated-name target.
        for live in [vec![cloned], vec![cloned, generated]] {
            assert!(emit.retain_metadata(|id| live.contains(&id)).is_err());
            assert_eq!(
                shared.emit_flags(garbage),
                crate::emit_flags::NO_COMMENTS,
                "validation failure must not partially prune the shared tables"
            );
        }
        emit.retain_metadata(|id| [source, generated, cloned].contains(&id))
            .unwrap();
        assert_eq!(shared.emit_flags(garbage), 0);
        assert_eq!(shared.most_original(cloned), generated);
        assert_eq!(shared.node_for_generated_name(&ast, cloned), source);
        assert_eq!(
            shared.comment_range(cloned),
            Some(ts_core::TextRange::new(1, 2))
        );
        assert_eq!(
            shared.synthetic_leading_comments(cloned)[0].text.as_bytes(),
            b"keep"
        );
        emit.retain_metadata(|_| false).unwrap();
        assert_eq!(shared.metadata_entries(), 0);
    }
}
