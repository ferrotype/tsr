//! Argument and contextual-parameter discovery. A cursor can be on a cached
//! punctuation token, a parsed argument, or an incomplete type argument list.
use crate::{documentation::list, syntax::Syntax, Result};
use tsr_ast::{utilities as ast, utilities_middle as mid, NodeId, NodeListId, SyntaxKind as K};
use tsr_checker::{Operation, SignatureRef, SymbolRef};
use tsr_core::TextRange;

#[derive(Clone, Copy)]
pub(crate) enum Invocation {
    Call(NodeId),
    TypeArguments(NodeId),
    Contextual {
        node: NodeId,
        signature: SignatureRef,
        symbol: Option<SymbolRef>,
    },
}
impl Invocation {
    pub fn enclosing(self) -> NodeId {
        match self {
            Self::Call(n) | Self::TypeArguments(n) | Self::Contextual { node: n, .. } => n,
        }
    }
    pub fn expression(self, syntax: &Syntax<'_>) -> Result<Option<NodeId>> {
        Ok(match self {
            Self::Call(n) => mid::get_invoked_expression(syntax.view, n)?,
            Self::TypeArguments(n) => Some(n),
            Self::Contextual { .. } => None,
        })
    }
}
#[derive(Clone, Copy)]
pub(crate) struct ArgumentInfo {
    pub invocation: Invocation,
    pub type_arguments: bool,
    pub span: TextRange,
    pub index: usize,
    pub count: usize,
}
struct ListInfo {
    list: Option<NodeListId>,
    index: usize,
    count: usize,
    span: TextRange,
}
impl Syntax<'_> {
    // port: tsc/internal/ls/utilities.go:IsInString
    pub(crate) fn in_string(&mut self, position: i64, previous: NodeId) -> Result<bool> {
        let n = self.view.node(previous)?;
        Ok(mid::is_string_text_containing_node(&n)
            && (self.start(previous)? < position && position < i64::from(n.end())
                || position == i64::from(n.end()) && mid::is_unterminated_literal(&n)))
    }
    // port: tsc/internal/ls/utilities.go:isInsideTemplateLiteral
    fn inside_template(&mut self, node: NodeId, position: i64) -> Result<bool> {
        let n = self.view.node(node)?;
        Ok(mid::is_template_literal_kind(n.kind())
            && (self.start(node)? < position && position < i64::from(n.end())
                || position == i64::from(n.end()) && mid::is_unterminated_literal(&n)))
    }
    // port: tsc/internal/ls/signaturehelp.go:getContainingArgumentInfo
    pub(crate) fn containing_argument(
        &mut self,
        token: NodeId,
        position: i64,
        manual: bool,
        checker: &mut Operation<'_>,
    ) -> Result<Option<ArgumentInfo>> {
        let mut current = Some(token);
        let mut first = None;
        while let Some(node) = current {
            let n = self.view.node(node)?;
            if n.kind() == K::SourceFile || !manual && n.kind() == K::Block {
                break;
            }
            current = n.parent();
            let info = match self.contextual_parameter(node, checker)? {
                Some(info) => Some(info),
                None => self.immediate_argument(node, position, checker)?,
            };
            if let Some(info) = info {
                if matches!(info.invocation, Invocation::Contextual { .. })
                    || info.span.end() == position
                    || info.span.contains(position)
                {
                    return Ok(Some(info));
                }
                if first.is_none() {
                    first = Some(info);
                }
            }
        }
        Ok(first)
    }
    // port: tsc/internal/ls/signaturehelp.go:getImmediatelyContainingArgumentInfo
    pub(crate) fn immediate_argument(
        &mut self,
        node: NodeId,
        position: i64,
        c: &mut Operation<'_>,
    ) -> Result<Option<ArgumentInfo>> {
        let n = self.view.node(node)?;
        let Some(parent) = n.parent() else {
            return Ok(None);
        };
        let p = self.view.node(parent)?;
        if mid::is_call_or_new_expression(&p) {
            return Ok(self.argument_list(node, c)?.map(
                |ListInfo {
                     list: lst,
                     index,
                     count,
                     span,
                 }| ArgumentInfo {
                    invocation: Invocation::Call(parent),
                    type_arguments: lst.is_some_and(|lst| {
                        p.type_argument_list().is_some_and(|t| {
                            self.view.list(t).is_ok_and(|t| {
                                self.view
                                    .list(lst)
                                    .is_ok_and(|l| l.loc().pos() == t.loc().pos())
                            })
                        })
                    }),
                    span,
                    index,
                    count,
                },
            ));
        }
        if n.kind() == K::NoSubstitutionTemplateLiteral && p.kind() == K::TaggedTemplateExpression {
            return if self.inside_template(node, position)? {
                self.template_info(parent, 0).map(Some)
            } else {
                Ok(None)
            };
        }
        if n.kind() == K::TemplateHead {
            if let Some(tag) = p.parent().filter(|t| {
                self.view
                    .node(*t)
                    .is_ok_and(|n| n.kind() == K::TaggedTemplateExpression)
            }) {
                let index = usize::from(!self.inside_template(node, position)?);
                return self.template_info(tag, index).map(Some);
            }
        }
        if p.kind() == K::TemplateSpan {
            if let Some(expr) = p.parent() {
                if let Some(tag) = self.view.node(expr)?.parent().filter(|t| {
                    self.view
                        .node(*t)
                        .is_ok_and(|n| n.kind() == K::TaggedTemplateExpression)
                }) {
                    if n.kind() == K::TemplateTail && !self.inside_template(node, position)? {
                        return Ok(None);
                    }
                    let spans = list(
                        self.view,
                        self.view
                            .node(expr)?
                            .data_source()
                            .as_template_expression()
                            .and_then(|d| d.template_spans()),
                    )?;
                    let index = spans
                        .iter()
                        .position(|s| *s == parent)
                        .ok_or(tsr_arena::Error::InvalidGraph)?;
                    let argument = if mid::is_template_literal_token(&n) {
                        if self.inside_template(node, position)? {
                            0
                        } else {
                            index + 2
                        }
                    } else {
                        index + 1
                    };
                    return self.template_info(tag, argument).map(Some);
                }
            }
        }
        if mid::is_jsx_opening_like_element(&p) {
            if let Some(attributes) = p.attributes() {
                let a = self.view.node(attributes)?;
                let start = i64::from(a.pos());
                let end = tsr_scanner::skip_trivia(self.file.text().as_bytes(), i64::from(a.end()));
                return Ok(Some(ArgumentInfo {
                    invocation: Invocation::Call(parent),
                    type_arguments: false,
                    // Preserve the pin's end-minus-start TextRange construction.
                    span: TextRange::new(start, end - start),
                    index: 0,
                    count: 1,
                }));
            }
        }
        Ok(self
            .possible_type_arguments(node)?
            .map(|(called, index)| ArgumentInfo {
                invocation: Invocation::TypeArguments(called),
                type_arguments: true,
                span: TextRange::new(
                    i64::from(self.view.node(called).expect("validated token").pos()),
                    i64::from(n.end()),
                ),
                index,
                count: index + 1,
            }))
    }
    // port: tsc/internal/ls/signaturehelp.go:getArgumentListInfoForTemplate
    fn template_info(&mut self, tag: NodeId, index: usize) -> Result<ArgumentInfo> {
        let template = self
            .view
            .node(tag)?
            .data_source()
            .as_tagged_template_expression()
            .and_then(|d| d.template())
            .ok_or(tsr_arena::Error::InvalidGraph)?;
        let n = self.view.node(template)?;
        let start = self.start(template)?;
        let mut end = i64::from(n.end());
        let mut count = 1;
        if let Some(expr) = n.data_source().as_template_expression() {
            let spans = list(self.view, expr.template_spans())?;
            count += spans.len();
            if let Some(last) = spans.last() {
                if let Some(lit) = self
                    .view
                    .node(*last)?
                    .data_source()
                    .as_template_span()
                    .and_then(|d| d.literal())
                {
                    let lit = self.view.node(lit)?;
                    if lit.pos() == lit.end() {
                        end = tsr_scanner::skip_trivia(self.file.text().as_bytes(), end);
                    }
                }
            }
        }
        Ok(ArgumentInfo {
            invocation: Invocation::Call(tag),
            type_arguments: false,
            span: TextRange::new(start, end - start),
            index,
            count,
        })
    }
    // port: tsc/internal/ls/utilities.go:findContainingList
    fn containing_list(&self, node: NodeId) -> Result<Option<NodeListId>> {
        let n = self.view.node(node)?;
        let Some(parent) = n.parent() else {
            return Ok(None);
        };
        let mut result = None;
        for visit in tsr_astnav::visit_each_child(self.view, parent)? {
            if let tsr_astnav::Visit::List(list) = visit {
                let range = self.view.list(list)?.loc();
                if range.pos() <= i64::from(n.pos()) && i64::from(n.end()) <= range.end() {
                    result = Some(list);
                }
            }
        }
        Ok(result)
    }
    // port: tsc/internal/ls/signaturehelp.go:getArgumentOrParameterListInfo
    fn argument_list(&self, node: NodeId, c: &mut Operation<'_>) -> Result<Option<ListInfo>> {
        let n = self.view.node(node)?;
        let Some(parent) = n.parent() else {
            return Ok(None);
        };
        let p = self.view.node(parent)?;
        let opener = matches!(n.kind().known(), Some(K::LessThanToken | K::OpenParenToken));
        let list = if opener {
            if mid::is_call_or_new_expression(&p) {
                if n.kind() == K::LessThanToken {
                    p.type_argument_list()
                } else {
                    p.argument_list()
                }
            } else {
                None
            }
        } else {
            let Some(list) = self.containing_list(node)? else {
                return Ok(None);
            };
            Some(list)
        };
        let tokens = self.list_tokens(list, parent)?;
        let index = if opener {
            0
        } else {
            argument_index(self, &tokens, Some(node), c)?
        };
        let count = argument_index(self, &tokens, None, c)?;
        let range = list
            .map(|id| self.view.list(id).map(|l| l.loc()))
            .transpose()?;
        let start = range.map_or(i64::from(n.end()), TextRange::pos);
        let end = tsr_scanner::skip_trivia(
            self.file.text().as_bytes(),
            range.map_or(i64::from(n.end()), TextRange::end),
        )
        .max(start + 1);
        Ok(Some(ListInfo {
            list,
            index,
            count,
            span: TextRange::new(start, end),
        }))
    }
    // port: tsc/internal/ls/signaturehelp.go:getTokenFromNodeList
    fn list_tokens(&self, lst: Option<NodeListId>, parent: NodeId) -> Result<Vec<NodeId>> {
        let Some(lst) = lst else {
            return Ok(Vec::new());
        };
        let range = self.view.list(lst)?.loc();
        let nodes = list(self.view, Some(lst))?;
        let mut index = 0;
        let mut left = range.pos();
        let mut result = Vec::new();
        while left < range.end() {
            if let Some(&node) = nodes
                .get(index)
                .filter(|&&id| self.view.node(id).is_ok_and(|n| i64::from(n.pos()) == left))
            {
                result.push(node);
                left = i64::from(self.view.node(node)?.end());
                index += 1;
            } else {
                let scanner = tsr_scanner::get_scanner_for_source_file(&self.file, left);
                let end = scanner.token_end();
                if end <= left {
                    return Err(tsr_arena::Error::InvalidTokenRange.into());
                }
                result.push(
                    self.view
                        .get_or_create_token(
                            scanner.token(),
                            scanner.token_full_start() as i32,
                            end as i32,
                            parent,
                            scanner.token_flags(),
                        )?
                        .id(),
                );
                left = end;
            }
        }
        Ok(result)
    }
    // port: tsc/internal/ls/signaturehelp.go:tryGetParameterInfo
    fn contextual_parameter(
        &mut self,
        starting: NodeId,
        c: &mut Operation<'_>,
    ) -> Result<Option<ArgumentInfo>> {
        let mut node = starting;
        if !matches!(
            self.view.node(node)?.kind().known(),
            Some(K::OpenParenToken | K::CommaToken)
        ) {
            let mut ancestor = self.view.node(node)?.parent();
            loop {
                let Some(n) = ancestor else { return Ok(None) };
                let read = self.view.node(n)?;
                if read.kind() == K::Parameter {
                    node = n;
                    break;
                }
                ancestor = read.parent();
            }
        }
        let Some(parent) = self.view.node(node)?.parent() else {
            return Ok(None);
        };
        let p = self.view.node(parent)?;
        let (context, index, count, span) = match p.kind().known() {
            Some(
                K::ParenthesizedExpression
                | K::MethodDeclaration
                | K::FunctionExpression
                | K::ArrowFunction,
            ) => {
                let Some(ListInfo {
                    index, count, span, ..
                }) = self.argument_list(node, c)?
                else {
                    return Ok(None);
                };
                let context = if p.kind() == K::MethodDeclaration {
                    c.get_contextual_type_for_object_literal_element(parent, 0)?
                } else {
                    c.get_contextual_type(parent, 0)?
                };
                (context, index, count, span)
            }
            Some(K::BinaryExpression) => {
                if self.view.node(node)?.kind() == K::OpenParenToken {
                    return Ok(None);
                }
                let mut highest = parent;
                while let Some(n) = self.view.node(highest)?.parent().filter(|n| {
                    self.view
                        .node(*n)
                        .is_ok_and(|n| n.kind() == K::BinaryExpression)
                }) {
                    highest = n;
                }
                (
                    c.get_contextual_type(highest, 0)?,
                    self.binary_parameter_count(parent)? - 1,
                    self.binary_parameter_count(highest)?,
                    TextRange::new(i64::from(p.pos()), i64::from(p.end())),
                )
            }
            _ => return Ok(None),
        };
        let Some(context) = context else {
            return Ok(None);
        };
        let context = c.get_non_nullable_type(context)?;
        let Some(id) = c.type_symbol(context)? else {
            return Ok(None);
        };
        let symbol = c.symbol_ref(id)?;
        let signatures = c.get_signatures_of_type(context, tsr_checker::SignatureKind::Call)?;
        let Some(&signature) = signatures.last() else {
            return Ok(None);
        };
        let mut better = Some(symbol);
        if c.symbol(symbol)?.name_bytes() == b"\xfetype" {
            for decl in c.symbol_declarations(symbol)?.iter().flatten() {
                let n = c.node(decl)?;
                if n.kind() == K::FunctionType {
                    if let Some(parent) = n
                        .parent()
                        .filter(|p| c.node(*p).is_ok_and(|p| ast::can_have_symbol(&p)))
                    {
                        better = c.bound_symbol_of_node(parent)?;
                        break;
                    }
                }
            }
        }
        Ok(Some(ArgumentInfo {
            invocation: Invocation::Contextual {
                node: starting,
                signature,
                symbol: better,
            },
            type_arguments: false,
            span,
            index,
            count,
        }))
    }
    fn binary_parameter_count(&self, mut node: NodeId) -> Result<usize> {
        let mut count = 2;
        loop {
            let left = self
                .view
                .node(node)?
                .data_source()
                .as_binary_expression()
                .and_then(|d| d.left())
                .ok_or(tsr_arena::Error::InvalidGraph)?;
            if self.view.node(left)?.kind() != K::BinaryExpression {
                return Ok(count);
            }
            count += 1;
            node = left;
        }
    }
    // port: tsc/internal/ls/signaturehelp.go:isSyntacticOwner
    pub(crate) fn syntactic_owner(&mut self, token: NodeId, node: NodeId) -> Result<bool> {
        let n = self.view.node(node)?;
        if !mid::is_call_or_new_expression(&n) {
            return Ok(false);
        }
        match self.view.node(token)?.kind().known() {
            Some(K::OpenParenToken | K::CommaToken) => {
                Ok(self.token_children(node)?.contains(&token))
            }
            Some(K::LessThanToken) => {
                let Some(expression) = n.expression() else {
                    return Ok(false);
                };
                self.contains_preceding(token, expression)
            }
            _ => Ok(false),
        }
    }
    // port: tsc/internal/ls/signaturehelp.go:containsPrecedingToken
    pub(crate) fn contains_preceding(&mut self, token: NodeId, container: NodeId) -> Result<bool> {
        let pos = i64::from(self.view.node(token)?.pos());
        let mut parent = self.view.node(token)?.parent();
        while let Some(p) = parent {
            if let Some(preceding) = self.nav().find_preceding_token_ex(pos, Some(p), true)? {
                let r = self.view.node(preceding)?;
                let c = self.view.node(container)?;
                return Ok(c.pos() <= r.pos() && r.end() <= c.end());
            }
            parent = self.view.node(p)?.parent();
        }
        Ok(false)
    }
    // port: tsc/internal/ls/utilities.go:getPossibleTypeArgumentsInfo
    pub(crate) fn possible_type_arguments(
        &mut self,
        mut token: NodeId,
    ) -> Result<Option<(NodeId, usize)>> {
        if !self.file.text().as_bytes().contains(&b'<') {
            return Ok(None);
        }
        let mut remaining = 0;
        let mut arguments = 0;
        loop {
            let n = self.view.node(token)?;
            match n.kind().known() {
                Some(K::LessThanToken) => {
                    let Some(mut preceding) =
                        self.nav().find_preceding_token(i64::from(n.pos()))?
                    else {
                        return Ok(None);
                    };
                    if self.view.node(preceding)?.kind() == K::QuestionDotToken {
                        let pos = i64::from(self.view.node(preceding)?.pos());
                        let Some(p) = self.nav().find_preceding_token(pos)? else {
                            return Ok(None);
                        };
                        preceding = p;
                    }
                    if self.view.node(preceding)?.kind() != K::Identifier {
                        return Ok(None);
                    }
                    token = preceding;
                    if remaining == 0 {
                        return Ok((!tsr_ast::utilities_positions::is_declaration_name(
                            self.view, token,
                        )?)
                        .then_some((token, arguments)));
                    }
                    remaining -= 1;
                }
                Some(K::GreaterThanGreaterThanGreaterThanToken) => remaining += 3,
                Some(K::GreaterThanGreaterThanToken) => remaining += 2,
                Some(K::GreaterThanToken) => remaining += 1,
                Some(K::CloseBraceToken | K::CloseParenToken | K::CloseBracketToken) => {
                    let matching = match n.kind().known() {
                        Some(K::CloseBraceToken) => K::OpenBraceToken,
                        Some(K::CloseParenToken) => K::OpenParenToken,
                        _ => K::OpenBracketToken,
                    };
                    let Some(matched) = self.preceding_matching(token, matching)? else {
                        return Ok(None);
                    };
                    token = matched;
                }
                Some(K::CommaToken) => arguments += 1,
                Some(
                    K::EqualsGreaterThanToken
                    | K::Identifier
                    | K::StringLiteral
                    | K::NumericLiteral
                    | K::BigIntLiteral
                    | K::TrueKeyword
                    | K::FalseKeyword
                    | K::TypeOfKeyword
                    | K::ExtendsKeyword
                    | K::KeyOfKeyword
                    | K::DotToken
                    | K::BarToken
                    | K::QuestionToken
                    | K::ColonToken,
                ) => {}
                _ if ast::is_type_node(&n) => {}
                _ => return Ok(None),
            }
            let pos = i64::from(self.view.node(token)?.pos());
            let Some(preceding) = self.nav().find_preceding_token(pos)? else {
                return Ok(None);
            };
            token = preceding;
        }
    }
    // port: tsc/internal/ls/utilities.go:findPrecedingMatchingToken
    fn preceding_matching(&mut self, mut token: NodeId, matching: K) -> Result<Option<NodeId>> {
        let close = self.view.node(token)?.kind();
        let matching_text = tsr_scanner::token_to_string(matching);
        let close_text =
            tsr_scanner::token_to_string(close.known().ok_or(tsr_arena::Error::InvalidGraph)?);
        let text = self.file.text().as_bytes();
        let Some(guess) = text
            .windows(matching_text.len())
            .rposition(|s| s == matching_text.as_bytes())
        else {
            return Ok(None);
        };
        let close_guess = text
            .windows(close_text.len())
            .rposition(|s| s == close_text.as_bytes());
        if close_guess.is_none_or(|end| end < guess) {
            if let Some(node) = self.nav().find_preceding_token(guess as i64 + 1)? {
                if self.view.node(node)?.kind() == matching {
                    return Ok(Some(node));
                }
            }
        }
        let mut remaining = 0;
        loop {
            let pos = i64::from(self.view.node(token)?.pos());
            let Some(previous) = self.nav().find_preceding_token(pos)? else {
                return Ok(None);
            };
            token = previous;
            let kind = self.view.node(token)?.kind();
            if kind == matching {
                if remaining == 0 {
                    return Ok(Some(token));
                }
                remaining -= 1;
            } else if kind == close {
                remaining += 1;
            }
        }
    }
}
// port: tsc/internal/ls/signaturehelp.go:getArgumentIndexOrCount
fn argument_index(
    syntax: &Syntax<'_>,
    args: &[NodeId],
    node: Option<NodeId>,
    c: &mut Operation<'_>,
) -> Result<usize> {
    let mut index = 0;
    let mut skip_comma = false;
    for &arg in args {
        let n = syntax.view.node(arg)?;
        if node == Some(arg) {
            if !skip_comma && n.kind() == K::CommaToken {
                index += 1;
            }
            return Ok(index);
        }
        if n.kind() == K::SpreadElement {
            let expression = n.expression().ok_or(tsr_arena::Error::InvalidGraph)?;
            let ty = c.get_type_at_location(expression)?;
            if let Some((fixed, flags)) = c.tuple_element_flags(ty)? {
                if fixed != 0 {
                    index += flags
                        .iter()
                        .position(|f| f & tsr_checker::element_flags::REQUIRED == 0)
                        .unwrap_or(fixed as usize);
                }
            }
            skip_comma = true;
        } else if n.kind() != K::CommaToken {
            index += 1;
            skip_comma = true;
        } else if skip_comma {
            skip_comma = false;
        } else {
            index += 1;
        }
    }
    if node.is_none()
        && args
            .last()
            .is_some_and(|&a| syntax.view.node(a).is_ok_and(|n| n.kind() == K::CommaToken))
    {
        index += 1;
    }
    Ok(index)
}
