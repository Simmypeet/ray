use std::{
    collections::hash_map::Entry,
    fmt::{self, Display, Write},
};

use qbice::storage::intern::Interned;
use rayc_hash::FxHashMap;
use rayc_qbice::TrackedEngine;
use rayc_symbol::{GlobalSymbolID, name::get_name};

use crate::{
    poly_var::{Key as PolyVarKey, PolyVarMap},
    ty::{
        InferenceConstraint, Mutability, Ty, TyKind, application,
        application::View as ApplicationView, lifetime::Lifetime,
    },
};

impl Ty {
    pub async fn display<'x>(&'x self, engine: &TrackedEngine) -> TyDisplay<'x> {
        let mut poly_var_maps = FxHashMap::default();
        let mut symbol_names = FxHashMap::default();
        self.collect_display_context(engine, &mut poly_var_maps, &mut symbol_names).await;

        TyDisplay { ty: self, poly_var_maps, symbol_names }
    }

    async fn collect_display_context(
        &self,
        engine: &TrackedEngine,
        poly_var_maps: &mut FxHashMap<GlobalSymbolID, Interned<PolyVarMap>>,
        symbol_names: &mut FxHashMap<GlobalSymbolID, Interned<str>>,
    ) {
        for ty in self.recursive_iter() {
            match ty {
                Self::Application(application) => {
                    let symbol_id = match application.view() {
                        ApplicationView::Instance(instance) => Some(instance.symbol_id()),
                        ApplicationView::Struct(struct_) => Some(struct_.symbol_id()),
                        ApplicationView::Enum(enum_) => Some(enum_.symbol_id()),
                        ApplicationView::InstanceAssociated(associated) => {
                            Some(associated.symbol_id())
                        }
                        ApplicationView::DefCoroutine(coroutine) => Some(coroutine.def_id()),
                        ApplicationView::TraitDefCoroutine(coroutine) => {
                            Some(coroutine.trait_def_id())
                        }
                        ApplicationView::ClosureCoroutine(_)
                        | ApplicationView::Primitive(_)
                        | ApplicationView::Tuple(_)
                        | ApplicationView::Pointer(_)
                        | ApplicationView::Reference(_)
                        | ApplicationView::DefInstance(_)
                        | ApplicationView::Closure(_)
                        | ApplicationView::NoOpDropInstance(_)
                        | ApplicationView::TupleDropInstance(_)
                        | ApplicationView::ClosureDropInstance(_)
                        | ApplicationView::ContinuationDefInstance(_)
                        | ApplicationView::CoroutineInstance(_)
                        | ApplicationView::CoroutineDropInstance(_)
                        | ApplicationView::GeneratedAdtDropInstance(_)
                        | ApplicationView::Error => None,
                    };
                    if let Some(symbol_id) = symbol_id {
                        Self::collect_symbol_display_context(
                            engine,
                            symbol_id,
                            poly_var_maps,
                            symbol_names,
                        )
                        .await;
                    }
                }
                Self::Inference(_) | Self::SelfInstance(_) | Self::Lifetime(_) => {}
                Self::PolyVar(poly_var) => {
                    let symbol_id = poly_var.parent_id();
                    if let Entry::Vacant(entry) = poly_var_maps.entry(symbol_id) {
                        let poly_var_map = engine.query(&PolyVarKey { symbol_id }).await;
                        entry.insert(poly_var_map);
                    }
                }
                Self::EffectRow(row) => {
                    for label in row.labels() {
                        Self::collect_symbol_display_context(
                            engine,
                            label.effect_symbol_id(),
                            poly_var_maps,
                            symbol_names,
                        )
                        .await;
                    }
                }
            }
        }
    }

    async fn collect_symbol_display_context(
        engine: &TrackedEngine,
        symbol_id: GlobalSymbolID,
        poly_var_maps: &mut FxHashMap<GlobalSymbolID, Interned<PolyVarMap>>,
        symbol_names: &mut FxHashMap<GlobalSymbolID, Interned<str>>,
    ) {
        if let Entry::Vacant(entry) = poly_var_maps.entry(symbol_id) {
            entry.insert(engine.query(&PolyVarKey { symbol_id }).await);
        }
        if let Entry::Vacant(entry) = symbol_names.entry(symbol_id) {
            entry.insert(engine.get_name(symbol_id).await);
        }
    }
}

#[derive(Debug)]
pub struct TyDisplay<'x> {
    ty: &'x Ty,
    poly_var_maps: FxHashMap<GlobalSymbolID, Interned<PolyVarMap>>,
    symbol_names: FxHashMap<GlobalSymbolID, Interned<str>>,
}

impl TyDisplay<'_> {
    fn fmt_symbol_application<'x>(
        &self,
        symbol_id: GlobalSymbolID,
        arguments: impl IntoIterator<Item = &'x Interned<Ty>>,
        f: &mut fmt::Formatter<'_>,
    ) -> fmt::Result {
        let name = self.symbol_names.get(&symbol_id).expect("should've been collected earlier");
        f.write_str(name)?;

        let poly_var_map =
            self.poly_var_maps.get(&symbol_id).expect("should've been collected earlier");
        let type_argument_count = poly_var_map
            .iter()
            .take_while(|(_, poly_var)| poly_var.kind() != TyKind::Instance)
            .count();

        let mut arguments = arguments.into_iter().peekable();
        if type_argument_count > 0 {
            f.write_char('[')?;
            for (index, argument) in arguments.by_ref().take(type_argument_count).enumerate() {
                if index > 0 {
                    f.write_str(", ")?;
                }
                self.fmt_ty(argument, f)?;
            }
            if arguments.peek().is_some() {
                f.write_str("; given ")?;
            }
        } else if arguments.peek().is_some() {
            f.write_str("[given ")?;
        } else {
            return Ok(());
        }

        for (index, argument) in arguments.enumerate() {
            if index > 0 {
                f.write_str(", ")?;
            }
            self.fmt_ty(argument, f)?;
        }
        f.write_char(']')?;

        Ok(())
    }

    fn fmt_signature(
        &self,
        parameters: &[Interned<Ty>],
        return_type: &Ty,
        effect_row: &Ty,
        f: &mut fmt::Formatter<'_>,
    ) -> fmt::Result {
        f.write_char('(')?;
        for (index, parameter) in parameters.iter().enumerate() {
            if index > 0 {
                f.write_str(", ")?;
            }
            self.fmt_ty(parameter, f)?;
        }
        f.write_str(") -> ")?;
        self.fmt_ty(return_type, f)?;
        f.write_str(" \\ ")?;
        self.fmt_ty(effect_row, f)
    }

    /// Formats a built-in Drop dictionary by its kind and target type.
    fn fmt_drop_instance(
        &self,
        view: ApplicationView<'_>,
        f: &mut fmt::Formatter<'_>,
    ) -> fmt::Result {
        let (description, target) = match view {
            ApplicationView::NoOpDropInstance(ty) => ("<no-op drop instance>", ty),
            ApplicationView::TupleDropInstance(instance) => {
                ("<tuple drop instance>", instance.tuple())
            }
            ApplicationView::ClosureDropInstance(instance) => {
                ("<closure drop instance>", instance.closure())
            }
            ApplicationView::GeneratedAdtDropInstance(instance) => {
                ("<nominal drop instance>", instance.nominal())
            }
            ApplicationView::CoroutineDropInstance(instance) => {
                ("<coroutine drop instance>", instance.coroutine())
            }
            ApplicationView::DefCoroutine(_)
            | ApplicationView::TraitDefCoroutine(_)
            | ApplicationView::ClosureCoroutine(_)
            | ApplicationView::ContinuationDefInstance(_)
            | ApplicationView::CoroutineInstance(_)
            | ApplicationView::Primitive(_)
            | ApplicationView::Tuple(_)
            | ApplicationView::Pointer(_)
            | ApplicationView::Reference(_)
            | ApplicationView::Struct(_)
            | ApplicationView::Enum(_)
            | ApplicationView::Instance(_)
            | ApplicationView::InstanceAssociated(_)
            | ApplicationView::Closure(_)
            | ApplicationView::DefInstance(_)
            | ApplicationView::Error => panic!("expected a built-in Drop dictionary"),
        };
        f.write_str(description)?;
        self.fmt_ty(target, f)
    }

    /// Formats a coroutine type by the function it suspends, as in
    /// `<coroutine of f[int32]>`. The arguments of the enclosing symbols and
    /// the given dictionaries are left out: the type arguments decide the
    /// dictionaries, which only lengthen the name.
    fn fmt_def_coroutine(
        &self,
        coroutine: application::DefCoroutineView<'_>,
        f: &mut fmt::Formatter<'_>,
    ) -> fmt::Result {
        let own_argument_count = self
            .poly_var_maps
            .get(&coroutine.def_id())
            .expect("should've been collected earlier")
            .iter()
            .take_while(|(_, poly_var)| poly_var.kind() != TyKind::Instance)
            .count();

        f.write_str("<coroutine of ")?;
        self.fmt_symbol_application(
            coroutine.def_id(),
            coroutine.poly_args().iter().take(own_argument_count),
            f,
        )?;
        f.write_char('>')
    }

    /// Formats the coroutine type of a call through an instance by the
    /// method it calls, as in `<coroutine of d.m[int32]>`.
    fn fmt_trait_def_coroutine(
        &self,
        coroutine: application::TraitDefCoroutineView<'_>,
        f: &mut fmt::Formatter<'_>,
    ) -> fmt::Result {
        f.write_str("<coroutine of ")?;
        self.fmt_ty(coroutine.instance(), f)?;
        f.write_char('.')?;
        self.fmt_symbol_application(coroutine.trait_def_id(), coroutine.args(), f)?;
        f.write_char('>')
    }

    /// Formats the coroutine type of a call of a closure by the closure, as
    /// in `<coroutine of <closure>(int32) -> int32>`.
    fn fmt_closure_coroutine(
        &self,
        coroutine: application::ClosureCoroutineView<'_>,
        f: &mut fmt::Formatter<'_>,
    ) -> fmt::Result {
        f.write_str("<coroutine of ")?;
        self.fmt_ty(coroutine.closure(), f)?;
        f.write_char('>')
    }

    fn fmt_tuple(
        &self,
        tuple: application::TupleView<'_>,
        f: &mut fmt::Formatter<'_>,
    ) -> fmt::Result {
        f.write_char('(')?;

        for (i, arg) in tuple.args().iter().enumerate() {
            if i > 0 {
                f.write_str(", ")?;
            }
            self.fmt_ty(arg, f)?;
        }

        f.write_char(')')
    }

    fn fmt_reference(
        &self,
        reference: application::ReferenceView<'_>,
        f: &mut fmt::Formatter<'_>,
    ) -> fmt::Result {
        f.write_char('&')?;

        // An erased lifetime is left out, as in Rust.
        if **reference.lifetime() != Ty::Lifetime(Lifetime::Erased) {
            self.fmt_ty(reference.lifetime(), f)?;
            f.write_char(' ')?;
        }
        if reference.mutability() == Mutability::Mutable {
            f.write_str("mut ")?;
        }
        self.fmt_ty(reference.pointee(), f)
    }

    /// Formats a built-in dictionary by the type it is the dictionary of, as
    /// in `DefInstance[Continuation[(), c]]`.
    fn fmt_built_in_instance(
        &self,
        name: &str,
        implementor: &Ty,
        f: &mut fmt::Formatter<'_>,
    ) -> fmt::Result {
        f.write_str(name)?;
        f.write_char('[')?;
        self.fmt_ty(implementor, f)?;
        f.write_char(']')
    }

    fn fmt_ty(&self, ty: &Ty, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match ty {
            Ty::Application(ty_application) => match ty_application.view() {
                ApplicationView::Primitive(primitive) => f.write_str(primitive.keyword()),

                view @ (ApplicationView::NoOpDropInstance(_)
                | ApplicationView::TupleDropInstance(_)
                | ApplicationView::ClosureDropInstance(_)
                | ApplicationView::CoroutineDropInstance(_)
                | ApplicationView::GeneratedAdtDropInstance(_)) => self.fmt_drop_instance(view, f),

                ApplicationView::DefCoroutine(coroutine) => self.fmt_def_coroutine(coroutine, f),
                ApplicationView::TraitDefCoroutine(view) => self.fmt_trait_def_coroutine(view, f),
                ApplicationView::ClosureCoroutine(view) => self.fmt_closure_coroutine(view, f),
                ApplicationView::CoroutineInstance(coroutine) => {
                    self.fmt_built_in_instance("CoroutineInstance", coroutine, f)
                }
                ApplicationView::ContinuationDefInstance(instance) => {
                    self.fmt_built_in_instance("DefInstance", instance.continuation(), f)
                }

                ApplicationView::Closure(closure) => {
                    f.write_str("<closure>")?;
                    self.fmt_signature(
                        closure.params(),
                        closure.return_type(),
                        closure.effect_row(),
                        f,
                    )
                }

                ApplicationView::Tuple(tuple) => self.fmt_tuple(tuple, f),

                ApplicationView::Pointer(pointer) => {
                    f.write_char('*')?;
                    if pointer.mutability() == Mutability::Mutable {
                        f.write_str("mut ")?;
                    }
                    self.fmt_ty(pointer.pointee(), f)
                }
                ApplicationView::Reference(reference) => self.fmt_reference(reference, f),
                ApplicationView::Instance(instance) => {
                    self.fmt_symbol_application(instance.symbol_id(), instance.args(), f)
                }
                ApplicationView::Struct(struct_) => {
                    self.fmt_symbol_application(struct_.symbol_id(), struct_.args(), f)
                }
                ApplicationView::Enum(enum_) => {
                    self.fmt_symbol_application(enum_.symbol_id(), enum_.args(), f)
                }
                ApplicationView::DefInstance(closure) => {
                    f.write_str("DefInstance[")?;
                    self.fmt_ty(closure, f)?;
                    f.write_char(']')
                }
                ApplicationView::InstanceAssociated(associated) => {
                    self.fmt_ty(associated.instance(), f)?;
                    f.write_char('.')?;
                    self.fmt_symbol_application(associated.symbol_id(), associated.args(), f)
                }
                ApplicationView::Error => write!(f, "<error>"),
            },

            Ty::Inference(inference) if inference.kind() == TyKind::Lifetime => f.write_str("'_"),
            Ty::Inference(inference) => match inference.constraint() {
                InferenceConstraint::Any => write!(f, "{{any}}"),
                InferenceConstraint::Numeric => {
                    write!(f, "{{numeric}}")
                }
                InferenceConstraint::EqualityComparable => {
                    write!(f, "{{equality comparable}}")
                }
            },

            Ty::SelfInstance(_) => f.write_str("this"),
            Ty::Lifetime(lifetime) => write!(f, "{lifetime}"),
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

                    self.fmt_symbol_application(label.effect_symbol_id(), label.arguments(), f)?;
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
