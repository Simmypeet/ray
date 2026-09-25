//! Discovers the polymorphic variables a function signature introduces
//! implicitly through its parameter types.
//!
//! A function without an explicit type-parameter list does not declare its
//! type, effect, and lifetime variables up front; each one is introduced by its
//! first occurrence in a parameter type. The kind of an introduced variable
//! comes from the position it occupies: a type argument takes the kind of the
//! parameter it instantiates, an effect-row tail is an effect row, and every
//! other type position is a value type.
//!
//! A function with an explicit type-parameter list introduces no variables by
//! name, but each lifetime elided in its parameter types still introduces a
//! fresh lifetime parameter.

use rayc_lexical::tree::RelativeSpan;
use rayc_qbice::TrackedEngine;
use rayc_source_file::SourceElement;
use rayc_symbol::{
    GlobalSymbolID,
    parent::get_parent_global,
    symbol_kind::{SymbolKind, get_symbol_kind},
};
use rayc_syntax::{
    Identifier,
    def::{Callable, ParameterEntry, ParameterList, ParameterType},
    effect_row::{ConcreteEffectRow, EffectRow as EffectRowSyntax},
    given::GivenArguments,
    path::{Path, PathRoot, PathSegment},
    r#type::{Lifetime as LifetimeSyntax, Type as TypeSyntax},
};
use rayc_type::{
    poly_var::{PolyVar, PolyVarMap, PolyVarOrigin, PolyVarStack, get_poly_var_map},
    ty::TyKind,
};

use crate::{lifetime, resolver::find_path_symbol};

/// Discovers the polymorphic variables the parameter types of `site`
/// introduce: lowercase single-letter type variables, effect variables, named
/// lifetimes, and, when `introduce_elided` is set, one fresh lifetime for each
/// lifetime elided outside a callable type.
///
/// Variables that `poly_var_stack` already declares are reused rather than
/// introduced again. Variables are returned in order of first occurrence.
#[must_use]
pub async fn discover_parameter_poly_vars(
    engine: &TrackedEngine,
    site: GlobalSymbolID,
    parameters: Option<&ParameterList>,
    poly_var_stack: Option<&PolyVarStack>,
    introduce_elided: bool,
) -> PolyVarMap {
    Discovery {
        engine,
        site,
        poly_var_stack,
        introduce_named: true,
        introduce_elided,
        poly_vars: PolyVarMap::new(),
    }
    .discover_parameters(parameters)
    .await
}

/// Appends one fresh lifetime for each lifetime elided outside a callable type
/// in the parameter types of `site` to `poly_vars`, which holds its explicitly
/// declared type parameters. No variable is introduced by name.
#[must_use]
pub async fn discover_elided_lifetimes(
    engine: &TrackedEngine,
    site: GlobalSymbolID,
    parameters: Option<&ParameterList>,
    poly_var_stack: Option<&PolyVarStack>,
    poly_vars: PolyVarMap,
) -> PolyVarMap {
    Discovery {
        engine,
        site,
        poly_var_stack,
        introduce_named: false,
        introduce_elided: true,
        poly_vars,
    }
    .discover_parameters(parameters)
    .await
}

/// Returns whether `name` follows the naming convention of an implicitly
/// introduced type variable: a single lowercase ASCII letter.
fn is_type_variable_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(|character| character.is_ascii_lowercase()) && chars.next().is_none()
}

/// The symbol a path prefix names, as far as discovery can tell without
/// resolving type arguments.
#[derive(Debug, Clone, Copy)]
enum PathContext {
    Symbol(GlobalSymbolID),

    /// The prefix names something whose members cannot be looked up before
    /// this signature's polymorphic variables are known, such as one of its
    /// own given parameters, or it does not resolve at all.
    Unknown,
}

struct Discovery<'a> {
    engine: &'a TrackedEngine,
    site: GlobalSymbolID,
    poly_var_stack: Option<&'a PolyVarStack>,

    /// Whether names introduce variables, which they do only for a function
    /// without an explicit type-parameter list.
    introduce_named: bool,
    introduce_elided: bool,
    poly_vars: PolyVarMap,
}

impl Discovery<'_> {
    /// Discovers the variables in every parameter type, in order.
    async fn discover_parameters(mut self, parameters: Option<&ParameterList>) -> PolyVarMap {
        for entry in parameters.into_iter().flat_map(ParameterList::entries) {
            let ParameterEntry::Parameter(parameter) = entry else { continue };

            match parameter.r#type() {
                Some(ParameterType::Type(ty)) => self.discover_type(&ty, TyKind::Star).await,
                Some(ParameterType::CallableSugar(callable)) => {
                    self.discover_callable(&callable).await;
                }
                None => {}
            }
        }

        self.poly_vars
    }

    /// Discovers the variables in `ty`, which occupies a position of kind
    /// `expected`.
    async fn discover_type(&mut self, ty: &TypeSyntax, expected: TyKind) {
        match ty {
            TypeSyntax::EffectRow(row) => Box::pin(self.discover_concrete_effect_row(row)).await,
            TypeSyntax::Primitive(_) => {}
            TypeSyntax::Pointer(pointer) => {
                if let Some(pointed_type) = pointer.pointed_type() {
                    Box::pin(self.discover_type(&pointed_type, TyKind::Star)).await;
                }
            }
            TypeSyntax::Reference(reference) => {
                // The key of an elided lifetime must match the span the resolver looks it up
                // by.
                match reference.lifetime() {
                    Some(lifetime) => self.discover_lifetime(&lifetime),
                    None => self.introduce_elided_lifetime(
                        lifetime::elided_reference_lifetime_span(reference),
                    ),
                }

                if let Some(pointed_type) = reference.pointed_type() {
                    Box::pin(self.discover_type(&pointed_type, TyKind::Star)).await;
                }
            }
            TypeSyntax::Lifetime(lifetime) => self.discover_lifetime(lifetime),
            TypeSyntax::Tuple(tuple) => {
                for element in tuple.elements() {
                    Box::pin(self.discover_type(&element, TyKind::Star)).await;
                }
            }
            TypeSyntax::Path(path) => Box::pin(self.discover_path(path, expected)).await,
        }
    }

    /// Discovers the variables in a callable parameter type.
    async fn discover_callable(&mut self, callable: &Callable) {
        // An elided lifetime in a callable type would need a higher-ranked
        // lifetime, which Ray does not have yet, so it introduces nothing and
        // is reported when resolved.
        let introduce_elided = std::mem::replace(&mut self.introduce_elided, false);

        if let Some(parameters) = callable.parameters() {
            for parameter in parameters.parameters() {
                self.discover_type(&parameter, TyKind::Star).await;
            }
        }
        if let Some(return_type) = callable.return_type().and_then(|x| x.r#type()) {
            self.discover_type(&return_type, TyKind::Star).await;
        }
        if let Some(effect_row) = callable.effect_row().and_then(|x| x.effect_row()) {
            match effect_row {
                EffectRowSyntax::Path(path) => self.discover_path(&path, TyKind::EffectRow).await,
                EffectRowSyntax::ConcreteEffectRow(row) => {
                    self.discover_concrete_effect_row(&row).await;
                }
            }
        }

        self.introduce_elided = introduce_elided;
    }

    /// Discovers the variables in the labels and the tail of `row`.
    async fn discover_concrete_effect_row(&mut self, row: &ConcreteEffectRow) {
        // A label names an effect, never a variable, but its arguments may
        // introduce variables.
        for label in row.effects() {
            self.discover_path_arguments(&label).await;
        }

        if let Some(tail) = row.tail().and_then(|tail| tail.variable()) {
            self.discover_path(&tail, TyKind::EffectRow).await;
        }
    }

    /// Discovers the variables in a path that occupies a position of kind
    /// `expected`: the path itself when it is a bare variable name, or its
    /// arguments otherwise.
    async fn discover_path(&mut self, path: &Path, expected: TyKind) {
        match path.bare_identifier() {
            Some(identifier) => self.discover_variable(&identifier, expected).await,
            None => self.discover_path_arguments(path).await,
        }
    }

    /// Introduces the variable a bare identifier names in a position of kind
    /// `expected`, unless the name is already bound.
    async fn discover_variable(&mut self, identifier: &Identifier, expected: TyKind) {
        if !self.introduce_named {
            return;
        }
        let name = &identifier.kind.0;

        // Only type and effect-row positions introduce variables by a bare
        // name; lifetimes are written with a quote, and dictionaries are
        // declared by given parameters.
        let variable = match expected {
            TyKind::Star if is_type_variable_name(name) => {
                PolyVar::new_type(name.clone(), identifier.span())
            }
            TyKind::EffectRow => PolyVar::new_effect(name.clone(), identifier.span()),
            TyKind::Star | TyKind::Instance | TyKind::Lifetime => return,
        };

        // A name bound by an enclosing symbol, by an earlier parameter, or by
        // a visible symbol does not introduce a new variable.
        if self.find_poly_var(name) {
            return;
        }
        if find_path_symbol(self.engine, self.site, None, name).await.is_some() {
            return;
        }

        let _ = self.poly_vars.insert(variable);
    }

    /// Discovers the variables in every argument list of `path`, giving each
    /// type argument the kind of the parameter it instantiates.
    async fn discover_path_arguments(&mut self, path: &Path) {
        let mut context = match path.root() {
            Some(PathRoot::This(_)) => Some(self.this_trait().await),
            Some(PathRoot::Segment(_)) | None => None,
        };

        for segment in path.segments() {
            context = Some(self.segment_context(context, &segment).await);

            let Some(arguments) = segment.arguments() else { continue };

            let kinds = match context {
                Some(PathContext::Symbol(symbol_id)) => self.argument_kinds(symbol_id).await,
                Some(PathContext::Unknown) | None => Vec::new(),
            };

            // An argument whose parameter is unknown, or that exceeds the
            // arity, is resolved as a value type.
            for (index, argument) in arguments.type_arguments().enumerate() {
                let kind = kinds.get(index).copied().unwrap_or(TyKind::Star);
                Box::pin(self.discover_type(&argument, kind)).await;
            }

            // A given argument names a dictionary, never a variable.
            for argument in
                arguments.given_arguments().iter().flat_map(GivenArguments::given_arguments)
            {
                if let Some(dictionary) = argument.dictionary() {
                    Box::pin(self.discover_path_arguments(&dictionary)).await;
                }
            }
        }
    }

    /// Returns what `segment` names, given what the preceding segments name.
    async fn segment_context(
        &self,
        previous: Option<PathContext>,
        segment: &PathSegment,
    ) -> PathContext {
        let Some(identifier) = segment.identifier() else { return PathContext::Unknown };
        let name = &identifier.kind.0;

        let parent = match previous {
            Some(PathContext::Symbol(parent)) => Some(parent),
            Some(PathContext::Unknown) => return PathContext::Unknown,

            // A polymorphic variable shadows a symbol of the same name. Only
            // an enclosing dictionary has a trait whose members are known.
            None if self.find_poly_var(name) => {
                return self
                    .poly_var_stack
                    .and_then(|stack| stack.trait_ref_of(stack.find_by_name(name)?))
                    .map_or(PathContext::Unknown, |x| PathContext::Symbol(x.trait_id()));
            }
            None => None,
        };

        find_path_symbol(self.engine, self.site, parent, name)
            .await
            .map_or(PathContext::Unknown, PathContext::Symbol)
    }

    /// Returns the trait that `this` refers to.
    async fn this_trait(&self) -> PathContext {
        let Some(parent) = self.engine.get_parent_global(self.site).await else {
            return PathContext::Unknown;
        };

        if self.engine.get_symbol_kind(parent).await == SymbolKind::Trait {
            PathContext::Symbol(parent)
        } else {
            PathContext::Unknown
        }
    }

    /// Returns the kinds of the explicit type parameters of `symbol_id`, or
    /// nothing when its type arguments are implicit.
    async fn argument_kinds(&self, symbol_id: GlobalSymbolID) -> Vec<TyKind> {
        // The type arguments of a function are always inferred. Their map is
        // also discovered like this one, so querying it could form a cycle.
        let symbol_kind = self.engine.get_symbol_kind(symbol_id).await;
        let explicit = match symbol_kind {
            SymbolKind::Effect
            | SymbolKind::Instance
            | SymbolKind::InstanceType
            | SymbolKind::MarkerImplementation
            | SymbolKind::Strut
            | SymbolKind::Trait
            | SymbolKind::TraitType => true,
            SymbolKind::Def
            | SymbolKind::InstanceDef
            | SymbolKind::TraitDef
            | SymbolKind::ExternDef
            | SymbolKind::EffectOperation
            | SymbolKind::Marker
            | SymbolKind::Module => false,
        };
        if !explicit {
            return Vec::new();
        }

        self.engine
            .get_poly_var_map(symbol_id)
            .await
            .iter()
            .map(|(_, parameter)| parameter.kind())
            .take_while(|kind| *kind != TyKind::Instance)
            .collect()
    }

    /// Returns whether `name` is bound by an enclosing symbol or by an earlier
    /// parameter type.
    fn find_poly_var(&self, name: &str) -> bool {
        self.poly_vars.find_by_name(name).is_some()
            || self.poly_var_stack.is_some_and(|stack| stack.find_by_name(name).is_some())
    }

    /// Introduces the lifetime `lifetime` names, unless it is already bound.
    /// An elided lifetime (`'_`) is introduced only when elided lifetimes
    /// introduce fresh parameters.
    fn discover_lifetime(&mut self, lifetime: &LifetimeSyntax) {
        if lifetime.is_placeholder() {
            self.introduce_elided_lifetime(lifetime.span());
            return;
        }

        let Some(identifier) = lifetime.identifier() else { return };
        if !self.introduce_named {
            return;
        }
        let name = lifetime::lifetime_parameter_name(&identifier.kind.0);
        if self.find_poly_var(&name) {
            return;
        }

        let _ = self
            .poly_vars
            .insert(PolyVar::new_lifetime(self.engine.intern_unsized(name), lifetime.span()));
    }

    /// Introduces the fresh lifetime parameter for the lifetime elided at
    /// `span`, when elided lifetimes introduce fresh parameters.
    fn introduce_elided_lifetime(&mut self, span: RelativeSpan) {
        if !self.introduce_elided {
            return;
        }

        self.poly_vars.insert_generated(
            PolyVar::new_lifetime(self.engine.intern_unsized("'_"), span),
            PolyVarOrigin::ElidedLifetime(span),
        );
    }
}
