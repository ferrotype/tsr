//! Diagnostic-only type rendering from the reference graph.
//!
//! Member and argument links are followed here, when native TypeToString would
//! request them. Construction never invokes this renderer to invent a name.

use super::*;
use std::collections::HashSet;

const FUNCTION: u8 = 0;
const UNION: u8 = 1;
const INTERSECTION: u8 = 2;
const ARRAY: u8 = 3;
const ATOM: u8 = 4;

struct Printed {
    text: String,
    precedence: u8,
}

impl Printed {
    fn atom(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            precedence: ATOM,
        }
    }

    fn in_context(self, precedence: u8) -> String {
        if self.precedence < precedence {
            format!("({})", self.text)
        } else {
            self.text
        }
    }
}

impl Checker {
    /// Formats only the supported native diagnostic forms. Any unavailable
    /// semantic payload is an explicit error, never a placeholder type name.
    pub fn type_to_string(&self, ty: &Rc<TypeCell>) -> Result<String, Error> {
        if !self.graph.owns(ty) {
            return unsupported("diagnostic type belongs to another graph");
        }
        Display {
            graph: &self.graph,
            visiting: HashSet::new(),
        }
        .ty(ty)
        .map(|text| text.text)
    }

    pub(crate) fn signature_to_string(&self, signature: &Signature) -> Result<String, Error> {
        Display {
            graph: &self.graph,
            visiting: HashSet::new(),
        }
        .signature(signature, true)
    }
}

struct Display<'a> {
    graph: &'a Graph,
    visiting: HashSet<u32>,
}

impl Display<'_> {
    // port: tsc/internal/checker/nodebuilderimpl.go:NodeBuilderImpl.typeToTypeNode
    fn ty(&mut self, ty: &Rc<TypeCell>) -> Result<Printed, Error> {
        if let Some(literal) = &ty.literal {
            return Ok(Printed::atom(match literal {
                LiteralValue::String(bytes) => quote(bytes)?,
                LiteralValue::Number(bits) => {
                    ts_jsnum::Number::new(f64::from_bits(*bits)).to_string()
                }
                LiteralValue::Boolean(value) => value.to_string(),
                LiteralValue::BigInt { negative, digits } => format!(
                    "{}{}n",
                    if *negative { "-" } else { "" },
                    std::str::from_utf8(digits)
                        .map_err(|_| Error::Unsupported("non-ASCII bigint digits".into()))?
                ),
            }));
        }
        // Intrinsic keywords precede aliases in the native builder. A union of
        // both boolean literals carries BOOLEAN too and prints as boolean.
        for (flag, text) in [
            (flags::UNKNOWN, "unknown"),
            (flags::STRING, "string"),
            (flags::NUMBER, "number"),
            (flags::BIG_INT, "bigint"),
            (flags::VOID, "void"),
            (flags::UNDEFINED, "undefined"),
            (flags::NULL, "null"),
            (flags::NEVER, "never"),
            (flags::ES_SYMBOL, "symbol"),
            (flags::NON_PRIMITIVE, "object"),
        ] {
            if ty.flags & flag != 0 {
                return Ok(Printed::atom(text));
            }
        }
        if ty.flags & flags::BOOLEAN != 0 && ty.alias.is_none() {
            return Ok(Printed::atom("boolean"));
        }
        if ty.alias.is_some() {
            let arguments = ty
                .alias_arguments
                .get()
                .map(|arguments| {
                    arguments
                        .iter()
                        .map(|argument| {
                            self.ty(&argument.upgrade().ok_or(Error::Released)?)
                                .map(|text| text.text)
                        })
                        .collect::<Result<Vec<_>, Error>>()
                })
                .transpose()?
                .unwrap_or_default();
            return Ok(Printed::atom(if arguments.is_empty() {
                ty.name.to_string()
            } else {
                format!("{}<{}>", ty.name, arguments.join(", "))
            }));
        }
        if ty.flags & flags::UNIQUE_ES_SYMBOL != 0 {
            return Ok(Printed::atom("unique symbol"));
        }
        if ty.flags & flags::ANY != 0 {
            return Ok(Printed::atom(if ty.name.as_ref() == "intrinsic" {
                "intrinsic"
            } else {
                "any"
            }));
        }
        if ty.flags & flags::TYPE_PARAMETER != 0 {
            return Ok(Printed::atom(ty.name.to_string()));
        }
        if !self.visiting.insert(ty.id) {
            return Ok(Printed::atom("..."));
        }
        let result = self.composite(ty);
        self.visiting.remove(&ty.id);
        result
    }

    fn composite(&mut self, ty: &Rc<TypeCell>) -> Result<Printed, Error> {
        if ty.flags & (flags::UNION | flags::INTERSECTION) != 0 {
            let union = ty.flags & flags::UNION != 0;
            let precedence = if union { UNION } else { INTERSECTION };
            let values = ty.types()?;
            let mut printed = Vec::new();
            let mut null = false;
            let mut undefined = false;
            let boolean_pair = union
                && values
                    .iter()
                    .any(|ty| matches!(ty.literal, Some(LiteralValue::Boolean(false))))
                && values
                    .iter()
                    .any(|ty| matches!(ty.literal, Some(LiteralValue::Boolean(true))));
            let mut boolean_written = false;
            // port: tsc/internal/checker/printer.go:Checker.formatUnionTypes
            for value in values {
                if union && value.flags & flags::NULL != 0 {
                    null = true;
                    continue;
                }
                if union && value.flags & flags::UNDEFINED != 0 {
                    undefined = true;
                    continue;
                }
                if boolean_pair && value.flags & flags::BOOLEAN_LITERAL != 0 {
                    if !boolean_written {
                        printed.push("boolean".to_owned());
                        boolean_written = true
                    }
                    continue;
                }
                printed.push(self.ty(&value)?.in_context(precedence));
            }
            if null {
                printed.push("null".to_owned())
            }
            if undefined {
                printed.push("undefined".to_owned())
            }
            return Ok(Printed {
                text: printed.join(if union { " | " } else { " & " }),
                precedence,
            });
        }
        if let Some(tuple) = ty.tuple_shape() {
            let mut elements = Vec::new();
            for (element, flag) in tuple.elements()?.iter().zip(&tuple.element_flags) {
                let text = self.ty(element)?;
                elements.push(match *flag {
                    element_flags::REQUIRED => text.text,
                    element_flags::OPTIONAL => format!("{}?", text.in_context(ARRAY)),
                    element_flags::REST => format!("...{}[]", text.in_context(ARRAY)),
                    element_flags::VARIADIC => format!("...{}", text.text),
                    _ => return unsupported("diagnostic tuple element flag"),
                });
            }
            return Ok(Printed::atom(format!(
                "{}[{}]",
                if tuple.readonly { "readonly " } else { "" },
                elements.join(", ")
            )));
        }
        if let Some(array) = ty.array_element() {
            let element = self.ty(&array.element()?)?.in_context(ARRAY);
            return Ok(Printed {
                text: format!(
                    "{}{element}[]",
                    if array.readonly { "readonly " } else { "" }
                ),
                precedence: ARRAY,
            });
        }
        if let Some(reference) = ty.reference_shape() {
            let target = reference.target()?;
            let arguments = reference.arguments()?;
            let arity = target
                .generic_target()
                .map(|generic| generic.parameters().map(|parameters| parameters.len()))
                .transpose()?
                .unwrap_or(0);
            if arguments.len() < arity {
                return unsupported("diagnostic reference type-argument arity");
            }
            let arguments = arguments[..arity]
                .iter()
                .map(|argument| self.ty(argument).map(|text| text.text))
                .collect::<Result<Vec<_>, _>>()?;
            return Ok(Printed::atom(if arguments.is_empty() {
                target.name.to_string()
            } else {
                format!("{}<{}>", target.name, arguments.join(", "))
            }));
        }
        if ty.object_flags & (object_flags::CLASS | object_flags::INTERFACE) != 0 {
            return Ok(Printed::atom(ty.name.to_string()));
        }
        if let Some(template) = ty.template_parts() {
            let types = template.types()?;
            let mut result = "`".to_owned();
            for (index, text) in template.texts.iter().enumerate() {
                result.push_str(&escaped(text, ts_jsstring::QuoteChar::Backtick)?);
                if let Some(ty) = types.get(index) {
                    result.push_str(&format!("${{{}}}", self.ty(ty)?.text))
                }
            }
            result.push('`');
            return Ok(Printed::atom(result));
        }
        if ty.flags & flags::OBJECT != 0 {
            return self.object(ty);
        }
        unsupported("diagnostic rendering requires an unavailable type payload")
    }

    // port: tsc/internal/checker/nodebuilderimpl.go:NodeBuilderImpl.createTypeNodeFromObjectType
    fn object(&mut self, ty: &Rc<TypeCell>) -> Result<Printed, Error> {
        let shape = ty.structure(self.graph)?;
        if shape.members.is_empty() && shape.index_infos.is_empty() {
            let signature = match (
                shape.call_signatures.as_slice(),
                shape.construct_signatures.as_slice(),
            ) {
                ([signature], []) | ([], [signature]) => Some(signature),
                _ => None,
            };
            if let Some(signature) = signature {
                return Ok(Printed {
                    text: self.signature(signature, true)?,
                    precedence: FUNCTION,
                });
            }
        }
        let mut items = Vec::new();
        for signature in &shape.call_signatures {
            items.push(format!("{};", self.signature(signature, false)?))
        }
        for signature in &shape.construct_signatures {
            if signature.is_abstract {
                return unsupported("abstract construct signatures in diagnostic object");
            }
            items.push(format!("{};", self.signature(signature, false)?));
        }
        for index in &shape.index_infos {
            items.push(format!(
                "{}[x: {}]: {};",
                if index.readonly { "readonly " } else { "" },
                self.ty(&index.key()?)?.text,
                self.ty(&index.value()?)?.text
            ));
        }
        for member in &shape.members {
            let member_type = member.r#type()?;
            let name = property_name(&member.name)?;
            let optional = if member.optional { "?" } else { "" };
            // The source frontend retains method variance on its signatures.
            // This preserves method syntax without eager source-node rendering.
            if member_type.flags & flags::OBJECT != 0
                && member_type.alias.is_none()
                && member_type.object_flags & object_flags::ANONYMOUS != 0
            {
                let method = member_type.structure(self.graph)?;
                if !method.call_signatures.is_empty()
                    && method.members.is_empty()
                    && method.index_infos.is_empty()
                    && method.construct_signatures.is_empty()
                    && method
                        .call_signatures
                        .iter()
                        .all(|signature| signature.bivariant_parameters)
                {
                    for signature in &method.call_signatures {
                        items.push(format!(
                            "{name}{optional}{};",
                            self.signature(signature, false)?
                        ));
                    }
                    continue;
                }
            }
            items.push(format!(
                "{}{name}{optional}: {};",
                if member.readonly { "readonly " } else { "" },
                self.ty(&member_type)?.text
            ));
        }
        Ok(Printed::atom(if items.is_empty() {
            "{}".to_owned()
        } else {
            format!("{{ {} }}", items.join(" "))
        }))
    }

    fn signature(&mut self, signature: &Signature, arrow: bool) -> Result<String, Error> {
        let mut generics = Vec::new();
        if let Some(generic) = &signature.generic {
            for parameter in generic.parameters()? {
                let mut text = self.ty(&parameter)?.text;
                if let Some(constraint) = parameter
                    .type_parameter_shape()
                    .map(TypeParameterShape::constraint)
                    .transpose()?
                    .flatten()
                {
                    text.push_str(&format!(" extends {}", self.ty(&constraint)?.text));
                }
                generics.push(text);
            }
        } else if signature.type_parameters != 0 {
            return unsupported("diagnostic generic signature has no parameter metadata");
        }
        let mut parameters = Vec::new();
        if let Some(this) = signature.this_type()? {
            parameters.push(format!("this: {}", self.ty(&this)?.text))
        }
        for index in 0..signature.parameter_count() {
            let name = signature
                .parameter_names
                .get(index)
                .map_or_else(|| format!("arg{index}"), ToString::to_string);
            let rest = signature.has_rest_parameter && index + 1 == signature.parameter_count();
            let optional = !rest && index >= signature.min_argument_count;
            let ty = signature.parameter(index)?.ok_or(Error::ResolutionFailed)?;
            parameters.push(format!(
                "{}{name}{}: {}",
                if rest { "..." } else { "" },
                if optional { "?" } else { "" },
                self.ty(&ty)?.text
            ));
        }
        Ok(format!(
            "{}{}{}({}){}{}",
            if signature.is_abstract {
                "abstract "
            } else {
                ""
            },
            if signature.is_construct { "new " } else { "" },
            if generics.is_empty() {
                String::new()
            } else {
                format!("<{}>", generics.join(", "))
            },
            parameters.join(", "),
            if arrow { " => " } else { ": " },
            self.ty(&signature.return_type()?)?.text
        ))
    }
}

fn escaped(bytes: &[u8], quote: ts_jsstring::QuoteChar) -> Result<String, Error> {
    String::from_utf8(ts_jsstring::escape::escape_string(bytes, quote))
        .map_err(|_| Error::Unsupported("diagnostic literal escaping produced non-UTF8".into()))
}

fn quote(bytes: &[u8]) -> Result<String, Error> {
    Ok(format!(
        "\"{}\"",
        escaped(bytes, ts_jsstring::QuoteChar::Double)?
    ))
}

fn property_name(name: &str) -> Result<String, Error> {
    if ts_scanner::is_identifier_text(name.as_bytes(), ts_core::LanguageVariant::STANDARD)
        || name.parse::<u64>().is_ok()
    {
        Ok(name.to_owned())
    } else if name.starts_with("__@") || name.starts_with("\u{ffff}@unique:") {
        unsupported("diagnostic unique-symbol property name")
    } else {
        quote(name.as_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anonymous_intersections_render_members_only_when_formatted() {
        let checker = Checker::new();
        let string = checker.graph.primitive(flags::STRING, "string");
        let number = checker.graph.primitive(flags::NUMBER, "number");
        let object = |name: &'static str, ty: &Rc<TypeCell>| {
            let ty = Rc::downgrade(ty);
            checker.graph.allocate_full(
                flags::OBJECT,
                object_flags::ANONYMOUS,
                "__type".into(),
                None,
                None,
                None,
                false,
                Vec::new(),
                true,
                Some(Box::new(move |_, _| {
                    Ok(Structure {
                        members: vec![Member {
                            name_type: None,
                            name: name.into(),
                            optional: false,
                            readonly: false,
                            class_member: true,
                            r#type: ty.clone().into(),
                        }],
                        ..Default::default()
                    })
                })),
            )
        };
        let a = object("a", &string);
        let b = object("b", &number);
        let intersection = checker.graph.allocate_full(
            flags::INTERSECTION,
            0,
            "intersection".into(),
            None,
            None,
            None,
            false,
            vec![Rc::downgrade(&a), Rc::downgrade(&b)],
            false,
            None,
        );
        assert_eq!(a.resolutions(), 0);
        assert_eq!(b.resolutions(), 0);
        assert_eq!(
            checker.type_to_string(&intersection).unwrap(),
            "{ a: string; } & { b: number; }"
        );
        assert_eq!(a.resolutions(), 1);
        assert_eq!(b.resolutions(), 1);
    }

    #[test]
    fn readonly_rest_tuple_uses_native_union_display_order() {
        let checker = Checker::new();
        let undefined = checker.graph.primitive(flags::UNDEFINED, "undefined");
        let number = checker.graph.primitive(flags::NUMBER, "number");
        let string = checker.graph.primitive(flags::STRING, "string");
        let f = checker.graph.intern_literal(
            flags::BOOLEAN_LITERAL,
            LiteralValue::Boolean(false),
            "false",
        );
        let t = checker.graph.intern_literal(
            flags::BOOLEAN_LITERAL,
            LiteralValue::Boolean(true),
            "true",
        );
        let rest = checker.graph.union(&[undefined, number, f, t]).unwrap();
        let tuple = checker.graph.primitive(flags::OBJECT, "tuple");
        tuple
            .set_tuple_shape(
                &[string, rest],
                &[element_flags::REQUIRED, element_flags::REST],
                true,
            )
            .unwrap();
        assert_eq!(
            checker.type_to_string(&tuple).unwrap(),
            "readonly [string, ...(number | boolean | undefined)[]]"
        );
    }
}
