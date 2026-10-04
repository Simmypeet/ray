//! Discovery and ordering of the aggregate types used by generated code.

use qbice::storage::intern::Interned;
use rayc_hash::{FxHashMap, FxHashSet};
use rayc_mono_ir::{
    MonoEffectInstance,
    ty::{
        AggregateType, FunctionSignature, HandlerLayout, MonoType, ReturnType, build_handler_layout,
    },
};
use rayc_qbice::TrackedEngine;

use crate::c::name::AggregateName;

/// Every aggregate type reachable from generated code, closed under the types
/// of their members.
#[derive(Debug, Default)]
pub(crate) struct AggregateRegistry {
    /// Each discovered aggregate with its precomputed C name.
    names: FxHashMap<AggregateType, AggregateName>,
    /// Discovered aggregates whose member types have not been visited yet.
    pending: Vec<AggregateType>,
    handler_layouts: FxHashMap<MonoEffectInstance, Interned<HandlerLayout>>,
}

impl AggregateRegistry {
    /// Discovers every aggregate that `ty` mentions.
    pub(crate) fn visit_type(&mut self, ty: &MonoType) {
        match ty {
            MonoType::Bool
            | MonoType::Int8
            | MonoType::Int16
            | MonoType::Int32
            | MonoType::Int64
            | MonoType::Isize
            | MonoType::Uint8
            | MonoType::Uint16
            | MonoType::Uint32
            | MonoType::Uint64
            | MonoType::Usize
            | MonoType::Float32
            | MonoType::CInt
            | MonoType::CStr
            | MonoType::OpaquePointer(_) => {}
            MonoType::Pointer(pointer) => self.visit_type(pointer.pointee()),
            // Check before cloning: most visits find an already-known type.
            MonoType::Aggregate(aggregate) => {
                if !self.names.contains_key(aggregate) {
                    self.insert(aggregate.clone());
                }
            }
            MonoType::FunctionPointer(signature) => self.visit_signature(signature),
        }
    }

    /// Discovers every aggregate that `signature` mentions.
    pub(crate) fn visit_signature(&mut self, signature: &FunctionSignature) {
        for parameter in signature.parameter_types() {
            self.visit_type(parameter);
        }
        match signature.return_type() {
            ReturnType::Void => {}
            ReturnType::Value(ty) => self.visit_type(ty),
        }
    }

    /// Discovers `aggregate` itself.
    pub(crate) fn insert(&mut self, aggregate: AggregateType) {
        if !self.names.contains_key(&aggregate) {
            self.names.insert(aggregate.clone(), AggregateName::of(&aggregate));
            self.pending.push(aggregate);
        }
    }

    /// Visits the members of every pending aggregate, resolving effect-handler
    /// layouts, until the discovered set is closed.
    pub(crate) async fn resolve_pending(&mut self, engine: &TrackedEngine) {
        while let Some(aggregate) = self.pending.pop() {
            match &aggregate {
                AggregateType::EffectHandler(handler) => {
                    let instance = handler.mono_effect_instance();
                    let layout = engine.build_handler_layout(instance.clone()).await;
                    for operation in layout.operations() {
                        self.visit_signature(operation.signature());
                    }
                    self.handler_layouts.insert(instance.clone(), layout);
                }
                AggregateType::Tuple(tuple) => {
                    tuple.fields().iter().for_each(|field| self.visit_type(field));
                }
                AggregateType::Environment(environment) => {
                    environment.captures().iter().for_each(|capture| self.visit_type(capture));
                }
                AggregateType::Struct(st) => {
                    st.fields().values().for_each(|field| self.visit_type(field));
                }
            }
        }
    }

    /// The operation slots of a resolved effect handler.
    pub(crate) fn handler_layout(&self, instance: &MonoEffectInstance) -> &HandlerLayout {
        self.handler_layouts
            .get(instance)
            .expect("effect handler should have been resolved before printing")
    }

    /// The operation slots of `aggregate` if it is an effect handler.
    fn layout_of(&self, aggregate: &AggregateType) -> Option<&HandlerLayout> {
        match aggregate {
            AggregateType::EffectHandler(handler) => {
                Some(self.handler_layout(handler.mono_effect_instance()))
            }
            AggregateType::Tuple(_) | AggregateType::Environment(_) | AggregateType::Struct(_) => {
                None
            }
        }
    }

    /// Every aggregate with its handler layout, ordered so that each one
    /// follows the aggregates it contains by value, as C requires complete
    /// member types. Ties are broken by name for determinism.
    pub(crate) fn in_definition_order(
        &self,
    ) -> impl Iterator<Item = (AggregateName, &AggregateType, Option<&HandlerLayout>)> {
        assert!(self.pending.is_empty(), "aggregates should be resolved before being ordered");

        let mut roots =
            self.names.iter().map(|(aggregate, name)| (*name, aggregate)).collect::<Vec<_>>();
        roots.sort_unstable_by_key(|(name, _)| *name);

        let mut ordering = TopologicalOrdering::default();
        for (name, aggregate) in roots {
            ordering.visit(self, name, aggregate);
        }
        ordering
            .ordered
            .into_iter()
            .map(|(name, aggregate)| (name, aggregate, self.layout_of(aggregate)))
    }

    /// The aggregates that `aggregate` stores by value, sorted by name.
    fn by_value_dependencies<'a>(
        &self,
        aggregate: &'a AggregateType,
    ) -> Vec<(AggregateName, &'a AggregateType)> {
        match aggregate {
            // Handler members are pointers, which need no complete type.
            AggregateType::EffectHandler(_) => Vec::new(),
            AggregateType::Tuple(tuple) => self.named_aggregates(tuple.fields()),
            AggregateType::Environment(environment) => {
                self.named_aggregates(environment.captures())
            }
            AggregateType::Struct(st) => self.named_aggregates(st.fields().values()),
        }
    }

    fn named_aggregates<'a>(
        &self,
        types: impl IntoIterator<Item = &'a Interned<MonoType>>,
    ) -> Vec<(AggregateName, &'a AggregateType)> {
        let mut dependencies = types
            .into_iter()
            .filter_map(|ty| by_value_aggregate(ty))
            .map(|aggregate| {
                let name = *self
                    .names
                    .get(aggregate)
                    .expect("aggregate dependency should have passed through the registry");
                (name, aggregate)
            })
            .collect::<Vec<_>>();
        dependencies.sort_unstable_by_key(|(name, _)| *name);
        dependencies
    }
}

/// The aggregate a value of type `ty` contains inline, if any.
const fn by_value_aggregate(ty: &MonoType) -> Option<&AggregateType> {
    match ty {
        MonoType::Aggregate(aggregate) => Some(aggregate),
        MonoType::Bool
        | MonoType::Int8
        | MonoType::Int16
        | MonoType::Int32
        | MonoType::Int64
        | MonoType::Isize
        | MonoType::Uint8
        | MonoType::Uint16
        | MonoType::Uint32
        | MonoType::Uint64
        | MonoType::Usize
        | MonoType::Float32
        | MonoType::CInt
        | MonoType::CStr
        | MonoType::OpaquePointer(_)
        | MonoType::Pointer(_)
        | MonoType::FunctionPointer(_) => None,
    }
}

/// Depth-first post-order over by-value containment.
#[derive(Debug, Default)]
struct TopologicalOrdering<'a> {
    visiting: FxHashSet<&'a AggregateType>,
    visited: FxHashSet<&'a AggregateType>,
    ordered: Vec<(AggregateName, &'a AggregateType)>,
}

impl<'a> TopologicalOrdering<'a> {
    fn visit(
        &mut self,
        registry: &'a AggregateRegistry,
        name: AggregateName,
        aggregate: &'a AggregateType,
    ) {
        if self.visited.contains(aggregate) {
            return;
        }
        assert!(self.visiting.insert(aggregate), "aggregate types cannot contain a cycle by value");

        for (dependency_name, dependency) in registry.by_value_dependencies(aggregate) {
            self.visit(registry, dependency_name, dependency);
        }

        self.visiting.remove(aggregate);
        self.visited.insert(aggregate);
        self.ordered.push((name, aggregate));
    }
}
