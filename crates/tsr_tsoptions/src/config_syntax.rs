//! Config syntax ownership and the exact tsoptions property/range operations.
use ts_ast::{AstFile, Diagnostic, NodeDataRead, NodeId, SourceFileParseOptions, SyntaxKind as K};
use ts_core::{ScriptKind, TextRange};
use ts_diagnostics::Message;
use ts_jsstring::{JsString, SourceText};

#[derive(Clone, Debug)]
pub struct TsConfigSourceFile {
    pub file: AstFile,
    pub root: NodeId,
    pub extended_source_files: Vec<JsString>,
}
impl TsConfigSourceFile {
    /// Input is already filesystem-loaded text, matching ParseSourceFile.
    /// port: tsc/internal/tsoptions/tsconfigparsing.go:NewTsconfigSourceFileFromFilePath
    pub fn parse(file_name: JsString, path: JsString, source: SourceText) -> Self {
        let parsed = ts_parser::parse_source_file(
            source,
            ScriptKind::JSON,
            SourceFileParseOptions {
                file_name,
                path,
                ..SourceFileParseOptions::default()
            },
        );
        let root = parsed.root();
        Self {
            file: parsed.publish_unbound(),
            root,
            extended_source_files: Vec::new(),
        }
    }
    pub fn object(&self) -> Option<NodeId> {
        let view = self.file.view();
        let node = view.node(self.root).expect("config source owner");
        let NodeDataRead::SourceFile(data) = node.data() else {
            return None;
        };
        let list = view.list(data.statements()?).expect("config statements");
        let nodes = view
            .node_slice(list.nodes())
            .expect("config statement slice");
        let first = nodes.first().flatten()?;
        let expression = view
            .node(first)
            .expect("config expression statement")
            .expression()?;
        (view.node(expression).expect("config expression").kind() == K::ObjectLiteralExpression)
            .then_some(expression)
    }
}

/// Find the first property along a nested key path, returning the assignment,
/// not its initializer. Duplicate names preserve first-property source order.
pub fn find_property(config: &TsConfigSourceFile, path: &[&[u8]]) -> Option<NodeId> {
    let mut object = config.object()?;
    let mut result = None;
    for (index, key) in path.iter().enumerate() {
        let property = find_property_in_object(config, object, &[*key])?;
        result = Some(property);
        if index + 1 < path.len() {
            let node = config.file.view().node(property).expect("config property");
            let NodeDataRead::PropertyAssignment(data) = node.data() else {
                unreachable!("property lookup returns an assignment")
            };
            object = data.initializer()?;
        }
    }
    result
}

/// port: tsc/internal/tsoptions/tsconfigparsing.go:ForEachPropertyAssignment
/// Alternatives are matched during source-order traversal, not by key order.
pub fn find_property_in_object(
    config: &TsConfigSourceFile,
    object: NodeId,
    keys: &[&[u8]],
) -> Option<NodeId> {
    let view = config.file.view();
    let node = view.node(object).expect("config object owner");
    let NodeDataRead::ObjectLiteralExpression(data) = node.data() else {
        return None;
    };
    let list = view.list(data.properties()?).expect("config properties");
    let nodes = view
        .node_slice(list.nodes())
        .expect("config property slice");
    for property in nodes.iter().flatten() {
        let node = view.node(property).expect("config property");
        let NodeDataRead::PropertyAssignment(data) = node.data() else {
            continue;
        };
        if let Some(name) = data.name().and_then(|name| property_name(config, name)) {
            if keys.iter().any(|key| *key == name.as_bytes()) {
                return Some(property);
            }
        }
    }
    None
}

pub fn property_name(config: &TsConfigSourceFile, node: NodeId) -> Option<JsString> {
    let view = config.file.view();
    let read = view.node(node).expect("config property name");
    match read.kind().known() {
        Some(
            K::Identifier
            | K::PrivateIdentifier
            | K::StringLiteral
            | K::NumericLiteral
            | K::BigIntLiteral
            | K::NoSubstitutionTemplateLiteral,
        ) => Some(view.node_text(node).expect("text name").into_js_string()),
        Some(K::ComputedPropertyName) => {
            let expression = read.expression()?;
            let kind = view
                .node(expression)
                .expect("computed property expression")
                .kind();
            matches!(
                kind.known(),
                Some(K::StringLiteral | K::NumericLiteral | K::NoSubstitutionTemplateLiteral)
            )
            .then(|| {
                view.node_text(expression)
                    .expect("computed text name")
                    .into_js_string()
            })
        }
        Some(K::JsxNamespacedName) => {
            let NodeDataRead::JsxNamespacedName(data) = read.data() else {
                unreachable!("JSX name payload")
            };
            let mut result = view
                .node_text(data.namespace()?)
                .expect("namespace text")
                .as_bytes()
                .to_vec();
            result.push(b':');
            result.extend_from_slice(view.node_text(data.name()?).expect("name text").as_bytes());
            Some(JsString::from_bytes(result))
        }
        _ => None,
    }
}

/// port: tsc/internal/tsoptions/errors.go:CreateDiagnosticForNodeInSourceFile
pub fn diagnostic_for_node(
    config: &TsConfigSourceFile,
    node: NodeId,
    message: &'static Message,
    args: Vec<JsString>,
) -> Diagnostic {
    let view = config.file.view();
    let node = view.node(node).expect("config diagnostic node owner");
    let source = view.source_file(config.root).expect("config source file");
    let start = ts_scanner::skip_trivia(source.text().as_bytes(), i64::from(node.pos()));
    Diagnostic::new(
        Some(config.root),
        TextRange::new(start, i64::from(node.end())),
        message,
        args,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn property_alternatives_follow_syntax_order_and_ranges_skip_trivia() {
        let text=br#"{"compilerOptions": {/* comment */ "strict": false, "target": "esnext"}, "compilerOptions": {"strict":true}}"#;
        let config = TsConfigSourceFile::parse(
            JsString::from_bytes(b"/tsconfig.json".as_slice()),
            JsString::from_bytes(b"/tsconfig.json".as_slice()),
            SourceText::from_loaded_bytes(text.as_slice()),
        );
        let property = find_property(&config, &[b"compilerOptions"]).unwrap();
        let read = config.file.view().node(property).unwrap();
        let NodeDataRead::PropertyAssignment(data) = read.data() else {
            panic!("expected property")
        };
        let selected = find_property_in_object(
            &config,
            data.initializer().unwrap(),
            &[b"target", b"strict"],
        )
        .unwrap();
        let node = config.file.view().node(selected).unwrap();
        assert_eq!(
            property_name(&config, node.name().unwrap())
                .unwrap()
                .as_bytes(),
            b"strict"
        );
        let diagnostic = diagnostic_for_node(
            &config,
            selected,
            ts_diagnostics::Unknown_compiler_option_0,
            vec![],
        );
        assert_eq!(
            diagnostic.loc.pos(),
            text.windows(8).position(|w| w == b"\"strict\"").unwrap() as i64
        );
        assert_eq!(diagnostic.loc.end(), i64::from(node.end()));
        assert_eq!(diagnostic.file, Some(config.root));
    }
}
