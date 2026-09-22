//! Token navigation over a parsed file: the token at a position, the token
//! before it, the token after another, and a child token of a given kind.
//!
//! A port of `tsc/internal/astnav/tokens.go`. Punctuation and keywords are not
//! nodes of the parsed tree, so the answers are either real nodes or tokens
//! built from the scanner and kept in the file's token cache
//! (`AstView::get_or_create_token`), which makes repeated questions return the
//! same node.
//!
//! Upstream drives these searches with a node visitor whose hooks cannot stop a
//! traversal, so its closures keep visiting after they have their answer and
//! ignore what follows. Here a node's children are first laid out as the same
//! sequence of visits ([`Navigator::visits`]) and each search is a loop over it
//! with the same guards, which keeps the order of side effects identical.
#![forbid(unsafe_code)]

use std::cmp::Ordering;
use std::ops::ControlFlow;
use tsr_arena::NodeId;
use tsr_ast::{
    node_flags, utilities, utilities_middle, AstView, ChildRole, ChildSlot, ChildVisitor,
    JsDocProvider, NodeKind, NodeListId, NodeRead, NodeSlice, SyntaxKind as K,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    Storage(tsr_arena::Error),
    /// An upstream `panic`: an invariant of the tree or the scanner did not
    /// hold. The message is upstream's, so a caller can report it unchanged.
    Assertion(String),
}

impl From<tsr_arena::Error> for Error {
    fn from(error: tsr_arena::Error) -> Self {
        Self::Storage(error)
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, output: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Storage(error) => write!(output, "{error:?}"),
            Self::Assertion(message) => output.write_str(message),
        }
    }
}

impl std::error::Error for Error {}

/// One hook call of upstream's visitor, for callers outside this crate.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChildVisit {
    Node(NodeId),
    List(Vec<NodeId>),
}

/// One hook call of upstream's visitor: a node (or token) child, or a list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Visit {
    Node(NodeId),
    List(NodeListId),
}

/// One observable call of the navigation visitor. A list remains an owner-
/// checked handle so consumers can read its range as well as its members.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HookVisit {
    Node(Option<NodeId>),
    List(Option<NodeListId>),
}

const LESS_THAN: Ordering = Ordering::Less;
const EQUAL_TO: Ordering = Ordering::Equal;
const GREATER_THAN: Ordering = Ordering::Greater;

/// `core.BinarySearchUniqueFunc`. The comparer has side effects in these
/// searches, so the probe order is part of the contract.
fn binary_search_unique(
    len: usize,
    mut compare: impl FnMut(usize) -> Result<Ordering, Error>,
) -> Result<(usize, bool), Error> {
    if len == 0 {
        return Ok((0, false));
    }
    let (mut low, mut high) = (0isize, len as isize - 1);
    while low <= high {
        let middle = low + ((high - low) >> 1);
        match compare(middle as usize)? {
            Ordering::Less => low = middle + 1,
            Ordering::Greater => high = middle - 1,
            Ordering::Equal => return Ok((middle as usize, true)),
        }
    }
    Ok((low as usize, false))
}

fn kind_name(kind: NodeKind) -> String {
    match kind.known() {
        Some(kind) => format!("Kind{kind:?}"),
        None => format!("Kind({})", kind.raw()),
    }
}

/// The hook calls of `node.VisitEachChild`, in order: a node, or a list that
/// upstream hands to `VisitNodes` or `VisitModifiers`.
pub fn visit_each_child(view: AstView<'_>, id: NodeId) -> Result<Vec<Visit>, Error> {
    let slots = view.node(id)?.child_slots();
    let mut visits = Vec::with_capacity(slots.len());
    for (_, slot) in slots {
        match slot {
            ChildSlot::Node(Some(child)) => visits.push(Visit::Node(child)),
            ChildSlot::List(Some(list)) => visits.push(Visit::List(list)),
            ChildSlot::Nodes(nodes) => {
                visits.extend(view.node_slice(nodes)?.iter().flatten().map(Visit::Node));
            }
            ChildSlot::Node(None) | ChildSlot::List(None) => {}
        }
    }
    Ok(visits)
}

pub struct Navigator<'a, 'p> {
    view: AstView<'a>,
    source: NodeId,
    jsdoc: &'p mut dyn JsDocProvider,
}

struct Collect<'v> {
    view: AstView<'v>,
    out: Vec<Visit>,
    error: Option<tsr_arena::Error>,
}

impl ChildVisitor for Collect<'_> {
    fn visit_node(&mut self, node: NodeId) -> ControlFlow<()> {
        self.out.push(Visit::Node(node));
        ControlFlow::Continue(())
    }
    fn visit_list(&mut self, nodes: NodeListId) -> ControlFlow<()> {
        self.out.push(Visit::List(nodes));
        ControlFlow::Continue(())
    }
    // Upstream visits a raw slice (`SyntaxList.Children`,
    // `JSDocTypeLiteral.JSDocPropertyTags`) one node at a time.
    fn visit_node_slice(&mut self, nodes: NodeSlice) -> ControlFlow<()> {
        match self.view.node_slice(nodes) {
            Ok(read) => {
                self.out.extend(read.iter().flatten().map(Visit::Node));
                ControlFlow::Continue(())
            }
            Err(error) => {
                self.error = Some(error);
                ControlFlow::Break(())
            }
        }
    }
}

impl<'a, 'p> Navigator<'a, 'p> {
    /// `source` is the file's `SourceFile` node.
    pub fn new(view: AstView<'a>, source: NodeId, jsdoc: &'p mut dyn JsDocProvider) -> Self {
        Self {
            view,
            source,
            jsdoc,
        }
    }

    fn node(&self, id: NodeId) -> Result<NodeRead<'a>, Error> {
        Ok(self.view.node(id)?)
    }

    fn reparsed(&self, id: NodeId) -> Result<bool, Error> {
        Ok(self.node(id)?.flags() & node_flags::REPARSED != 0)
    }

    fn pos(&self, id: NodeId) -> Result<i64, Error> {
        Ok(i64::from(self.node(id)?.pos()))
    }

    fn end(&self, id: NodeId) -> Result<i64, Error> {
        Ok(i64::from(self.node(id)?.end()))
    }

    fn list_nodes(&self, list: NodeListId) -> Result<Vec<NodeId>, Error> {
        let read = self.view.list(list)?;
        Ok(self
            .view
            .node_slice(read.nodes())?
            .iter()
            .flatten()
            .collect())
    }

    fn list_range(&self, list: NodeListId) -> Result<(i64, i64), Error> {
        let loc = self.view.list(list)?.loc();
        Ok((loc.pos(), loc.end()))
    }

    fn jsdoc_of(&mut self, id: NodeId) -> Result<Vec<NodeId>, Error> {
        Ok(self.jsdoc.jsdoc(self.view, self.source, id)?.to_vec())
    }

    fn end_of_file(&self) -> Result<i64, Error> {
        let file = self.node(self.source)?;
        let token = file
            .data_source()
            .as_source_file()
            .and_then(|data| data.end_of_file_token())
            .ok_or(tsr_arena::Error::InvalidGraph)?;
        self.end(token)
    }

    /// Read-only hook observations, retaining each list identity/range and
    /// absent child slot. Modifiers omit the nil-list call, as getNodeVisitor
    /// does; raw slices invoke the node hook once per element.
    // port: tsc/internal/astnav/tokens.go:VisitEachChildAndJSDoc
    // port: tsc/internal/astnav/tokens.go:getNodeVisitor
    pub fn visit_child_slots_and_jsdoc(&mut self, id: NodeId) -> Result<Vec<HookVisit>, Error> {
        let mut out = Vec::new();
        for doc in self.jsdoc_of(id)? {
            self.push_node_hook(&mut out, Some(doc))?;
        }
        let slots = self.node(id)?.child_slots();
        for (role, slot) in slots {
            match slot {
                ChildSlot::Node(node) => self.push_node_hook(&mut out, node)?,
                ChildSlot::List(list) => {
                    if (role != ChildRole::Modifiers || list.is_some())
                        && !utilities_middle::is_js_doc_single_comment_node_list(self.view, list)?
                    {
                        out.push(HookVisit::List(list));
                    }
                }
                ChildSlot::Nodes(nodes) => {
                    for node in self.view.node_slice(nodes)?.iter() {
                        self.push_node_hook(&mut out, node)?;
                    }
                }
            }
        }
        Ok(out)
    }

    fn push_node_hook(&self, out: &mut Vec<HookVisit>, node: Option<NodeId>) -> Result<(), Error> {
        if !utilities_middle::is_js_doc_single_comment_node_comment(self.view, node)? {
            out.push(HookVisit::Node(node));
        }
        Ok(())
    }

    fn visits(&mut self, id: NodeId) -> Result<Vec<Visit>, Error> {
        Ok(self
            .visit_child_slots_and_jsdoc(id)?
            .into_iter()
            .filter_map(|visit| match visit {
                HookVisit::Node(node) => node.map(Visit::Node),
                HookVisit::List(list) => list.map(Visit::List),
            })
            .collect())
    }

    /// Upstream's traversal as other packages use it: the hook calls for a
    /// node's JSDoc and children in order, with each list resolved to its nodes.
    pub fn visit_each_child_and_jsdoc(&mut self, id: NodeId) -> Result<Vec<ChildVisit>, Error> {
        self.visits(id)?
            .into_iter()
            .map(|visit| {
                Ok(match visit {
                    Visit::Node(node) => ChildVisit::Node(node),
                    Visit::List(list) => ChildVisit::List(self.list_nodes(list)?),
                })
            })
            .collect()
    }

    // port: tsc/internal/astnav/tokens.go:GetStartOfNode
    pub fn get_start_of_node(&mut self, node: NodeId, include_jsdoc: bool) -> Result<i64, Error> {
        Ok(tsr_scanner::get_token_pos_of_node(
            self.view,
            self.source,
            node,
            include_jsdoc,
            self.jsdoc,
        )?)
    }

    // port: tsc/internal/astnav/tokens.go:getPosition
    fn position_of(&mut self, node: NodeId, allow_leading_trivia: bool) -> Result<i64, Error> {
        if allow_leading_trivia {
            return self.pos(node);
        }
        self.get_start_of_node(node, true)
    }

    // port: tsc/internal/astnav/tokens.go:shouldSkipChild
    fn should_skip_child(&self, id: NodeId) -> Result<bool, Error> {
        let node = self.node(id)?;
        Ok(matches!(
            node.kind().known(),
            Some(K::JSDoc | K::JSDocText | K::JSDocTypeLiteral | K::JSDocSignature)
        ) || utilities_middle::is_js_doc_link_like(&node)
            || utilities_middle::is_js_doc_tag(&node))
    }

    fn token(
        &self,
        kind: K,
        full_start: i64,
        end: i64,
        parent: NodeId,
        flags: tsr_ast::TokenFlags,
    ) -> Result<NodeId, Error> {
        let (Ok(pos), Ok(end)) = (i32::try_from(full_start), i32::try_from(end)) else {
            return Err(tsr_arena::Error::InvalidTokenRange.into());
        };
        match self.view.get_or_create_token(kind, pos, end, parent, flags) {
            Ok(token) => Ok(token.id()),
            // Upstream's two token-cache panics.
            Err(tsr_arena::Error::TokenKindMismatch { cached, requested }) => {
                Err(Error::Assertion(format!(
                    "Token cache mismatch: {} != {}",
                    kind_name(NodeKind::from_raw(cached as i16)),
                    kind_name(NodeKind::from_raw(requested as i16))
                )))
            }
            Err(tsr_arena::Error::ReparsedParent) => Err(Error::Assertion(format!(
                "Cannot create token from reparsed node of kind {}",
                kind_name(self.node(parent)?.kind())
            ))),
            Err(error) => Err(error.into()),
        }
    }

    // port: tsc/internal/astnav/tokens.go:GetTouchingPropertyName
    pub fn get_touching_property_name(&mut self, position: i64) -> Result<NodeId, Error> {
        self.token_at(
            position,
            false,
            Some(&|node: &NodeRead<'_>| {
                utilities::is_property_name_literal(node)
                    || tsr_ast::is_keyword_kind(node.kind())
                    || tsr_ast::is_private_identifier(node)
            }),
        )
    }

    // port: tsc/internal/astnav/tokens.go:GetTouchingToken
    pub fn get_touching_token(&mut self, position: i64) -> Result<NodeId, Error> {
        self.token_at(position, false, None)
    }

    // port: tsc/internal/astnav/tokens.go:GetTokenAtPosition
    pub fn get_token_at_position(&mut self, position: i64) -> Result<NodeId, Error> {
        self.token_at(position, true, None)
    }

    /// Returns a token at the position: a real node of the tree, or a token
    /// built from the scanner's result. When leading trivia is not allowed and
    /// no token is there, the lowest node that encloses the position.
    // port: tsc/internal/astnav/tokens.go:getTokenAtPosition
    fn token_at(
        &mut self,
        position: i64,
        allow_leading_trivia: bool,
        include_preceding: Option<&dyn Fn(&NodeRead<'_>) -> bool>,
    ) -> Result<NodeId, Error> {
        let mut search = TokenAt {
            position,
            allow_leading_trivia,
            include_preceding,
            next: None,
            prev_subtree: None,
            left: 0,
            node_after_left: None,
        };
        let mut current = self.source;
        loop {
            for visit in self.visits(current)? {
                match visit {
                    Visit::Node(node) => search.visit_node(self, node)?,
                    Visit::List(list) => search.visit_list(self, list)?,
                }
            }
            // A subtree that ends at the position may hold the wanted token as
            // its rightmost one.
            if let Some(subtree) = search.prev_subtree.take() {
                if let Some(child) = search.included_preceding_token(self, subtree)? {
                    return Ok(child);
                }
            }
            let Some(next) = search.next.take() else {
                return self.scan_for_token(current, &search);
            };
            current = next;
            search.left = self.pos(current)?;
            search.node_after_left = None;
        }
    }

    /// No child contains the position: the answer is `current` itself, or a
    /// token between its children that only the scanner can produce.
    fn scan_for_token(&mut self, current: NodeId, search: &TokenAt<'_>) -> Result<NodeId, Error> {
        let current_kind = self.node(current)?.kind();
        if tsr_ast::is_token_kind(current_kind) || self.should_skip_child(current)? {
            return Ok(current);
        }
        let mut end = self.end(current)?;
        // Only scan up to the next node after the one ending at `left`; the
        // position can fall between two nodes without being in the trivia of
        // the second, as inside a JSDoc type literal.
        if let Some(after) = search.node_after_left {
            end = self.pos(after)?;
        }
        let jsx_child = utilities::is_jsx_child(&self.node(current)?);
        let view = self.view;
        let file = view.source_file(self.source)?;
        let mut scanner = tsr_scanner::get_scanner_for_source_file(&file, search.left);
        let mut left = search.left;
        while left < end {
            let token = scan_navigation_token(&mut scanner, jsx_child);
            let full_start = scanner.token_full_start();
            let start = if search.allow_leading_trivia {
                full_start
            } else {
                scanner.token_start()
            };
            let token_end = scanner.token_end();
            let flags = scanner.token_flags();
            if token_end > end {
                break;
            }
            if start <= search.position && search.position < token_end {
                if token == K::Identifier || !tsr_ast::is_token_kind(token.into()) {
                    if utilities::is_js_doc_kind(current_kind) {
                        return Ok(current);
                    }
                    return Err(Error::Assertion(format!(
                        "did not expect {} to have {} in its trivia",
                        kind_name(current_kind),
                        kind_name(token.into())
                    )));
                }
                return self.token(token, full_start, token_end, current, flags);
            }
            if let Some(include) = search.include_preceding {
                if token_end == search.position {
                    let previous = self.token(token, full_start, token_end, current, flags)?;
                    if include(&self.node(previous)?) {
                        return Ok(previous);
                    }
                }
            }
            left = token_end;
            scanner.scan();
        }
        Ok(current)
    }

    // port: tsc/internal/astnav/tokens.go:FindPrecedingToken
    pub fn find_preceding_token(&mut self, position: i64) -> Result<Option<NodeId>, Error> {
        self.find_preceding_token_ex(position, None, false)
    }

    /// Finds the leftmost token satisfying `position < token.end`. If that token
    /// is invalid, or the position is in its trivia, finds the rightmost valid
    /// token with `token.end <= position`.
    // port: tsc/internal/astnav/tokens.go:FindPrecedingTokenEx
    pub fn find_preceding_token_ex(
        &mut self,
        position: i64,
        start_node: Option<NodeId>,
        exclude_jsdoc: bool,
    ) -> Result<Option<NodeId>, Error> {
        let start = start_node.unwrap_or(self.source);
        let result = self.find_preceding(start, position, exclude_jsdoc)?;
        if let Some(token) = result {
            if utilities_middle::is_whitespace_only_jsx_text(&self.node(token)?) {
                return Err(Error::Assertion(
                    "Expected result to be a non-whitespace token.".into(),
                ));
            }
        }
        Ok(result)
    }

    fn find_preceding(
        &mut self,
        mut n: NodeId,
        position: i64,
        exclude_jsdoc: bool,
    ) -> Result<Option<NodeId>, Error> {
        // Upstream tail-recurses into one child; retain its visitation and
        // scanner order without consuming one native frame per ancestor.
        loop {
            {
                let node = self.node(n)?;
                if utilities_middle::is_non_whitespace_token(&node) && node.kind() != K::EndOfFile {
                    return Ok(Some(n));
                }
            }
            // `found` is the leftmost child that contains the position; `previous`
            // is the last child visited before it.
            let mut found: Option<NodeId> = None;
            let mut previous: Option<NodeId> = None;
            for visit in self.visits(n)? {
                match visit {
                    Visit::Node(node) => {
                        if self.reparsed(node)? || found.is_some() {
                            continue;
                        }
                        let previous_end = match previous {
                            Some(previous) => Some(self.end(previous)?),
                            None => None,
                        };
                        if position < self.end(node)?
                            && previous_end.is_none_or(|end| end <= position)
                        {
                            found = Some(node);
                        } else {
                            previous = Some(node);
                        }
                    }
                    Visit::List(list) => {
                        if found.is_some() {
                            continue;
                        }
                        let nodes = self.list_nodes(list)?;
                        if nodes.is_empty() {
                            continue;
                        }
                        let (index, matched) = binary_search_unique(nodes.len(), |middle| {
                            // A reparsed JSDoc node ends before the node it is for.
                            if self.reparsed(nodes[middle])? {
                                return Ok(LESS_THAN);
                            }
                            if position < self.end(nodes[middle])? {
                                if middle == 0 || position >= self.end(nodes[middle - 1])? {
                                    return Ok(EQUAL_TO);
                                }
                                return Ok(GREATER_THAN);
                            }
                            Ok(LESS_THAN)
                        })?;
                        if matched {
                            found = Some(nodes[index]);
                        }
                        let lookup = if matched {
                            index as isize - 1
                        } else {
                            nodes.len() as isize - 1
                        };
                        let mut at = lookup;
                        while at >= 0 {
                            if !self.reparsed(nodes[at as usize])? && previous.is_none() {
                                previous = Some(nodes[at as usize]);
                            }
                            at -= 1;
                        }
                    }
                }
            }

            if let Some(found) = found {
                // A node's tokens span [start of node, node.end). Either the
                // position precedes the child's tokens, so the answer is in a
                // previous child or in the tokens between, or it is inside them.
                let start = self.get_start_of_node(found, !exclude_jsdoc)?;
                let look_in_previous = start >= position || !self.is_valid_preceding_node(found)?;
                if !look_in_previous {
                    n = found;
                    continue;
                }
                let found_pos = self.pos(found)?;
                if position >= found_pos {
                    // JSDoc that precedes the found child.
                    let mut doc = None;
                    for &candidate in self.jsdoc_of(n)?.iter().rev() {
                        if self.pos(candidate)? >= found_pos {
                            doc = Some(candidate);
                            break;
                        }
                    }
                    if let Some(doc) = doc {
                        let doc_end = self.end(doc)?;
                        if !exclude_jsdoc && position < doc_end {
                            n = doc;
                            continue;
                        }
                        return self.find_rightmost_valid_token(
                            doc_end,
                            n,
                            position,
                            exclude_jsdoc,
                        );
                    }
                    return self.find_rightmost_valid_token(found_pos, n, -1, exclude_jsdoc);
                }
                // The answer is in the tokens between two visited children.
                return self.find_rightmost_valid_token(found_pos, n, position, exclude_jsdoc);
            }

            // Either the position is at the end of the file, or the wanted token is
            // among the trailing tokens of this node that no child covers.
            let end = self.end(n)?;
            return if position >= end {
                self.find_rightmost_valid_token(end, n, -1, exclude_jsdoc)
            } else {
                self.find_rightmost_valid_token(end, n, position, exclude_jsdoc)
            };
        }
    }

    // port: tsc/internal/astnav/tokens.go:isValidPrecedingNode
    fn is_valid_preceding_node(&mut self, id: NodeId) -> Result<bool, Error> {
        if self.node(id)?.kind() == K::EndOfFile {
            return Ok(!self.jsdoc_of(id)?.is_empty());
        }
        let start = self.get_start_of_node(id, false)?;
        let width = self.end(id)? - start;
        Ok(!(utilities_middle::is_whitespace_only_jsx_text(&self.node(id)?) || width == 0))
    }

    /// Looks for the rightmost valid token in `[start, end_pos)`. With a
    /// position of at least zero, the rightmost one that precedes or touches it.
    // port: tsc/internal/astnav/tokens.go:findRightmostValidToken
    fn find_rightmost_valid_token(
        &mut self,
        end_pos: i64,
        containing: NodeId,
        position: i64,
        exclude_jsdoc: bool,
    ) -> Result<Option<NodeId>, Error> {
        let position = if position == -1 {
            self.end(containing)?
        } else {
            position
        };
        self.rightmost(
            Some(containing),
            end_pos,
            containing,
            position,
            exclude_jsdoc,
        )
    }

    fn should_visit(
        &mut self,
        node: NodeId,
        end_pos: i64,
        position: i64,
        exclude_jsdoc: bool,
    ) -> Result<bool, Error> {
        // A reparsed node, or one outside the wanted range, is not visited.
        Ok(!(self.reparsed(node)?
            || self.end(node)? > end_pos
            || self.get_start_of_node(node, !exclude_jsdoc)? >= position))
    }

    fn rightmost(
        &mut self,
        mut current: Option<NodeId>,
        mut end_pos: i64,
        containing: NodeId,
        position: i64,
        exclude_jsdoc: bool,
    ) -> Result<Option<NodeId>, Error> {
        loop {
            let Some(n) = current else {
                return Ok(None);
            };
            if utilities_middle::is_non_whitespace_token(&self.node(n)?) {
                return Ok(Some(n));
            }
            let mut rightmost_valid: Option<NodeId> = None;
            // Nodes after the last valid node.
            let mut rightmost_visited: Vec<NodeId> = Vec::new();
            let mut has_children = false;
            for visit in self.visits(n)? {
                match visit {
                    Visit::Node(node) => {
                        if self.reparsed(node)? {
                            continue;
                        }
                        has_children = true;
                        if !self.should_visit(node, end_pos, position, exclude_jsdoc)? {
                            continue;
                        }
                        rightmost_visited.push(node);
                        if self.is_valid_preceding_node(node)? {
                            rightmost_valid = Some(node);
                            rightmost_visited.clear();
                        }
                    }
                    Visit::List(list) => {
                        let nodes = self.list_nodes(list)?;
                        if nodes.is_empty() {
                            continue;
                        }
                        has_children = true;
                        let (index, _) = binary_search_unique(nodes.len(), |middle| {
                            Ok(if self.end(nodes[middle])? > end_pos {
                                GREATER_THAN
                            } else {
                                LESS_THAN
                            })
                        })?;
                        let mut valid_index: isize = -1;
                        let mut at = index as isize - 1;
                        while at >= 0 {
                            let candidate = nodes[at as usize];
                            if self.should_visit(candidate, end_pos, position, exclude_jsdoc)?
                                && self.is_valid_preceding_node(candidate)?
                            {
                                valid_index = at;
                                rightmost_valid = Some(candidate);
                                break;
                            }
                            at -= 1;
                        }
                        for &candidate in &nodes[(valid_index + 1) as usize..index] {
                            if self.should_visit(candidate, end_pos, position, exclude_jsdoc)? {
                                rightmost_visited.push(candidate);
                            }
                        }
                    }
                }
            }

            // The answer is a token of the rightmost valid node, or one of the
            // tokens after it that no child covers, or this childless node itself.
            // JSDoc nodes do not have trivia tokens as children.
            if !self.should_skip_child(n)? {
                let mut start_pos = match rightmost_valid {
                    Some(valid) => self.end(valid)?,
                    None => self.pos(n)?,
                };
                let jsx_child = utilities::is_jsx_child(&self.node(n)?);
                let view = self.view;
                let file = view.source_file(self.source)?;
                let mut scanner = tsr_scanner::get_scanner_for_source_file(&file, start_pos);
                let mut tokens: Vec<NodeId> = Vec::new();
                for &visited in &rightmost_visited {
                    // Trailing tokens that occur before this node.
                    let limit = self.pos(visited)?.min(position);
                    while start_pos < limit {
                        let token = scan_navigation_token(&mut scanner, jsx_child);
                        if scanner.token_start() >= limit {
                            break;
                        }
                        let (full_start, token_end) =
                            (scanner.token_full_start(), scanner.token_end());
                        start_pos = token_end;
                        let flags = scanner.token_flags();
                        tokens.push(self.token(token, full_start, token_end, n, flags)?);
                        scanner.scan();
                    }
                    start_pos = self.end(visited)?;
                    scanner.reset_pos(start_pos);
                    scanner.scan();
                }
                // Trailing tokens after the last visited node.
                let limit = end_pos.min(position);
                while start_pos < limit {
                    let token = scan_navigation_token(&mut scanner, jsx_child);
                    if scanner.token_start() >= limit {
                        break;
                    }
                    let (full_start, token_end) = (scanner.token_full_start(), scanner.token_end());
                    start_pos = token_end;
                    let flags = scanner.token_flags();
                    tokens.push(self.token(token, full_start, token_end, n, flags)?);
                    scanner.scan();
                }
                for &token in tokens.iter().rev() {
                    if !utilities_middle::is_whitespace_only_jsx_text(&self.node(token)?) {
                        return Ok(Some(token));
                    }
                }
            }

            if !has_children {
                return Ok((n != containing).then_some(n));
            }
            if let Some(valid) = rightmost_valid {
                end_pos = self.end(valid)?;
            }
            current = rightmost_valid;
        }
    }

    // port: tsc/internal/astnav/tokens.go:FindNextToken
    pub fn find_next_token(
        &mut self,
        previous_token: NodeId,
        parent: NodeId,
    ) -> Result<Option<NodeId>, Error> {
        let (previous_pos, previous_end) = (self.pos(previous_token)?, self.end(previous_token)?);
        let mut n = parent;
        loop {
            {
                let node = self.node(n)?;
                if tsr_ast::is_token_kind(node.kind()) && i64::from(node.pos()) == previous_end {
                    // A token that starts where the previous one ends.
                    return Ok(Some(n));
                }
            }
            // The node that contains the previous token or follows it directly.
            let mut found: Option<NodeId> = None;
            for visit in self.visits(n)? {
                match visit {
                    Visit::Node(node) => {
                        if !self.reparsed(node)?
                            && self.pos(node)? <= previous_end
                            && self.end(node)? > previous_end
                        {
                            found = Some(node);
                        }
                    }
                    Visit::List(list) => {
                        if found.is_some() {
                            continue;
                        }
                        let nodes = self.list_nodes(list)?;
                        let (index, matched) = binary_search_unique(nodes.len(), |middle| {
                            let node = nodes[middle];
                            if self.reparsed(node)? {
                                return Ok(LESS_THAN);
                            }
                            if self.pos(node)? > previous_end {
                                return Ok(GREATER_THAN);
                            }
                            if self.end(node)? <= previous_pos {
                                return Ok(LESS_THAN);
                            }
                            Ok(EQUAL_TO)
                        })?;
                        if matched {
                            found = Some(nodes[index]);
                        }
                    }
                }
            }
            if let Some(found) = found {
                n = found;
                continue;
            }
            // The next token is not a node; ask the scanner for it.
            if previous_end >= self.pos(n)? && previous_end < self.end(n)? {
                let view = self.view;
                let file = view.source_file(self.source)?;
                let scanner = tsr_scanner::get_scanner_for_source_file(&file, previous_end);
                let (token, full_start) = (scanner.token(), scanner.token_full_start());
                // Compared on the full start, which includes leading trivia, as
                // a node's position does.
                if full_start == previous_end {
                    let (token_end, flags) = (scanner.token_end(), scanner.token_flags());
                    return self.token(token, full_start, token_end, n, flags).map(Some);
                }
                return Err(Error::Assertion(format!(
                    "Expected to find next token at {previous_end}, got token {} at {full_start}",
                    kind_name(token.into())
                )));
            }
            return Ok(None);
        }
    }

    /// Searches the children of a node, and the tokens between them, for the
    /// first one of a kind.
    // port: tsc/internal/astnav/tokens.go:FindChildOfKind
    pub fn find_child_of_kind(
        &mut self,
        containing: NodeId,
        kind: K,
    ) -> Result<Option<NodeId>, Error> {
        let mut last_node_pos = self.pos(containing)?;
        // `ForEachChildAndJSDoc`: JSDoc first, then the children, with a list
        // visited one node at a time and the single-comment filter not applied.
        let mut children: Vec<NodeId> = self.jsdoc_of(containing)?;
        let mut collect = Collect {
            view: self.view,
            out: Vec::new(),
            error: None,
        };
        let _ = self.node(containing)?.for_each_child(&mut collect);
        if let Some(error) = collect.error {
            return Err(error.into());
        }
        for visit in collect.out {
            match visit {
                Visit::Node(node) => children.push(node),
                Visit::List(list) => children.extend(self.list_nodes(list)?),
            }
        }
        let view = self.view;
        let file = view.source_file(self.source)?;
        let mut scanner = tsr_scanner::get_scanner_for_source_file(&file, last_node_pos);
        for node in children {
            if self.reparsed(node)? {
                continue;
            }
            // The tokens that precede this child.
            let mut start_pos = last_node_pos;
            let node_pos = self.pos(node)?;
            while start_pos < node_pos {
                let (token, token_end) = (scanner.token(), scanner.token_end());
                if token == kind {
                    let (full_start, flags) = (scanner.token_full_start(), scanner.token_flags());
                    return self
                        .token(token, full_start, token_end, containing, flags)
                        .map(Some);
                }
                start_pos = token_end;
                scanner.scan();
            }
            if self.node(node)?.kind() == kind {
                return Ok(Some(node));
            }
            last_node_pos = self.end(node)?;
            scanner.reset_pos(last_node_pos);
        }
        // The tokens after the last child.
        let mut start_pos = last_node_pos;
        let end = self.end(containing)?;
        while start_pos < end {
            let (token, token_end) = (scanner.token(), scanner.token_end());
            if token == kind {
                let (full_start, flags) = (scanner.token_full_start(), scanner.token_flags());
                return self
                    .token(token, full_start, token_end, containing, flags)
                    .map(Some);
            }
            start_pos = token_end;
            scanner.scan();
        }
        Ok(None)
    }
}

// port: tsc/internal/astnav/tokens.go:scanNavigationToken
// port: tsc/internal/astnav/tokens.go:shouldRescanLessThanLessThanToken
fn scan_navigation_token(scanner: &mut tsr_scanner::Scanner<'_>, jsx_child: bool) -> K {
    let token = scanner.token();
    if token == K::LessThanLessThanToken && jsx_child {
        return scanner.rescan_jsx_token(true);
    }
    token
}

/// The state upstream's `getTokenAtPosition` closures share.
struct TokenAt<'f> {
    position: i64,
    allow_leading_trivia: bool,
    include_preceding: Option<&'f dyn Fn(&NodeRead<'_>) -> bool>,
    /// The child whose children are visited next.
    next: Option<NodeId>,
    /// A node that ends exactly at the position, when preceding tokens count.
    prev_subtree: Option<NodeId>,
    /// The lower bound of what can be returned; the scanner starts here.
    left: i64,
    /// The first node visited after the one that advanced `left`.
    node_after_left: Option<NodeId>,
}

impl TokenAt<'_> {
    fn included_preceding_token(
        &self,
        navigator: &mut Navigator<'_, '_>,
        subtree: NodeId,
    ) -> Result<Option<NodeId>, Error> {
        let Some(include) = self.include_preceding else {
            return Ok(None);
        };
        let child = navigator.find_preceding_token_ex(self.position, Some(subtree), false)?;
        if let Some(child) = child {
            let node = navigator.node(child)?;
            if i64::from(node.end()) == self.position && include(&node) {
                return Ok(Some(child));
            }
        }
        Ok(None)
    }

    fn test_node(
        &mut self,
        navigator: &mut Navigator<'_, '_>,
        id: NodeId,
    ) -> Result<Ordering, Error> {
        let (kind, end, flags) = {
            let node = navigator.node(id)?;
            (node.kind(), i64::from(node.end()), node.flags())
        };
        if kind != K::EndOfFile
            && end == self.position
            && self.include_preceding.is_some()
            && flags & node_flags::REPARSED == 0
        {
            if let Some(subtree) = self.prev_subtree {
                if self.included_preceding_token(navigator, subtree)?.is_some() {
                    return Ok(EQUAL_TO);
                }
            }
            self.prev_subtree = Some(id);
        }
        // A node contains the position when `position < end`. At the end of the
        // file the end is inclusive, for the end-of-file token and for JSDoc
        // that reaches it, such as an unterminated comment.
        if end < self.position
            || end == self.position
                && kind != K::EndOfFile
                && (!utilities::is_js_doc_kind(kind) || end != navigator.end_of_file()?)
        {
            return Ok(LESS_THAN);
        }
        if navigator.position_of(id, self.allow_leading_trivia)? > self.position {
            return Ok(GREATER_THAN);
        }
        Ok(EQUAL_TO)
    }

    fn visit_node(&mut self, navigator: &mut Navigator<'_, '_>, id: NodeId) -> Result<(), Error> {
        if navigator.reparsed(id)? {
            return Ok(());
        }
        if self.node_after_left.is_none() {
            self.node_after_left = Some(id);
        }
        if self.next.is_some() {
            return Ok(());
        }
        match self.test_node(navigator, id)? {
            LESS_THAN => {
                // The left boundary cannot move into or past JSDoc: the token
                // after it may be built by the scanner, and its position has to
                // include all its leading trivia.
                if !utilities::is_js_doc_kind(navigator.node(id)?.kind()) {
                    self.left = navigator.end(id)?;
                }
                self.node_after_left = None;
            }
            EQUAL_TO => self.next = Some(id),
            GREATER_THAN => {}
        }
        Ok(())
    }

    fn visit_list(
        &mut self,
        navigator: &mut Navigator<'_, '_>,
        list: NodeListId,
    ) -> Result<(), Error> {
        let nodes = navigator.list_nodes(list)?;
        if nodes.is_empty() {
            return Ok(());
        }
        if self.node_after_left.is_none() {
            for &node in &nodes {
                if !navigator.reparsed(node)? {
                    self.node_after_left = Some(node);
                    break;
                }
            }
        }
        if self.next.is_some() {
            return Ok(());
        }
        let (list_pos, list_end) = navigator.list_range(list)?;
        if list_end == self.position && self.include_preceding.is_some() {
            self.left = list_end;
            self.node_after_left = None;
            for &node in nodes.iter().rev() {
                if !navigator.reparsed(node)? {
                    self.prev_subtree = Some(node);
                    break;
                }
            }
        } else if list_end <= self.position {
            self.left = list_end;
            self.node_after_left = None;
        } else if list_pos <= self.position {
            let (mut index, mut matched) = binary_search_unique(nodes.len(), |middle| {
                let node = nodes[middle];
                if navigator.reparsed(node)? {
                    return Ok(EQUAL_TO);
                }
                let compared = self.test_node(navigator, node)?;
                if compared == LESS_THAN {
                    self.left = navigator.end(node)?;
                    self.node_after_left = None;
                    for &after in &nodes[middle + 1..] {
                        if !navigator.reparsed(after)? {
                            self.node_after_left = Some(after);
                            break;
                        }
                    }
                }
                Ok(compared)
            })?;
            let mut searched = nodes;
            if matched && navigator.reparsed(searched[index])? {
                // Filter the reparsed nodes out and search again.
                let mut kept = Vec::with_capacity(searched.len());
                for node in searched {
                    if !navigator.reparsed(node)? {
                        kept.push(node);
                    }
                }
                searched = kept;
                (index, matched) = binary_search_unique(searched.len(), |middle| {
                    let compared = self.test_node(navigator, searched[middle])?;
                    if compared == LESS_THAN {
                        self.left = navigator.end(searched[middle])?;
                        self.node_after_left = searched.get(middle + 1).copied();
                    }
                    Ok(compared)
                })?;
            }
            if matched {
                self.next = Some(searched[index]);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
