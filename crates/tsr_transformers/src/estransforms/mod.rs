//! The ECMAScript downlevel transforms (`transformers/estransforms`).
pub mod async_;
pub mod classfields;
pub mod classthis;
pub mod esdecorator;
pub mod exponentiation;
pub mod forawait;
pub mod logicalassignment;
pub mod namedevaluation;
pub mod nullishcoalescing;
pub mod objectrestspread;
pub mod optionalcatch;
pub mod optionalchain;
pub mod taggedtemplate;
pub mod usestrict;
pub mod using;
pub mod utilities;

use crate::transformer::{chain, TransformOptions, Transformer, TransformerFactory};
use tsr_core::ScriptTarget;

// `definitions.go`'s package variables, as functions: a factory owns its
// components, so each use builds its chain.

fn es_decorator_and_class_fields<'a>() -> TransformerFactory<'a> {
    chain(vec![
        Box::new(esdecorator::new_es_decorator_transformer),
        Box::new(classfields::new_class_fields_transformer),
    ])
}

/// `NewESNextTransformer`.
pub fn new_es_next_transformer<'a>() -> TransformerFactory<'a> {
    chain(vec![
        Box::new(using::new_using_declaration_transformer),
        es_decorator_and_class_fields(),
    ])
}

// 2025: only module system syntax (import attributes, json modules), untransformed regex modifiers
// 2024: no new downlevel syntax
// 2023: no new downlevel syntax
// 2022: class static blocks and class fields are handled by newClassFieldsTransformer

/// `NewES2021Transformer`.
pub fn new_es2021_transformer<'a>() -> TransformerFactory<'a> {
    chain(vec![
        new_es_next_transformer(),
        Box::new(logicalassignment::new_logical_assignment_transformer),
    ])
}

/// `NewES2020Transformer`.
pub fn new_es2020_transformer<'a>() -> TransformerFactory<'a> {
    chain(vec![
        new_es2021_transformer(),
        Box::new(nullishcoalescing::new_nullish_coalescing_transformer),
        Box::new(optionalchain::new_optional_chain_transformer),
    ])
}

/// `NewES2019Transformer`.
pub fn new_es2019_transformer<'a>() -> TransformerFactory<'a> {
    chain(vec![
        new_es2020_transformer(),
        Box::new(optionalcatch::new_optional_catch_transformer),
    ])
}

/// `NewES2018Transformer`.
pub fn new_es2018_transformer<'a>() -> TransformerFactory<'a> {
    chain(vec![
        new_es2019_transformer(),
        Box::new(objectrestspread::new_object_rest_spread_transformer),
        Box::new(forawait::new_for_await_transformer),
        Box::new(taggedtemplate::new_tagged_template_lift_restriction_transformer),
    ])
}

/// `NewES2017Transformer`.
pub fn new_es2017_transformer<'a>() -> TransformerFactory<'a> {
    chain(vec![
        new_es2018_transformer(),
        Box::new(async_::new_async_transformer),
    ])
}

/// `NewES2016Transformer`.
pub fn new_es2016_transformer<'a>() -> TransformerFactory<'a> {
    chain(vec![
        new_es2017_transformer(),
        Box::new(exponentiation::new_exponentiation_transformer),
    ])
}

// port: tsc/internal/transformers/estransforms/definitions.go:GetESTransformer
pub fn get_es_transformer<'a>(opts: &TransformOptions<'a>) -> Option<Transformer<'a>> {
    let factory = match opts.compiler_options.emit_script_target() {
        ScriptTarget::ESNEXT => es_decorator_and_class_fields(),
        ScriptTarget::ES2025
        | ScriptTarget::ES2024
        | ScriptTarget::ES2023
        | ScriptTarget::ES2022
        | ScriptTarget::ES2021 => new_es_next_transformer(),
        ScriptTarget::ES2020 => new_es2021_transformer(),
        ScriptTarget::ES2019 => new_es2020_transformer(),
        ScriptTarget::ES2018 => new_es2019_transformer(),
        ScriptTarget::ES2017 => new_es2018_transformer(),
        ScriptTarget::ES2016 => new_es2017_transformer(),
        // other, older, option, transform maximally
        _ => new_es2016_transformer(),
    };
    factory(opts)
}
