//! Checker-local type mappings. Links are IDs, so mappers cannot retain the
//! checker or a mutable borrow of it. Composite mapping may instantiate a type;
//! merged mapping only applies the two mappings in order (`mapper.go`).

use crate::{CheckerState, Error, MapperId, TypeId, TypeList};

#[derive(Clone, Debug)]
pub(crate) enum Mapper {
    DeferredArguments {
        node: tsr_arena::NodeId,
        sources: TypeList,
    },
    Inference {
        context: crate::InferenceId,
        fixing: bool,
    },
    UniqueLiteral,
    Permissive,
    Restrictive,
    ReportReliability {
        flag: u32,
    },
    Simple {
        source: TypeId,
        target: TypeId,
    },
    Array {
        sources: TypeList,
        targets: TypeList,
    },
    ArrayToSingle {
        sources: TypeList,
        target: TypeId,
    },
    Merged {
        first: MapperId,
        second: MapperId,
    },
    Composite {
        first: MapperId,
        second: MapperId,
    },
}

impl CheckerState {
    // port: tsc/internal/checker/utilities.go:compareTypeMappers
    pub(crate) fn compare_type_mappers(
        &self,
        first: Option<MapperId>,
        second: Option<MapperId>,
    ) -> Result<std::cmp::Ordering, Error> {
        use std::cmp::Ordering;
        if first == second {
            return Ok(Ordering::Equal);
        }
        let (first, second) = match (first, second) {
            (None, _) => return Ok(Ordering::Greater),
            (_, None) => return Ok(Ordering::Less),
            (Some(first), Some(second)) => (self.mapper(first)?, self.mapper(second)?),
        };
        // Composite and deferred/function mappers have KindUnknown upstream.
        // The source comparator deliberately does not inspect their closures.
        let kind = |mapper: &Mapper| match mapper {
            Mapper::Composite { .. }
            | Mapper::UniqueLiteral
            | Mapper::ReportReliability { .. }
            | Mapper::Permissive
            | Mapper::Restrictive
            | Mapper::Inference { .. }
            | Mapper::ArrayToSingle { .. }
            | Mapper::DeferredArguments { .. } => 0,
            Mapper::Simple { .. } => 1,
            Mapper::Array { .. } => 2,
            Mapper::Merged { .. } => 3,
        };
        let order = kind(first).cmp(&kind(second));
        if order != Ordering::Equal {
            return Ok(order);
        }
        match (first, second) {
            (
                Mapper::Simple {
                    source: a,
                    target: x,
                },
                Mapper::Simple {
                    source: b,
                    target: y,
                },
            ) => {
                let head = self.compare_types(*a, *b)?;
                if head == Ordering::Equal {
                    self.compare_types(*x, *y)
                } else {
                    Ok(head)
                }
            }
            (
                Mapper::Array {
                    sources: a,
                    targets: x,
                },
                Mapper::Array {
                    sources: b,
                    targets: y,
                },
            ) => {
                let head = self.compare_type_lists(a, b)?;
                if head == Ordering::Equal {
                    self.compare_type_lists(x, y)
                } else {
                    Ok(head)
                }
            }
            (
                Mapper::Merged {
                    first: a,
                    second: x,
                },
                Mapper::Merged {
                    first: b,
                    second: y,
                },
            ) => {
                let head = self.compare_type_mappers(Some(*a), Some(*b))?;
                if head == Ordering::Equal {
                    self.compare_type_mappers(Some(*x), Some(*y))
                } else {
                    Ok(head)
                }
            }
            _ => Ok(Ordering::Equal),
        }
    }

    pub(crate) fn alloc_mapper(&mut self, mapper: Mapper) -> Result<MapperId, Error> {
        let id = MapperId::next(0, self.instantiation.mappers.len())?;
        self.instantiation.mappers.push(mapper);
        Ok(id)
    }

    pub(crate) fn unique_literal_mapper(&mut self) -> Result<MapperId, Error> {
        if let Some(mapper) = self.instantiation.unique_literal_mapper {
            return Ok(mapper);
        }
        let mapper = self.alloc_mapper(Mapper::UniqueLiteral)?;
        self.instantiation.unique_literal_mapper = Some(mapper);
        Ok(mapper)
    }

    pub(crate) fn report_unreliable_markers(
        &mut self,
        ty: TypeId,
        unmeasurable: bool,
    ) -> Result<TypeId, Error> {
        let index = usize::from(unmeasurable);
        let mapper = if let Some(mapper) = self.instantiation.reliability_mappers[index] {
            mapper
        } else {
            let flag = if unmeasurable {
                crate::variance::REPORTS_UNMEASURABLE
            } else {
                crate::variance::REPORTS_UNRELIABLE
            };
            let mapper = self.alloc_mapper(Mapper::ReportReliability { flag })?;
            self.instantiation.reliability_mappers[index] = Some(mapper);
            mapper
        };
        self.instantiate_type(ty, Some(mapper))
    }

    pub(crate) fn mapper(&self, id: MapperId) -> Result<&Mapper, Error> {
        id.index(0)
            .and_then(|index| self.instantiation.mappers.get(index))
            .ok_or(Error::Arena(tsr_arena::Error::InvalidSlot))
    }

    // port: tsc/internal/checker/mapper.go:newTypeMapper
    pub(crate) fn new_type_mapper(
        &mut self,
        sources: &[TypeId],
        targets: &[TypeId],
    ) -> Result<MapperId, Error> {
        if sources.len() > targets.len() {
            return Err(Error::MissingLink("type mapper target arity"));
        }
        self.alloc_mapper(if sources.len() == 1 {
            Mapper::Simple {
                source: sources[0],
                target: targets[0],
            }
        } else {
            Mapper::Array {
                sources: sources.into(),
                targets: targets.into(),
            }
        })
    }

    // port: tsc/internal/checker/mapper.go:newArrayToSingleTypeMapper
    pub(crate) fn new_array_to_single_type_mapper(
        &mut self,
        sources: &[TypeId],
        target: TypeId,
    ) -> Result<MapperId, Error> {
        self.alloc_mapper(Mapper::ArrayToSingle {
            sources: sources.into(),
            target,
        })
    }

    // port: tsc/internal/checker/mapper.go:Checker.combineTypeMappers
    pub(crate) fn combine_type_mappers(
        &mut self,
        first: Option<MapperId>,
        second: MapperId,
    ) -> Result<MapperId, Error> {
        match first {
            Some(first) => self.alloc_mapper(Mapper::Composite { first, second }),
            None => Ok(second),
        }
    }

    // port: tsc/internal/checker/mapper.go:appendTypeMapping
    pub(crate) fn append_type_mapping(
        &mut self,
        mapper: Option<MapperId>,
        source: TypeId,
        target: TypeId,
    ) -> Result<MapperId, Error> {
        let second = self.alloc_mapper(Mapper::Simple { source, target })?;
        match mapper {
            Some(first) => self.alloc_mapper(Mapper::Merged { first, second }),
            None => Ok(second),
        }
    }

    // port: tsc/internal/checker/mapper.go:prependTypeMapping
    pub(crate) fn prepend_type_mapping(
        &mut self,
        source: TypeId,
        target: TypeId,
        mapper: Option<MapperId>,
    ) -> Result<MapperId, Error> {
        let first = self.alloc_mapper(Mapper::Simple { source, target })?;
        match mapper {
            Some(second) => self.alloc_mapper(Mapper::Merged { first, second }),
            None => Ok(first),
        }
    }

    pub(crate) fn mapper_maps_this_only(&self, id: MapperId) -> Result<bool, Error> {
        let source = match self.mapper(id)? {
            Mapper::Simple { source, .. } => Some(*source),
            Mapper::Array { sources, .. }
            | Mapper::ArrayToSingle { sources, .. }
            | Mapper::DeferredArguments { sources, .. }
                if sources.len() == 1 =>
            {
                Some(sources[0])
            }
            _ => None,
        };
        match source {
            Some(source) if self.types.flags(source)? & crate::type_flags::TYPE_PARAMETER != 0 => {
                Ok(self.types.type_parameter(source)?.is_this_type)
            }
            _ => Ok(false),
        }
    }

    // port: tsc/internal/checker/mapper.go:SimpleTypeMapper.Map
    // port: tsc/internal/checker/mapper.go:ArrayTypeMapper.Map
    // port: tsc/internal/checker/mapper.go:MergedTypeMapper.Map
    // port: tsc/internal/checker/mapper.go:CompositeTypeMapper.Map
    pub(crate) fn map_type_parameter(
        &mut self,
        ty: TypeId,
        mapper: MapperId,
    ) -> Result<TypeId, Error> {
        // Only ID fields are copied for recursive variants. Array matching
        // borrows its stored lists and does not clone them on a cache read.
        match *self.mapper(mapper)? {
            Mapper::DeferredArguments { node, ref sources } => {
                let Some(index) = sources.iter().position(|&s| s == ty) else {
                    return Ok(ty);
                };
                let sources = sources.clone();
                let nodes = self.source_list(node, self.node(node)?.type_argument_list())?;
                if let Some(&node) = nodes.get(index) {
                    self.get_type_from_type_node(node)
                } else {
                    self.effective_type_arguments(node, &sources)
                        .map(|args| args[index])
                }
            }
            Mapper::Inference { context, fixing } => self.map_inference_type(context, ty, fixing),
            Mapper::Permissive => Ok(
                if self.types.flags(ty)? & crate::type_flags::TYPE_PARAMETER != 0 {
                    self.builtins.wildcard_type
                } else {
                    ty
                },
            ),
            Mapper::Restrictive => {
                if self.types.flags(ty)? & crate::type_flags::TYPE_PARAMETER != 0 {
                    self.restrictive_type_parameter(ty)
                } else {
                    Ok(ty)
                }
            }
            Mapper::ReportReliability { flag } => {
                self.report_variance_marker(ty, flag);
                Ok(ty)
            }
            Mapper::UniqueLiteral => Ok(
                if self.types.flags(ty)? & crate::type_flags::TYPE_PARAMETER != 0 {
                    self.builtins.unique_literal_type
                } else {
                    ty
                },
            ),
            Mapper::Simple { source, target } => Ok(if ty == source { target } else { ty }),
            Mapper::ArrayToSingle {
                ref sources,
                target,
            } => Ok(if sources.contains(&ty) { target } else { ty }),
            Mapper::Array {
                ref sources,
                ref targets,
            } => Ok(sources
                .iter()
                .position(|&source| source == ty)
                .map_or(ty, |index| targets[index])),
            Mapper::Merged { first, second } => {
                let ty = self.map_type_parameter(ty, first)?;
                self.map_type_parameter(ty, second)
            }
            Mapper::Composite { first, second } => {
                let mapped = self.map_type_parameter(ty, first)?;
                if mapped == ty {
                    self.map_type_parameter(ty, second)
                } else {
                    self.instantiate_type(mapped, Some(second))
                }
            }
        }
    }
}
