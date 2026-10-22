use std::collections::hash_map::Entry;

use bon::Builder;
use qbice::{Decode, Encode, StableHash, storage::intern::Interned};
use rayc_arena::{Arena, ID};
use rayc_hash::FxHashMap;
use rayc_lexical::tree::RelativeSpan;
use rayc_qbice::TrackedEngine;
use rayc_semantic_element::parameter::ParameterID;
use rayc_source_file::ByteIndex;
use rayc_symbol::source_map::to_absolute_span;
use rayc_type::{
    subst::{MutSubstitutable, Subst, Substitutable},
    ty::Ty,
};

use crate::{
    typed_closure::ClosureParameterID,
    typed_function::{TypedFunctionID, TypedFunctionLocalID},
    typed_operation_handler::OperationHandlerParameterID,
    typed_variable::TypedVariableID,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
pub enum Source {
    Variable(TypedFunctionLocalID<TypedVariableID>),
    Parameter(TypedFunctionLocalID<ParameterID>),
    ClosureParameter(TypedFunctionLocalID<ClosureParameterID>),
    OperationHandlerParameter(TypedFunctionLocalID<OperationHandlerParameterID>),
}

impl Source {
    #[must_use]
    pub const fn function_id(self) -> TypedFunctionID {
        match self {
            Self::Variable(id) => id.function_id(),
            Self::Parameter(id) => id.function_id(),
            Self::ClosureParameter(id) => id.function_id(),
            Self::OperationHandlerParameter(id) => id.function_id(),
        }
    }
}

#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Builder,
)]
pub struct NameBinding {
    ty: Interned<Ty>,
    name: Interned<str>,
    source: Source,
    mutable: bool,
    span: RelativeSpan,
}

impl NameBinding {
    #[must_use]
    pub const fn span(&self) -> &RelativeSpan { &self.span }

    #[must_use]
    pub const fn ty(&self) -> &Interned<Ty> { &self.ty }

    #[must_use]
    pub const fn name(&self) -> &Interned<str> { &self.name }

    #[must_use]
    pub const fn is_mutable(&self) -> bool { self.mutable }

    #[must_use]
    pub const fn source(&self) -> &Source { &self.source }
}

impl MutSubstitutable for NameBinding {
    fn apply_mut_subst(&mut self, subst: &Subst, engine: &TrackedEngine) {
        self.ty.apply_in_place(subst, engine);
    }
}

pub type NameBindingID = ID<NameBinding>;

#[derive(Debug, Clone, PartialEq, Eq, StableHash, Encode, Decode, Default)]
pub struct NameBindingMap {
    name_bindings: Arena<NameBinding>,
    name_binding_groups: Arena<NameBindingGroup>,
}

impl NameBindingMap {
    #[must_use]
    pub fn insert_name_binding(&mut self, name_binding: NameBinding) -> ID<NameBinding> {
        self.name_bindings.insert(name_binding)
    }

    /// Creates an empty name binding group whose bindings can be referred to
    /// by name in `visibility`.
    #[must_use]
    pub fn new_name_binding_group(&mut self, visibility: RelativeSpan) -> NameBindingGroupID {
        self.name_binding_groups.insert(NameBindingGroup::new(visibility))
    }

    /// Inserts a name binding into a name binding group. Returns an error if
    /// the name binding with the same name already exists in the group.
    pub fn insert_name_binding_to_group(
        &mut self,
        group_id: NameBindingGroupID,
        name_binding_id: NameBindingID,
    ) -> Result<(), NameBindingID> {
        let group =
            self.name_binding_groups.get_mut(group_id).expect("NameBindingGroupID should be valid");

        let name = self.name_bindings[name_binding_id].name.clone();

        match group.name_bindings.entry(name) {
            Entry::Occupied(entry) => Err(*entry.get()),
            Entry::Vacant(entry) => {
                entry.insert(name_binding_id);
                Ok(())
            }
        }
    }

    #[must_use]
    pub fn insert_name_binding_group(
        &mut self,
        name_binding_group: NameBindingGroup,
    ) -> ID<NameBindingGroup> {
        self.name_binding_groups.insert(name_binding_group)
    }

    #[must_use]
    pub fn lookup_name(&self, group_id: NameBindingGroupID, name: &str) -> Option<NameBindingID> {
        let group =
            self.name_binding_groups.get(group_id).expect("NameBindingGroupID should be valid");

        group.name_bindings.get(name).copied()
    }

    #[must_use]
    pub fn get_name_binding(&self, id: NameBindingID) -> &NameBinding {
        self.name_bindings.get(id).expect("NameBindingID should be valid")
    }

    /// Iterates over the name bindings that can be referred to by name at
    /// `offset`, a byte index in the source file declaring them.
    ///
    /// A binding is visible after its declaration ends, up to and including
    /// the end of its scope. Of the visible bindings sharing a name, the one
    /// declared last shadows the others. Scopes nest, so that binding is also
    /// the one [`Self::lookup_name`] finds from the innermost scope.
    ///
    /// The iteration order is not stable.
    pub async fn visible_name_bindings(
        &self,
        engine: &TrackedEngine,
        offset: ByteIndex,
    ) -> impl Iterator<Item = (NameBindingID, &NameBinding)> {
        let mut visible = FxHashMap::<&str, (ByteIndex, NameBindingID)>::default();

        for group in self.name_binding_groups.items() {
            // the cursor at the end of a declaration is still inside it, while
            // the cursor at the end of the scope is not past it yet
            let span = engine.to_absolute_span(&group.visibility).await;
            if !span.contains_cursor(offset) {
                continue;
            }

            // keep the binding declared last of each name
            for (name, name_binding_id) in &group.name_bindings {
                visible
                    .entry(&**name)
                    .and_modify(|shadowed| {
                        if span.start > shadowed.0 {
                            *shadowed = (span.start, *name_binding_id);
                        }
                    })
                    .or_insert((span.start, *name_binding_id));
            }
        }

        visible
            .into_values()
            .map(|(_, name_binding_id)| (name_binding_id, self.get_name_binding(name_binding_id)))
    }

    /// Iterates over all name bindings and their IDs.
    ///
    /// The iteration order is not stable.
    #[must_use]
    pub fn name_bindings(&self) -> impl ExactSizeIterator<Item = (NameBindingID, &NameBinding)> {
        self.name_bindings.iter()
    }
}

impl MutSubstitutable for NameBindingMap {
    fn apply_mut_subst(&mut self, subst: &Subst, engine: &TrackedEngine) {
        for name_binding in self.name_bindings.items_mut() {
            name_binding.apply_mut_subst(subst, engine);
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, StableHash, Encode, Decode)]
pub struct NameBindingGroup {
    name_bindings: FxHashMap<Interned<str>, NameBindingID>,

    /// Where the bindings of this group can be referred to by name: from the
    /// end of their declaration to the end of the scope declaring them.
    visibility: RelativeSpan,
}

impl NameBindingGroup {
    #[must_use]
    fn new(visibility: RelativeSpan) -> Self {
        Self { name_bindings: FxHashMap::default(), visibility }
    }
}

pub type NameBindingGroupID = ID<NameBindingGroup>;
