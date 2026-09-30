//! The ported mappers through the production host, as the harness runs them.
use crate::*;
use std::sync::Arc;
use tsr_contentmapper::{
    new_host, Host, ProjectSpec, Request, TransformErrorKind, TransformResult,
};
use tsr_core::CompilerOptions;
use tsr_jsstring::JsString;
use tsr_tsoptions::config_mappers::{ContentMapper, MapperManifest};

/// A resolved mapper entry naming `command` as its executable.
fn mapper(command: &str, declared: &[&str]) -> ContentMapper {
    ContentMapper {
        package: JsString::from_bytes(b"mapper".as_slice()),
        extensions: vec![JsString::from_bytes(b".box".as_slice())],
        manifest: MapperManifest {
            name: JsString::from_bytes(b"mapper".as_slice()),
            version: JsString::from_bytes(b"1.0.0".as_slice()),
            exec: Some(vec![JsString::from_bytes(command.as_bytes())]),
            compiler_options: Some(
                declared
                    .iter()
                    .map(|name| JsString::from_bytes(name.as_bytes()))
                    .collect(),
            ),
            ..MapperManifest::default()
        },
        ..ContentMapper::default()
    }
}

fn transform(command: &str, content: &str) -> Result<TransformResult, tsr_contentmapper::Error> {
    let host = new_host(
        &tsr_ipc::Context::background(),
        new_spawner(),
        tsr_locale::Locale::default(),
    );
    let options = CompilerOptions {
        target: tsr_core::ScriptTarget::ESNEXT,
        jsx: tsr_core::compiler_options::JsxEmit::PRESERVE,
        ..CompilerOptions::default()
    };
    let project = host
        .project(ProjectSpec {
            config_file_name: JsString::from_bytes(b"/tsconfig.json".as_slice()),
            mappers: Arc::from(vec![mapper(command, DECLARED_OPTIONS)]),
            compiler_options: Arc::new(options),
        })
        .unwrap();
    let result = project.transform(
        0,
        &Request {
            file_name: JsString::from_bytes(b"/widget.box".as_slice()),
            content: content.as_bytes().to_vec(),
        },
    );
    project.close().unwrap();
    host.close().unwrap();
    result
}

#[test]
fn the_transforming_mapper_renders_options_and_reports_unclosed_interpolations() {
    let result = transform(
        TRANSFORMING_MAPPER,
        "const t = #{target};\nconst bad = #{jsx\n",
    )
    .unwrap();
    assert_eq!(
        result.text,
        "const __VERSION = \"1.0.0\";\nconst t = 99;\nconst bad = undefined\n"
    );
    let segments = result.mappings.unwrap().segments().to_vec();
    assert_eq!(segments.len(), 5);
    assert_eq!(
        (
            segments[1].kind,
            segments[1].original_start,
            segments[1].original_end
        ),
        (1, 10, 19)
    );
    let diagnostic = &result.diagnostics[0];
    assert_eq!(
        diagnostic.message_text.as_bytes(),
        b"Unclosed interpolation."
    );
    assert_eq!(
        (diagnostic.loc.pos(), diagnostic.loc.end(), diagnostic.code),
        (33, 38, 1000)
    );
    assert_eq!(diagnostic.source.as_bytes(), b"box");
}

#[test]
fn the_transforming_mapper_maps_directives_to_the_following_line() {
    let content = "// @box-expect-error: Expected an error.\nconst x: number = \"no\";\n// @box-ignore\nconst y: number = \"no\";\n";
    let result = transform(TRANSFORMING_MAPPER, content).unwrap();
    let directives = &result.diagnostic_directives;
    assert_eq!(directives.len(), 2);
    assert_eq!((directives[0].policy, directives[0].unused_code), (1, 2578));
    assert_eq!(
        directives[0].unused_message_text.as_bytes(),
        b"Expected an error."
    );
    assert_eq!(
        (
            directives[0].original_range.pos(),
            directives[0].original_range.end()
        ),
        (0, 40)
    );
    assert_eq!(directives[1].policy, 0);
    let invalid = transform(
        TRANSFORMING_MAPPER,
        "// @box-invalid-directive: invalid-policy\n",
    )
    .unwrap_err();
    assert_eq!(invalid.transform_kind(), Some(TransformErrorKind::Response));
    let extension = transform(
        TRANSFORMING_MAPPER,
        "// @box-extension: .coffee\nexport const value = 1;\n",
    )
    .unwrap_err();
    assert!(
        matches!(extension.cause(), tsr_contentmapper::Error::InvalidVirtualExtension(e) if e == ".coffee")
    );
}

#[test]
fn the_other_mappers_answer_as_the_pin_does() {
    let lisp = transform(LISP_MAPPER, "(+ 1 2 \"oops\")\n").unwrap();
    assert_eq!(lisp.text, r#"add(1, 2, "oops");"#);
    assert_eq!(lisp.mappings.unwrap().segments()[0].kind, 2);
    let failing = transform(FAILING_MAPPER, "x").unwrap_err();
    assert_eq!(failing.transform_kind(), Some(TransformErrorKind::Request));
    let supplemental = transform(SUPPLEMENTAL_MAPPER, "const value = 1;").unwrap();
    assert_eq!(supplemental.text, "export {};");
    assert_eq!(supplemental.mappings.unwrap().segments().len(), 0);
    assert_eq!(supplemental.supplemental[0].text, "const value = 1;");
    let diagnostics = transform(SUPPLEMENTAL_DIAGNOSTICS_MAPPER, "x;").unwrap();
    assert_eq!(
        diagnostics.supplemental[0].text,
        "missingSupplementalGlobal;\nx;"
    );
    let module = transform(SUPPLEMENTAL_MODULE_MAPPER, "").unwrap();
    assert_eq!(
        module.supplemental[0].text,
        r#"export const privateValue: number = "wrong";"#
    );
    let unknown = transform("hoisting-mapper", "x").unwrap_err();
    assert_eq!(
        unknown.transform_kind(),
        Some(TransformErrorKind::Initialize)
    );
}
