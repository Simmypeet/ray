use std::{
    collections::{VecDeque, hash_map::Entry},
    fmt::{self, Display, Write},
};

use qbice::{Decode, Encode, Identifiable, StableHash, storage::intern::Interned};
use rayc_hash::FxHashMap;
use rayc_qbice::TrackedEngine;
use rayc_symbol::{GlobalSymbolID, name::get_name};

use crate::{
    poly_var::{GlobalPolyVarID, Key as PolyVarKey, PolyVarMap, get_poly_var_map},
    reduce::Reduce,
    subst::{Subst, Substitutable},
    ty::{
        application::{Application, Constant, View as ApplicationView},
        effect_row::EffectRow,
        inference::{GenInfer, Inference},
    },
};

pub mod application;
pub mod args;
pub mod effect_row;
pub mod inference;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
pub enum Primitive {
    Int32,
    Float32,
    Bool,
    CInt,
    CStr,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
pub enum Mutability {
    Immutable,
    Mutable,
}

impl Mutability {
    #[must_use]
    pub const fn constness(&self) -> bool {
        match self {
            Self::Immutable => true,
            Self::Mutable => false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
pub enum TyKind {
    Star,
    EffectRow,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
pub enum InferenceConstraint {
    Any,
    Numeric,
    EqualityComparable,
}

impl InferenceConstraint {
    #[must_use]
    #[allow(clippy::match_same_arms)]
    pub const fn meet(&self, other: &Self) -> Option<Self> {
        match (self, other) {
            (Self::Any, Self::Any) => Some(Self::Any),
            (Self::Any, Self::Numeric) | (Self::Numeric, Self::Any) => Some(Self::Numeric),
            (Self::Any, Self::EqualityComparable) | (Self::EqualityComparable, Self::Any) => {
                Some(Self::EqualityComparable)
            }
            (Self::Numeric, Self::Numeric) => Some(Self::Numeric),
            (Self::Numeric, Self::EqualityComparable)
            | (Self::EqualityComparable, Self::Numeric) => Some(Self::Numeric),
            (Self::EqualityComparable, Self::EqualityComparable) => Some(Self::EqualityComparable),
        }
    }
}

#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Identifiable,
)]
pub enum Ty {
    Application(Application),
    Inference(Inference),
    PolyVar(GlobalPolyVarID),
    EffectRow(EffectRow),
}

impl Ty {
    pub async fn kind_of(&self, engine: &TrackedEngine) -> TyKind {
        match self {
            Self::Application(application) => application.kind_of(),
            Self::Inference(inference) => inference.kind(),
            Self::PolyVar(poly_var) => {
                let poly_var_map = engine.get_poly_var_map(poly_var.parent_id()).await;
                poly_var_map.kind_of(poly_var.id())
            }
            Self::EffectRow(_) => TyKind::EffectRow,
        }
    }

    /// Iterates over an interned type and all its recursively nested type
    /// arguments in breadth-first order.
    pub fn interned_recursive_iter(
        root: &Interned<Self>,
    ) -> impl Iterator<Item = &'_ Interned<Self>> {
        let mut pending = VecDeque::from([root]);

        std::iter::from_fn(move || {
            let ty = pending.pop_front()?;
            match &**ty {
                Self::Application(application) => pending.extend(application.interned_iter()),
                Self::EffectRow(row) => pending.extend(row.interned_iter()),
                Self::Inference(_) | Self::PolyVar(_) => {}
            }
            Some(ty)
        })
    }

    pub fn recursive_iter(&self) -> impl Iterator<Item = &Self> {
        let mut pending = VecDeque::from([self]);

        std::iter::from_fn(move || {
            let ty = pending.pop_front()?;
            match ty {
                Self::Application(application) => {
                    pending.extend(application.iter());
                }
                Self::EffectRow(row) => pending.extend(row.iter()),
                Self::Inference(_) | Self::PolyVar(_) => {}
            }
            Some(ty)
        })
    }

    #[must_use]
    pub fn has_inference_variable(&self, ty: &Inference) -> bool {
        match self {
            Self::Application(application) => application.has_inference_variable(ty),
            Self::Inference(ty_inference) => ty_inference == ty,
            Self::EffectRow(row) => row.has_inference_variable(ty),
            Self::PolyVar(_) => false,
        }
    }
}

impl Substitutable for Interned<Ty> {
    fn apply_subst(&self, subst: &Subst, engine: &TrackedEngine) -> Option<Self>
    where
        Self: Sized,
    {
        match &**self {
            Ty::Application(application) => application
                .apply_subst(subst, engine)
                .map(|application| engine.intern(Ty::Application(application))),

            Ty::Inference(ty_inference) => subst.get(ty_inference).cloned(),
            Ty::PolyVar(poly) => subst.get(poly).cloned(),
            Ty::EffectRow(row) => {
                row.apply_subst(subst, engine).map(|new_row| engine.intern(Ty::EffectRow(new_row)))
            }
        }
    }
}

impl Reduce for Interned<Ty> {
    fn reduce(&self, engine: &TrackedEngine) -> Option<Self> {
        match &**self {
            Ty::Application(application) => application
                .reduce(engine)
                .map(|application| engine.intern(Ty::Application(application))),
            Ty::Inference(_) | Ty::PolyVar(_) => None,
            Ty::EffectRow(row) => {
                if row.labels().len() == 0
                    && let Some(tail) = row.tail()
                {
                    return Some(tail.clone());
                }

                row.reduce(engine).map(|row| engine.intern(Ty::EffectRow(row)))
            }
        }
    }
}

impl Ty {
    #[must_use]
    pub fn new_primitive(primitive: Primitive, engine: &TrackedEngine) -> Interned<Self> {
        engine.intern(Self::Application(Application::new(
            Constant::Primitive(primitive),
            engine.intern_unsized([]),
        )))
    }

    #[must_use]
    pub fn new_tuple(args: Interned<[Interned<Self>]>, engine: &TrackedEngine) -> Interned<Self> {
        engine.intern(Self::Application(Application::new(Constant::Tuple, args)))
    }

    /// Creates a lambda type with its effect row stored after its return type.
    #[must_use]
    pub fn new_lambda(
        parameter_types: impl IntoIterator<Item = Interned<Self>>,
        return_type: Interned<Self>,
        effect_row: Interned<Self>,
        engine: &TrackedEngine,
    ) -> Interned<Self> {
        let args = parameter_types.into_iter().chain([return_type, effect_row]).collect::<Vec<_>>();
        engine.intern(Self::Application(Application::new(
            Constant::Lambda,
            engine.intern_unsized(args),
        )))
    }

    #[must_use]
    pub fn new_pointer(
        arg: Interned<Self>,
        mutability: Mutability,
        engine: &TrackedEngine,
    ) -> Interned<Self> {
        engine.intern(Self::Application(Application::new(
            Constant::Pointer(mutability),
            engine.intern_unsized([arg]),
        )))
    }

    #[must_use]
    pub fn new_error(kind: TyKind, engine: &TrackedEngine) -> Interned<Self> {
        engine.intern(Self::Application(Application::new(
            Constant::Error(kind),
            engine.intern_unsized([]),
        )))
    }

    #[must_use]
    pub fn new_star_error(engine: &TrackedEngine) -> Interned<Self> {
        Self::new_error(TyKind::Star, engine)
    }

    #[must_use]
    pub fn new_unit(engine: &TrackedEngine) -> Interned<Self> {
        engine
            .intern(Self::Application(Application::new(Constant::Tuple, engine.intern_unsized([]))))
    }

    #[must_use]
    pub fn new_poly_var(id: GlobalPolyVarID, engine: &TrackedEngine) -> Interned<Self> {
        engine.intern(Self::PolyVar(id))
    }

    #[must_use]
    pub fn new_effect_row(
        labels: impl IntoIterator<Item = Interned<effect_row::EffectLabel>>,
        tail: Option<Interned<Self>>,
        engine: &TrackedEngine,
    ) -> Interned<Self> {
        engine.intern(Self::EffectRow(EffectRow::new(labels, tail, engine)))
    }
}

impl Ty {
    pub async fn display<'x>(&'x self, engine: &TrackedEngine) -> TyDisplay<'x> {
        let mut poly_var_maps = FxHashMap::default();
        let mut effect_names = FxHashMap::default();
        self.collect_display_context(engine, &mut poly_var_maps, &mut effect_names).await;

        TyDisplay { ty: self, poly_var_maps, effect_names }
    }

    async fn collect_display_context(
        &self,
        engine: &TrackedEngine,
        poly_var_maps: &mut FxHashMap<GlobalSymbolID, Interned<PolyVarMap>>,
        effect_names: &mut FxHashMap<GlobalSymbolID, Interned<str>>,
    ) {
        for ty in self.recursive_iter() {
            match ty {
                Self::Application(_) | Self::Inference(_) => {}
                Self::PolyVar(poly_var) => {
                    let symbol_id = poly_var.parent_id();
                    if let Entry::Vacant(entry) = poly_var_maps.entry(symbol_id) {
                        let poly_var_map = engine.query(&PolyVarKey { symbol_id }).await;
                        entry.insert(poly_var_map);
                    }
                }
                Self::EffectRow(row) => {
                    for label in row.labels() {
                        if let Entry::Vacant(entry) = effect_names.entry(label.effect_symbol_id()) {
                            entry.insert(engine.get_name(label.effect_symbol_id()).await);
                        }
                    }
                }
            }
        }
    }
}

#[derive(Debug)]
pub struct TyDisplay<'x> {
    ty: &'x Ty,
    poly_var_maps: FxHashMap<GlobalSymbolID, Interned<PolyVarMap>>,
    effect_names: FxHashMap<GlobalSymbolID, Interned<str>>,
}

impl TyDisplay<'_> {
    fn fmt_ty(&self, ty: &Ty, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match ty {
            Ty::Application(ty_application) => match ty_application.view() {
                ApplicationView::Primitive(primitive) => match primitive {
                    Primitive::Int32 => write!(f, "int32"),
                    Primitive::Float32 => write!(f, "float32"),
                    Primitive::Bool => write!(f, "bool"),
                    Primitive::CInt => write!(f, "c_int"),
                    Primitive::CStr => write!(f, "cstr"),
                },
                ApplicationView::Tuple(tuple) => {
                    f.write_char('(')?;

                    for (i, arg) in tuple.args().iter().enumerate() {
                        if i > 0 {
                            f.write_str(", ")?;
                        }
                        self.fmt_ty(arg, f)?;
                    }

                    f.write_char(')')
                }
                ApplicationView::Lambda(lambda) => {
                    f.write_str("def(")?;
                    for (index, parameter) in lambda.parameter_types().iter().enumerate() {
                        if index > 0 {
                            f.write_str(", ")?;
                        }
                        self.fmt_ty(parameter, f)?;
                    }
                    f.write_str(") -> ")?;
                    self.fmt_ty(lambda.return_type(), f)?;
                    f.write_str(" \\ ")?;
                    self.fmt_ty(lambda.effect_row(), f)
                }
                ApplicationView::Pointer(pointer) => {
                    f.write_char('*')?;
                    if pointer.mutability() == Mutability::Mutable {
                        f.write_str("mut ")?;
                    }
                    self.fmt_ty(pointer.pointee(), f)
                }
                ApplicationView::Error => write!(f, "<error>"),
            },

            Ty::Inference(inference) => match inference.constraint() {
                InferenceConstraint::Any => write!(f, "{{any}}"),
                InferenceConstraint::Numeric => {
                    write!(f, "{{numeric}}")
                }
                InferenceConstraint::EqualityComparable => {
                    write!(f, "{{equality comparable}}")
                }
            },

            Ty::PolyVar(poly_var) => {
                let poly_var_map = self
                    .poly_var_maps
                    .get(&poly_var.parent_id())
                    .expect("should've been collected earlier");

                write!(f, "{}", poly_var_map.name_of(poly_var.id()))
            }

            Ty::EffectRow(row) => {
                f.write_char('{')?;

                for (index, label) in row.labels().enumerate() {
                    if index > 0 {
                        f.write_str(", ")?;
                    }

                    let name = self
                        .effect_names
                        .get(&label.effect_symbol_id())
                        .expect("should've been collected earlier");
                    f.write_str(name)?;

                    if label.has_arguments() {
                        f.write_char('[')?;
                        for (argument_index, argument) in label.arguments().iter().enumerate() {
                            if argument_index > 0 {
                                f.write_str(", ")?;
                            }
                            self.fmt_ty(argument, f)?;
                        }
                        f.write_char(']')?;
                    }
                }

                if let Some(tail) = row.tail() {
                    if row.labels().len() > 0 {
                        f.write_str(" | ")?;
                    } else {
                        f.write_str("| ")?;
                    }
                    self.fmt_ty(tail, f)?;
                }

                f.write_char('}')
            }
        }
    }
}

impl Display for TyDisplay<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { self.fmt_ty(self.ty, f) }
}

impl Ty {
    #[must_use]
    pub fn unwrap_as_application_view(&self) -> ApplicationView<'_> {
        let Self::Application(ty_application) = self else {
            panic!("Expected Ty::Application, found {self:?}");
        };

        ty_application.view()
    }

    #[must_use]
    pub fn open_closed_row(
        ty: &Interned<Self>,
        infer_gen: &mut impl GenInfer,
        engine: &TrackedEngine,
    ) -> Option<Interned<Self>> {
        if let Self::EffectRow(row) = ty.as_ref() {
            row.open_closed_row(infer_gen, engine).map(|x| engine.intern(Self::EffectRow(x)))
        } else {
            None
        }
    }

    #[must_use]
    pub const fn as_inference(&self) -> Option<&Inference> {
        if let Self::Inference(inference) = self { Some(inference) } else { None }
    }
}

#[cfg(test)]
mod test;
