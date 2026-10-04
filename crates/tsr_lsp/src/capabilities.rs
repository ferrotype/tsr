use tsr_jsstring::PositionEncoding;
use tsr_lsproto as lsp;

// The literal is the pin's capability declaration, decoded through the same
// typed codec as its tests. L3–L6 handlers still refuse their named features.
pub fn initialize(
    caps: &lsp::ClientCapabilities,
    encoding: PositionEncoding,
) -> lsp::InitializeResult {
    let mut server = lsp::ServerCapabilities::default();
    tsr_json::unmarshal(
        include_bytes!("capabilities.json"),
        &mut server,
        tsr_json::Options::default(),
    )
    .expect("static capability declaration");
    server.position_encoding = Some(Box::new(match encoding {
        PositionEncoding::Utf8 => lsp::PositionEncodingKind(lsp::PositionEncodingKind::UTF8.into()),
        PositionEncoding::Utf16 => {
            lsp::PositionEncodingKind(lsp::PositionEncodingKind::UTF16.into())
        }
    }));
    let offered = caps
        .text_document
        .as_deref()
        .and_then(|d| d.semantic_tokens.as_deref());
    let legend = server
        .semantic_tokens_provider
        .as_mut()
        .unwrap()
        .options
        .as_mut()
        .unwrap()
        .legend
        .as_mut()
        .unwrap();
    if let Some(offered) = offered {
        // port: tsc/internal/ls/semantictokens.go:SemanticTokensLegend
        legend.token_types = tsr_ls::TOKEN_TYPES
            .iter()
            .copied()
            .filter(|t| offered.token_types.iter().any(|s| s == t))
            .map(str::to_owned)
            .collect();
        legend.token_modifiers = tsr_ls::TOKEN_MODIFIERS
            .iter()
            .copied()
            .filter(|t| offered.token_modifiers.iter().any(|s| s == t))
            .map(str::to_owned)
            .collect();
    }
    lsp::InitializeResult {
        capabilities: Some(Box::new(server)),
        server_info: Some(Box::new(lsp::ServerInfo {
            name: "typescript".into(),
            version: Some(Box::new(tsr_core::version().into())),
        })),
    }
}

pub fn encoding(caps: &lsp::ClientCapabilities) -> PositionEncoding {
    if caps
        .general
        .as_deref()
        .and_then(|g| g.position_encodings.as_deref())
        .is_some_and(|encodings| {
            encodings
                .iter()
                .any(|e| e.0 == lsp::PositionEncodingKind::UTF8)
        })
    {
        PositionEncoding::Utf8
    } else {
        PositionEncoding::Utf16
    }
}
