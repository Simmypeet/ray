use std::{collections::hash_map::Entry, ops::Index};

use bon::Builder;
use qbice::{Decode, Encode, Identifiable, Query, StableHash, storage::intern::Interned};
use rayc_arena::{ID, OrderedArena};
use rayc_hash::FxHashMap;
use rayc_lexical::tree::RelativeSpan;
use rayc_symbol::{GlobalSymbolID, accessibility::Accessibility};
use rayc_type::ty::Ty;

/// A resolved field declared by a struct.
#[derive(Debug, Clone, PartialEq, Eq, StableHash, Encode, Decode, Identifiable, Builder)]
pub struct Field {
    name: Interned<str>,
    span: RelativeSpan,
    ty: Interned<Ty>,
    accessibility: Accessibility,
}

impl Field {
    #[must_use]
    pub const fn name(&self) -> &Interned<str> { &self.name }

    #[must_use]
    pub const fn span(&self) -> RelativeSpan { self.span }

    #[must_use]
    pub const fn ty(&self) -> &Interned<Ty> { &self.ty }

    /// Returns from where the field can be accessed.
    #[must_use]
    pub const fn accessibility(&self) -> Accessibility { self.accessibility }
}

pub type FieldID = ID<Field>;

/// The resolved fields of a struct in declaration order.
#[derive(Debug, Clone, PartialEq, Eq, StableHash, Encode, Decode, Default, Identifiable)]
pub struct StructBody {
    fields: OrderedArena<Field>,
    field_ids_by_name: FxHashMap<Interned<str>, FieldID>,
}

impl StructBody {
    #[must_use]
    pub fn new() -> Self { Self::default() }

    #[must_use]
    pub fn len(&self) -> usize { self.fields.len() }

    #[must_use]
    pub fn is_empty(&self) -> bool { self.fields.is_empty() }

    #[must_use]
    pub fn iter(&self) -> impl ExactSizeIterator<Item = (FieldID, &Field)> { self.fields.iter() }

    #[must_use]
    pub fn get_by_name(&self, name: &str) -> Option<(FieldID, &Field)> {
        let id = self.field_ids_by_name.get(name).copied()?;
        Some((id, &self[id]))
    }

    #[allow(clippy::result_large_err)]
    pub fn insert(&mut self, field: Field) -> Result<FieldID, (Field, FieldID)> {
        match self.field_ids_by_name.entry(field.name.clone()) {
            Entry::Occupied(entry) => Err((field, *entry.get())),
            Entry::Vacant(entry) => {
                let id = self.fields.insert(field);
                entry.insert(id);
                Ok(id)
            }
        }
    }
}

impl Index<FieldID> for StructBody {
    type Output = Field;

    fn index(&self, index: FieldID) -> &Self::Output {
        self.fields.get(index).expect("struct field ID should be valid")
    }
}

/// Retrieves the resolved body of a struct symbol.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Query,
)]
#[value(Interned<StructBody>)]
#[extend(by_val, name = get_struct_body)]
pub struct Key {
    pub symbol_id: GlobalSymbolID,
}
