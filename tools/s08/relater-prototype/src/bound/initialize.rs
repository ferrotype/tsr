//! The pinned NewChecker type constructors, followed by bound global lookups.
//! Each named marker owns an actual graph cell; counters observe those cells.

use super::*;
use crate::bound_input::BoundInputOptions;
use ts_ast::symbol_flags as sf;

const CONTAINS_WIDENING_TYPE: u32 = 1 << 16;

pub(super) struct Initialization {
    pub primitives: HashMap<u32, Rc<TypeCell>>,
    pub named: RefCell<HashMap<&'static str, Rc<TypeCell>>>,
    pub signatures: Vec<Signature>,
    pub index_infos: Vec<IndexInfo>,
}

impl Initialization {
    pub(super) fn named(&self, name: &'static str) -> Result<Rc<TypeCell>, Error> {
        self.named.borrow().get(name).cloned().ok_or_else(|| {
            Error::Unsupported(format!("uninitialized constructor field {name}").into())
        })
    }
}

// port: tsc/internal/checker/checker.go:NewChecker
pub(super) fn intrinsics(
    checker: &Checker,
    options: &BoundInputOptions,
) -> Result<Initialization, Error> {
    let graph = &checker.graph;
    let mut named = HashMap::new();
    let mut primitives = HashMap::new();
    let intrinsic = |flags, object_flags, name: &str| {
        graph.allocate_full(
            flags,
            object_flags,
            name.into(),
            None,
            None,
            None,
            false,
            Vec::new(),
            false,
            None,
        )
    };
    for (field, flags, name, object_flags) in [
        ("anyType", tf::ANY, "any", 0),
        ("autoType", tf::ANY, "any", of::NON_INFERRABLE_TYPE),
        ("wildcardType", tf::ANY, "any", 0),
        ("blockedStringType", tf::ANY, "any", 0),
        ("errorType", tf::ANY, "error", 0),
        ("unresolvedType", tf::ANY, "unresolved", 0),
        (
            "nonInferrableAnyType",
            tf::ANY,
            "any",
            CONTAINS_WIDENING_TYPE,
        ),
        ("intrinsicMarkerType", tf::ANY, "intrinsic", 0),
        ("unknownType", tf::UNKNOWN, "unknown", 0),
        ("undefinedType", tf::UNDEFINED, "undefined", 0),
    ] {
        let ty = intrinsic(flags, object_flags, name);
        primitives.entry(flags).or_insert_with(|| ty.clone());
        named.insert(field, ty);
    }
    let undefined = named["undefinedType"].clone();
    let widening = if options.strict_null_checks {
        undefined.clone()
    } else {
        intrinsic(tf::UNDEFINED, CONTAINS_WIDENING_TYPE, "undefined")
    };
    named.insert("undefinedWideningType", widening);
    let missing = intrinsic(tf::UNDEFINED, 0, "undefined");
    named.insert("missingType", missing.clone());
    named.insert(
        "undefinedOrMissingType",
        if options.exact_optional_property_types {
            missing
        } else {
            undefined.clone()
        },
    );
    named.insert("optionalType", intrinsic(tf::UNDEFINED, 0, "undefined"));
    let null = intrinsic(tf::NULL, 0, "null");
    primitives.insert(tf::NULL, null.clone());
    named.insert("nullType", null.clone());
    named.insert(
        "nullWideningType",
        if options.strict_null_checks {
            null.clone()
        } else {
            intrinsic(tf::NULL, CONTAINS_WIDENING_TYPE, "null")
        },
    );
    for (field, flags, name) in [
        ("stringType", tf::STRING, "string"),
        ("numberType", tf::NUMBER, "number"),
        ("bigintType", tf::BIG_INT, "bigint"),
    ] {
        let ty = intrinsic(flags, 0, name);
        primitives.insert(flags, ty.clone());
        named.insert(field, ty);
    }
    for (value, regular_field, fresh_field, name) in [
        (false, "regularFalseType", "falseType", "false"),
        (true, "regularTrueType", "trueType", "true"),
    ] {
        let regular = graph.intern_literal(tf::BOOLEAN_LITERAL, LiteralValue::Boolean(value), name);
        let fresh = graph.allocate_full(
            tf::BOOLEAN_LITERAL,
            0,
            name.into(),
            None,
            None,
            Some(LiteralValue::Boolean(value)),
            true,
            Vec::new(),
            false,
            None,
        );
        *regular.alternate.borrow_mut() = Rc::downgrade(&fresh);
        *fresh.alternate.borrow_mut() = Rc::downgrade(&regular);
        named.insert(regular_field, regular);
        named.insert(fresh_field, fresh);
    }
    let boolean = graph.union(&[
        named["regularFalseType"].clone(),
        named["regularTrueType"].clone(),
    ])?;
    named.insert("booleanType", boolean.clone());
    primitives.insert(tf::BOOLEAN, boolean);
    for (field, flags, name, object_flags) in [
        ("esSymbolType", tf::ES_SYMBOL, "symbol", 0),
        ("voidType", tf::VOID, "void", 0),
        ("neverType", tf::NEVER, "never", 0),
        (
            "silentNeverType",
            tf::NEVER,
            "never",
            of::NON_INFERRABLE_TYPE,
        ),
        ("implicitNeverType", tf::NEVER, "never", 0),
        ("unreachableNeverType", tf::NEVER, "never", 0),
        ("nonPrimitiveType", tf::NON_PRIMITIVE, "object", 0),
    ] {
        let ty = intrinsic(flags, object_flags, name);
        primitives.entry(flags).or_insert_with(|| ty.clone());
        named.insert(field, ty);
    }
    let string = named["stringType"].clone();
    let number = named["numberType"].clone();
    let bigint = named["bigintType"].clone();
    named.insert(
        "stringOrNumberType",
        graph.union(&[string.clone(), number.clone()])?,
    );
    named.insert(
        "stringNumberSymbolType",
        graph.union(&[
            string.clone(),
            number.clone(),
            named["esSymbolType"].clone(),
        ])?,
    );
    named.insert(
        "numberOrBigIntType",
        graph.union(&[number.clone(), bigint.clone()])?,
    );
    named.insert(
        "numericStringType",
        graph.template_literal(&[Vec::new(), Vec::new()], std::slice::from_ref(&number))?,
    );
    let mut template_constraint = vec![
        string.clone(),
        number.clone(),
        named["booleanType"].clone(),
        bigint,
    ];
    if options.strict_null_checks {
        template_constraint.extend([null, undefined]);
    }
    named.insert("templateConstraintType", graph.union(&template_constraint)?);
    named.insert("uniqueLiteralType", intrinsic(tf::NEVER, 0, "never"));
    for field in [
        "emptyObjectType",
        "emptyJsxObjectType",
        "emptyFreshJsxObjectType",
        "emptyTypeLiteralType",
        "unknownEmptyObjectType",
    ] {
        named.insert(field, empty_object(graph, 0));
    }
    let unknown_union = if options.strict_null_checks {
        graph.union(&[
            named["undefinedType"].clone(),
            named["nullType"].clone(),
            named["unknownEmptyObjectType"].clone(),
        ])?
    } else {
        named["unknownType"].clone()
    };
    named.insert("unknownUnionType", unknown_union);
    for field in [
        "emptyGenericType",
        "anyFunctionType",
        "noConstraintType",
        "circularConstraintType",
        "resolvingDefaultType",
    ] {
        named.insert(
            field,
            empty_object(
                graph,
                if field == "anyFunctionType" {
                    of::NON_INFERRABLE_TYPE
                } else {
                    0
                },
            ),
        );
    }
    let marker_super = graph.type_parameter("", None);
    let marker_sub = graph.type_parameter("", Some(&marker_super));
    let marker_other = graph.type_parameter("", None);
    checker.register_variance_markers(&marker_super, &marker_sub, &marker_other)?;
    named.insert("markerSuperType", marker_super);
    named.insert("markerSubType", marker_sub);
    named.insert("markerOtherType", marker_other);
    let marker_super_check = graph.type_parameter("", None);
    let marker_sub_check = graph.type_parameter("", Some(&marker_super_check));
    named.insert("markerSuperTypeForCheck", marker_super_check);
    named.insert("markerSubTypeForCheck", marker_sub_check);
    let any = named["anyType"].clone();
    let signatures = [
        any.clone(),
        named["errorType"].clone(),
        any.clone(),
        named["silentNeverType"].clone(),
    ]
    .iter()
    .map(|result| Signature {
        parameters: Vec::new(),
        parameter_names: Vec::new(),
        min_argument_count: 0,
        has_rest_parameter: false,
        type_parameters: 0,
        generic: None,
        this_type: None,
        return_type: Rc::downgrade(result).into(),
        bivariant_parameters: false,
        is_abstract: false,
        is_construct: false,
    })
    .collect();
    let index_infos = vec![
        IndexInfo {
            key: Rc::downgrade(&number),
            value: Rc::downgrade(&string),
            readonly: true,
        },
        IndexInfo {
            key: Rc::downgrade(&string),
            value: Rc::downgrade(&any),
            readonly: false,
        },
    ];
    named.insert("emptyStringType", graph.string_literal(b""));
    named.insert(
        "zeroType",
        graph.intern_literal(
            tf::NUMBER_LITERAL,
            LiteralValue::Number(0f64.to_bits()),
            "0",
        ),
    );
    named.insert(
        "zeroBigIntType",
        graph.intern_literal(
            tf::BIG_INT_LITERAL,
            LiteralValue::BigInt {
                negative: false,
                digits: b"0".to_vec(),
            },
            "0n",
        ),
    );
    let typeof_types = [
        b"bigint".as_slice(),
        b"boolean",
        b"function",
        b"number",
        b"object",
        b"string",
        b"symbol",
        b"undefined",
    ]
    .iter()
    .map(|name| graph.string_literal(name))
    .collect::<Vec<_>>();
    named.insert("typeofType", graph.union(&typeof_types)?);
    checker.register_intrinsics(&primitives.values().cloned().collect::<Vec<_>>());
    Ok(Initialization {
        primitives,
        named: RefCell::new(named),
        signatures,
        index_infos,
    })
}

fn empty_object(graph: &crate::Graph, additional_flags: u32) -> Rc<TypeCell> {
    let object = graph.allocate_full(
        tf::OBJECT,
        of::ANONYMOUS | additional_flags,
        "{}".into(),
        None,
        None,
        None,
        false,
        Vec::new(),
        false,
        None,
    );
    object
        .structure
        .set(Structure::default())
        .expect("new anonymous type has no members yet");
    object
}

impl Construction {
    // port: tsc/internal/checker/checker.go:Checker.initializeChecker
    pub(super) fn initialize_globals(self: &Rc<Self>) -> Result<(), Error> {
        let arguments = self.global_type("IArguments", 0)?;
        self.initialization
            .named
            .borrow_mut()
            .insert("argumentsType", arguments);
        let global_this = self.checker.graph.allocate_full(
            tf::OBJECT,
            of::ANONYMOUS,
            "typeof globalThis".into(),
            None,
            None,
            None,
            false,
            Vec::new(),
            false,
            Some(Box::new(|_, _| {
                missing("globalThis value member construction")
            })),
        );
        self.initialization
            .named
            .borrow_mut()
            .insert("globalThisType", global_this);
        for (name, arity) in [
            ("Array", 1),
            ("Object", 0),
            ("Function", 0),
            ("CallableFunction", 0),
            ("NewableFunction", 0),
            ("String", 0),
            ("Number", 0),
            ("Boolean", 0),
            ("RegExp", 0),
        ] {
            let ty = if matches!(name, "CallableFunction" | "NewableFunction")
                && !self.input.options().strict_bind_call_apply
            {
                self.initialization.named("Function")?
            } else {
                self.global_type(name, arity)?
            };
            self.initialization.named.borrow_mut().insert(name, ty);
        }
        let any = self.initialization.named("anyType")?;
        let auto = self.initialization.named("autoType")?;
        let any_array = self.global_reference("Array", std::slice::from_ref(&any))?;
        let mut auto_array = self.global_reference("Array", &[auto])?;
        if Rc::ptr_eq(&auto_array, &self.initialization.named("emptyObjectType")?) {
            auto_array = empty_object(&self.checker.graph, 0);
        }
        self.initialization
            .named
            .borrow_mut()
            .insert("anyArrayType", any_array);
        self.initialization
            .named
            .borrow_mut()
            .insert("autoArrayType", auto_array);
        let mut readonly = self.global_type("ReadonlyArray", 1)?;
        let readonly_name =
            if Rc::ptr_eq(&readonly, &self.initialization.named("emptyGenericType")?) {
                readonly = self.initialization.named("Array")?;
                "Array"
            } else {
                "ReadonlyArray"
            };
        self.initialization
            .named
            .borrow_mut()
            .insert("ReadonlyArray", readonly);
        let any_readonly = self.global_reference(readonly_name, &[any])?;
        self.initialization
            .named
            .borrow_mut()
            .insert("anyReadonlyArrayType", any_readonly);
        let this = self.global_type("ThisType", 1)?;
        self.initialization
            .named
            .borrow_mut()
            .insert("ThisType", this);
        for (name, readonly) in [("Array", false), ("ReadonlyArray", true)] {
            let target = self.initialization.named(name)?;
            if target.array_element().is_none() {
                if let Some(generic) = target.generic_target() {
                    let parameters = generic.parameters()?;
                    target.set_array_element(
                        parameters.first().ok_or(Error::ResolutionFailed)?,
                        readonly,
                    )?;
                }
            }
        }
        for (flags, name) in [
            (tf::STRING, "String"),
            (tf::NUMBER, "Number"),
            (tf::BOOLEAN, "Boolean"),
            (tf::NON_PRIMITIVE, "emptyObjectType"),
        ] {
            self.checker
                .register_apparent_type(flags, &self.initialization.named(name)?)?;
        }
        Ok(())
    }

    fn global_type(
        self: &Rc<Self>,
        name: &'static str,
        arity: usize,
    ) -> Result<Rc<TypeCell>, Error> {
        let Some(group) = self.input.resolve_global(name.as_bytes(), sf::TYPE)? else {
            return self.initialization.named(if arity == 0 {
                "emptyObjectType"
            } else {
                "emptyGenericType"
            });
        };
        let declarations = self.type_declaration_nodes(&group)?;
        let declaration = declarations
            .first()
            .copied()
            .ok_or(Error::ResolutionFailed)?;
        let node = self.input.node(declaration)?;
        if !matches!(
            node.kind().known(),
            Some(K::InterfaceDeclaration | K::ClassDeclaration)
        ) {
            return missing(&format!("global {name} must be a class or interface"));
        }
        if self.list(declaration, node.type_parameter_list())?.len() != arity {
            return missing(&format!("global {name} must have {arity} type parameters"));
        }
        self.declared(&group).map_err(|error| {
            Error::Unsupported(format!("initialize global {name}: {error:?}").into())
        })
    }

    fn global_reference(
        self: &Rc<Self>,
        name: &'static str,
        arguments: &[Rc<TypeCell>],
    ) -> Result<Rc<TypeCell>, Error> {
        let target = self.initialization.named(name)?;
        if Rc::ptr_eq(&target, &self.initialization.named("emptyGenericType")?) {
            return self.initialization.named("emptyObjectType");
        }
        let group = self
            .input
            .resolve_global(name.as_bytes(), sf::TYPE)?
            .ok_or(Error::ResolutionFailed)?;
        self.type_reference(&group, arguments, &Environment::default())
            .map_err(|error| {
                Error::Unsupported(format!("initialize global reference {name}: {error:?}").into())
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constructors_match_pinned_prefix_counts_and_marker_links() {
        // Independent pinned observations: data/s08/families-observations.json,
        // the two NewChecker prefix_counts records (strict / non-strict).
        for (strict, expected_types) in [(true, 62), (false, 63)] {
            let checker = Checker::new();
            let init = intrinsics(
                &checker,
                &BoundInputOptions {
                    strict_null_checks: strict,
                    ..Default::default()
                },
            )
            .unwrap();
            assert_eq!(checker.graph.len(), expected_types);
            assert_eq!(init.signatures.len(), 4);
            assert_eq!(init.index_infos.len(), 2);
            let undefined = init.named("undefinedType").unwrap();
            let widening = init.named("undefinedWideningType").unwrap();
            assert_eq!(Rc::ptr_eq(&undefined, &widening), strict);
            let sub = init.named("markerSubType").unwrap();
            let constraint = sub
                .type_parameter_shape()
                .unwrap()
                .constraint()
                .unwrap()
                .unwrap();
            assert!(Rc::ptr_eq(
                &constraint,
                &init.named("markerSuperType").unwrap()
            ));
            let regular = init.named("regularTrueType").unwrap();
            assert!(Rc::ptr_eq(
                &regular.alternate().unwrap(),
                &init.named("trueType").unwrap()
            ));
        }
    }
}
