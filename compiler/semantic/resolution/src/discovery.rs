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

use qbice::storage::intern::Interned;
use rayc_hash::FxHashMap;
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
    given::{GivenArguments, GivenParameterList, GivenParameters},
    path::{Path, PathRoot, PathSegment},
    r#type::{Lifetime as LifetimeSyntax, Type as TypeSyntax},
};
use rayc_type::{
    poly_var::{PolyVar, PolyVarMap, PolyVarOrigin, PolyVarStack, get_poly_var_map},
    ty::TyKind,
};

use crate::{
    lifetime,
    resolver::{find_path_symbol, find_path_target},
};

/// Discovers the polymorphic variables the parameter types of `site`
/// introduce: lowercase-named type and effect variables, named lifetimes, and,
/// when `introduce_elided` is set, one fresh lifetime for each lifetime elided
/// outside a callable type.
///
/// Variables that `poly_var_stack` already declares are reused rather than
/// introduced again. Variables are returned in order of first occurrence.
#[must_use]
pub async fn discover_parameter_poly_vars(
    engine: &TrackedEngine,
    site: GlobalSymbolID,
    parameters: Option<&ParameterList>,
    given_traits: &GivenTraits,
    poly_var_stack: Option<&PolyVarStack>,
    introduce_elided: bool,
) -> PolyVarMap {
    Discovery {
        engine,
        site,
        poly_var_stack,
        given_traits,
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
    given_traits: &GivenTraits,
    poly_var_stack: Option<&PolyVarStack>,
    poly_vars: PolyVarMap,
) -> PolyVarMap {
    Discovery {
        engine,
        site,
        poly_var_stack,
        given_traits,
        introduce_named: false,
        introduce_elided: true,
        poly_vars,
    }
    .discover_parameters(parameters)
    .await
}

/// The trait of each of a signature's own given parameters, by name.
///
/// Discovery runs before the given parameters are resolved: a given
/// parameter's trait reference can mention the variables being discovered, as
/// `s` in `given (s: Show[a])` mentions `a`. Discovery needs only the trait,
/// which the trait path names without its arguments. With it, a member of the
/// dictionary resolves while discovering. In
///
/// ```ray
/// def run(handler: Handler[d.Row[r]]) given (d: Scoped) -> int32
/// ```
///
/// `d.Row` is the `Row` of `Scoped`, so `r` takes the kind of its parameter.
#[derive(Debug, Clone, Default)]
pub struct GivenTraits {
    traits: FxHashMap<Interned<str>, GlobalSymbolID>,
}

impl GivenTraits {
    /// Records the trait that each of `given_parameters` names, as seen from
    /// `site`. A parameter whose path does not name a trait is left out; it is
    /// reported when the given parameters are resolved.
    pub async fn new(
        engine: &TrackedEngine,
        site: GlobalSymbolID,
        given_parameters: Option<&GivenParameterList>,
    ) -> Self {
        let mut traits = FxHashMap::default();

        for parameter in given_parameters
            .and_then(GivenParameterList::parameters)
            .iter()
            .flat_map(GivenParameters::parameters)
        {
            let (Some(name), Some(trait_reference)) =
                (parameter.name(), parameter.trait_reference())
            else {
                continue;
            };
            let Some(symbol_id) = find_path_target(engine, site, &trait_reference).await else {
                continue;
            };

            // A duplicated name is reported when the given parameters are
            // declared; the first declaration wins, as it does there.
            if engine.get_symbol_kind(symbol_id).await == SymbolKind::Trait {
                traits.entry(name.kind.0).or_insert(symbol_id);
            }
        }

        Self { traits }
    }

    /// Returns whether `name` is one of the given parameters.
    fn contains(&self, name: &str) -> bool { self.traits.contains_key(name) }

    /// Returns the trait of the given parameter `name`.
    fn trait_of(&self, name: &str) -> Option<GlobalSymbolID> { self.traits.get(name).copied() }
}

/// The symbol a path prefix names, as far as discovery can tell without
/// resolving type arguments.
#[derive(Debug, Clone, Copy)]
enum PathContext {
    Symbol(GlobalSymbolID),

    /// The prefix names something whose members are unknown, such as a type
    /// variable, or it does not resolve at all.
    Unknown,
}

impl PathContext {
    /// Returns the symbol the prefix names, if it is known.
    const fn symbol(self) -> Option<GlobalSymbolID> {
        match self {
            Self::Symbol(symbol_id) => Some(symbol_id),
            Self::Unknown => None,
        }
    }
}

struct Discovery<'a> {
    engine: &'a TrackedEngine,
    site: GlobalSymbolID,
    poly_var_stack: Option<&'a PolyVarStack>,
    given_traits: &'a GivenTraits,

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
            if let Some(ty) = parameter.r#type() {
                self.discover_parameter_type(&ty).await;
            }
        }

        self.poly_vars
    }

    /// Discovers the variables in the annotation of a parameter.
    async fn discover_parameter_type(&mut self, ty: &ParameterType) {
        match ty {
            ParameterType::Type(ty) => self.discover_type(ty, TyKind::Star).await,
            ParameterType::CallableSugar(callable) => self.discover_callable(callable).await,
        }
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
                // The key of an elided lifetime must match the span the
                // resolver looks it up by.
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

        // Discover the parameters, the return type and the effect row, in
        // that order.
        if let Some(parameters) = callable.parameters() {
            for parameter in parameters.parameters() {
                self.discover_type(&parameter, TyKind::Star).await;
            }
        }
        if let Some(return_type) = callable.return_type().and_then(|x| x.r#type()) {
            self.discover_type(&return_type, TyKind::Star).await;
        }
        if let Some(effect_row) = callable.effect_row().and_then(|x| x.effect_row()) {
            self.discover_effect_row(&effect_row).await;
        }

        self.introduce_elided = introduce_elided;
    }

    /// Discovers the variables in an effect-row annotation.
    async fn discover_effect_row(&mut self, effect_row: &EffectRowSyntax) {
        match effect_row {
            EffectRowSyntax::Path(path) => self.discover_path(path, TyKind::EffectRow).await,
            EffectRowSyntax::ConcreteEffectRow(row) => self.discover_concrete_effect_row(row).await,
        }
    }

    /// Discovers the variables in the labels and the tail of `row`.
    async fn discover_concrete_effect_row(&mut self, row: &ConcreteEffectRow) {
        // A label names an effect, never a variable, but its arguments may
        // introduce variables.
        for label in row.effects() {
            self.discover_path_arguments(&label).await;
        }

        // The tail is an effect row.
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
        let Some(variable) = Self::implicit_variable(identifier, expected) else { return };

        // A name bound by an enclosing symbol, by an earlier parameter, or by
        // a visible symbol does not introduce a new variable.
        let name = &identifier.kind.0;
        if self.find_poly_var(name)
            || find_path_symbol(self.engine, self.site, None, name).await.is_some()
        {
            return;
        }

        let _ = self.poly_vars.insert(variable);
    }

    /// Returns the variable a bare `identifier` would introduce in a position
    /// of kind `expected`.
    ///
    /// Only value-type and effect-row positions introduce variables by a bare
    /// name: a lifetime is written with a quote, and a dictionary is declared
    /// by a given parameter. The name must also follow the naming convention
    /// of a variable, so that `x: Point` names a struct while `x: elem` and
    /// `\ effects` introduce variables.
    fn implicit_variable(identifier: &Identifier, expected: TyKind) -> Option<PolyVar> {
        let name = &identifier.kind.0;
        if !Self::is_variable_name(name) {
            return None;
        }

        match expected {
            TyKind::Star => Some(PolyVar::new_type(name.clone(), identifier.span())),
            TyKind::EffectRow => Some(PolyVar::new_effect(name.clone(), identifier.span())),
            TyKind::Instance | TyKind::Lifetime => None,
        }
    }

    /// Returns whether `name` follows the naming convention of an implicitly
    /// introduced variable: a lowercase name of any length, which starts with
    /// a lowercase ASCII letter and contains no uppercase letter, as `a`,
    /// `elem` or `row_type`.
    fn is_variable_name(name: &str) -> bool {
        name.chars().next().is_some_and(|character| character.is_ascii_lowercase())
            && !name.chars().any(char::is_uppercase)
    }

    /// Discovers the variables in every argument list of `path`, giving each
    /// type argument the kind of the parameter it instantiates.
    async fn discover_path_arguments(&mut self, path: &Path) {
        let mut context = self.root_context(path).await;

        for segment in path.segments() {
            context = Some(self.segment_context(context, &segment).await);
            let Some(arguments) = segment.arguments() else { continue };

            // Each type argument takes the kind of its parameter. An argument
            // whose parameter is unknown, because the path does not resolve
            // or the argument exceeds the arity, introduces no variables; the
            // resolver reports it instead.
            let parameters = match context.and_then(PathContext::symbol) {
                Some(symbol_id) => self.argument_parameters(symbol_id).await,
                None => None,
            };

            for (index, argument) in arguments.type_arguments().enumerate() {
                let Some(kind) = parameters
                    .as_ref()
                    .and_then(|parameters| parameters.type_parameter_kind(index))
                else {
                    break;
                };
                Box::pin(self.discover_type(&argument, kind)).await;
            }

            // A given argument names a dictionary, never a variable, but its
            // own arguments may introduce variables.
            for argument in
                arguments.given_arguments().iter().flat_map(GivenArguments::given_arguments)
            {
                if let Some(dictionary) = argument.dictionary() {
                    Box::pin(self.discover_path_arguments(&dictionary)).await;
                }
            }
        }
    }

    /// Returns what the root of `path` names before its first segment: the
    /// enclosing trait for `this`, or nothing for an ordinary segment, which
    /// is looked up as the first segment.
    async fn root_context(&self, path: &Path) -> Option<PathContext> {
        match path.root() {
            Some(PathRoot::This(_)) => Some(self.this_trait().await),
            Some(PathRoot::Segment(_)) | None => None,
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

        // Find the symbol whose member the segment names, if any.
        let parent = match previous {
            Some(PathContext::Symbol(parent)) => Some(parent),
            Some(PathContext::Unknown) => return PathContext::Unknown,

            // A polymorphic variable shadows a symbol of the same name. Only
            // a dictionary, whether this signature's own or an enclosing
            // symbol's, has a trait whose members are known.
            None if self.find_poly_var(name) => {
                let trait_id = self.given_traits.trait_of(name).or_else(|| {
                    let stack = self.poly_var_stack?;
                    Some(stack.trait_ref_of(stack.find_by_name(name)?)?.trait_id())
                });
                return trait_id.map_or(PathContext::Unknown, PathContext::Symbol);
            }
            None => None,
        };

        // Look the segment up in its parent, or among the visible names.
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

    /// Returns the polymorphic variables of `symbol_id` that the arguments of a
    /// path naming it instantiate, if discovery may query them.
    async fn argument_parameters(&self, symbol_id: GlobalSymbolID) -> Option<Interned<PolyVarMap>> {
        // A definition that may leave out its type-parameter list has its
        // map discovered like this one, so querying it could form a cycle. A
        // definition is never a type, so its parameters are not needed here.
        let symbol_kind = self.engine.get_symbol_kind(symbol_id).await;
        if !symbol_kind.has_poly_var_map() || symbol_kind.has_optional_type_parameter_list() {
            return None;
        }

        Some(self.engine.get_poly_var_map(symbol_id).await)
    }

    /// Returns whether `name` is bound by an enclosing symbol, by an earlier
    /// parameter type, or by one of this signature's given parameters.
    fn find_poly_var(&self, name: &str) -> bool {
        self.poly_vars.find_by_name(name).is_some()
            || self.given_traits.contains(name)
            || self.poly_var_stack.is_some_and(|stack| stack.find_by_name(name).is_some())
    }

    /// Introduces the lifetime `lifetime` names, unless it is already bound.
    /// An elided lifetime (`'_`) is introduced only when elided lifetimes
    /// introduce fresh parameters.
    fn discover_lifetime(&mut self, lifetime: &LifetimeSyntax) {
        // `'_` stands for an elided lifetime.
        if lifetime.is_placeholder() {
            self.introduce_elided_lifetime(lifetime.span());
            return;
        }

        // A named lifetime is introduced by its first occurrence; `'static`
        // has no identifier and is never introduced.
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
