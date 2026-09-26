//! The variance of struct and `eff` parameters.
//!
//! The variance of a struct parameter is the join of the variances of its
//! occurrences in the struct's field types. The variance of an `eff` parameter
//! comes from its operation signatures: a `perform` calls the handler, so
//! parameter types start covariant and the return type starts contravariant.
//!
//! Structs can be recursive and mutually recursive, and structs and effects
//! can mention each other: an operation signature can name a struct, and a
//! struct can name an effect row inside a closure type. So every struct and
//! `eff` in a target is computed together as one fixed point that starts every
//! parameter at bivariant.
//!
//! A parameter can instead declare its variance, as in `+t`, `-t` or `=t`.
//! Its variance is then fixed to the declared one, and every use of it must
//! be within that variance; see [`VarianceMismatch`].

use linkme::distributed_slice;
use qbice::{
    Decode, Encode, Identifiable, Query, StableHash, executor, program::Registration,
    storage::intern::Interned,
};
use rayc_diagnostic::{ByteIndex, Highlight, Rendered, Report};
use rayc_hash::FxHashMap;
use rayc_lexical::tree::RelativeSpan;
use rayc_qbice::{Config, RAY_PROGRAM, TrackedEngine};
use rayc_semantic_element::{
    parameter::get_parameter_map, return_type::get_return_type, struct_body::get_struct_body,
};
use rayc_source_file::SourceElement;
use rayc_symbol::{
    GlobalSymbolID, SymbolID,
    member::get_members,
    name::get_name,
    source_map::to_absolute_span,
    span::get_span,
    symbol_kind::{SymbolKind, get_all_effect_ids, get_all_nominal_type_ids, get_symbol_kind},
    syntax::get_return_type_syntax,
};
use rayc_target::TargetID;
use rayc_type::{
    poly_var::{PolyVarID, PolyVarMap, get_poly_var_map},
    ty::Ty,
    variance::{Variance, VarianceKey, VarianceMap, get_variance},
};

#[cfg(test)]
mod test;

/// Retrieves the variances of every struct and `eff` in a target.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Query,
)]
#[value(Interned<FxHashMap<SymbolID, Interned<VarianceMap>>>)]
struct VariancesKey {
    target_id: TargetID,
}

#[executor(config = Config)]
async fn variances_executor(
    &VariancesKey { target_id }: &VariancesKey,
    engine: &TrackedEngine,
) -> Interned<FxHashMap<SymbolID, Interned<VarianceMap>>> {
    // Start every parameter of every struct and effect at its declared
    // variance, or at bivariant.
    let struct_ids = engine.get_all_nominal_type_ids(target_id).await;
    let effect_ids = engine.get_all_effect_ids(target_id).await;
    let owner_ids = struct_ids
        .iter()
        .map(|id| (*id, VarianceOwner::Struct))
        .chain(effect_ids.iter().map(|id| (*id, VarianceOwner::Effect)));

    let mut owners = Vec::with_capacity(struct_ids.len() + effect_ids.len());
    let mut variances = FxHashMap::default();
    for (owner_id, owner) in owner_ids {
        let owner_id = target_id.make_global(owner_id);
        let poly_vars = engine.get_poly_var_map(owner_id).await;
        variances.insert(owner_id, VarianceMap::new(&poly_vars));
        owners.push((owner_id, owner, poly_vars));
    }

    // Grow the variances until nothing changes, then make every unused
    // parameter that is not a lifetime invariant. That can make a use of it
    // elsewhere invariant too, so grow again until the defaults change
    // nothing. Variances only move up the lattice, so this terminates.
    loop {
        let mut changed = true;
        while changed {
            changed = false;
            for (owner_id, owner, poly_vars) in &owners {
                let inferrer = VarianceInferrer::new(*owner_id, poly_vars, &mut variances, engine);
                changed |= inferrer.infer_declaration(*owner).await;
            }
        }

        let mut defaulted = false;
        for (owner_id, _, poly_vars) in &owners {
            let map = variances.get_mut(owner_id).expect("every owner has variances");
            defaulted |= map.default_unused(poly_vars);
        }
        if !defaulted {
            break;
        }
    }

    engine.intern(
        variances
            .into_iter()
            .map(|(owner_id, variances)| (owner_id.id, engine.intern(variances)))
            .collect(),
    )
}

#[distributed_slice(RAY_PROGRAM)]
static VARIANCES_EXECUTOR: Registration<Config> =
    Registration::new::<VariancesKey, VariancesExecutor>();

#[executor(config = Config)]
async fn variance_executor(
    &VarianceKey { symbol_id }: &VarianceKey,
    engine: &TrackedEngine,
) -> Interned<VarianceMap> {
    let variances = engine.query(&VariancesKey { target_id: symbol_id.target_id }).await;
    variances.get(&symbol_id.id).cloned().expect("incorrect key")
}

#[distributed_slice(RAY_PROGRAM)]
static VARIANCE_EXECUTOR: Registration<Config> =
    Registration::new::<VarianceKey, VarianceExecutor>();

/// Joins the uses of an owner's undeclared variables in its declaration into
/// its variances, as one step of the fixed point.
struct VarianceInferrer<'a> {
    owner_id: GlobalSymbolID,

    /// The owner's polymorphic variables. A declared variable's variance is
    /// fixed, so its uses are not joined.
    poly_vars: &'a PolyVarMap,

    /// The current variances of every struct and effect in the owner's
    /// target, the owner's own included.
    local: &'a mut FxHashMap<GlobalSymbolID, VarianceMap>,
    engine: &'a TrackedEngine,

    /// The uses found in the type being walked, kept to reuse its buffer.
    uses: Vec<(PolyVarID, Variance)>,

    /// Whether any variance of the owner changed.
    changed: bool,
}

impl<'a> VarianceInferrer<'a> {
    /// Creates an inferrer that joins uses into the variances of `owner_id`
    /// in `local`, which holds the current variances of every owner of its
    /// target.
    const fn new(
        owner_id: GlobalSymbolID,
        poly_vars: &'a PolyVarMap,
        local: &'a mut FxHashMap<GlobalSymbolID, VarianceMap>,
        engine: &'a TrackedEngine,
    ) -> Self {
        Self { owner_id, poly_vars, local, engine, uses: Vec::new(), changed: false }
    }

    /// Joins every use in the owner's declaration into its variances.
    /// Returns whether they changed.
    async fn infer_declaration(mut self, owner: VarianceOwner) -> bool {
        match owner {
            VarianceOwner::Struct => {
                for (_, field) in self.engine.get_struct_body(self.owner_id).await.iter() {
                    self.infer(field.ty(), Variance::Covariant).await;
                }
            }

            // The handler receives the arguments and supplies the result.
            VarianceOwner::Effect => {
                for operation_id in self.engine.get_members(self.owner_id).await.all_ids() {
                    let operation_id = self.owner_id.target_id.make_global(operation_id);
                    for (_, parameter) in self.engine.get_parameter_map(operation_id).await.iter() {
                        self.infer(parameter.ty(), Variance::Covariant).await;
                    }
                    let return_type = self.engine.get_return_type(operation_id).await;
                    self.infer(&return_type, Variance::Contravariant).await;
                }
            }
        }

        self.changed
    }

    /// Joins the uses of the owner's undeclared variables in `root`, which
    /// occurs at a position of variance `ambient`.
    async fn infer(&mut self, root: &Interned<Ty>, ambient: Variance) {
        // Find the uses against the current variances.
        let collector = UseCollector::new(self.owner_id, Some(self.local), self.engine);
        collector.collect(root, ambient, &mut self.uses).await;

        // Then join them, except into a declared variance, which is fixed.
        let own = self.local.get_mut(&self.owner_id).expect("the owner has variances");
        for (id, variance) in self.uses.drain(..) {
            if self.poly_vars[id].declared_variance().is_none() {
                self.changed |= own.join(id, variance);
            }
        }
    }
}

/// Finds the uses of an owner's polymorphic variables in a type, each at the
/// variance of its position.
struct UseCollector<'a> {
    owner_id: GlobalSymbolID,

    /// The variances of the owner's target while the fixed point computes
    /// them, or `None` once they are final. Every other variance is final
    /// and comes from [`get_variance`].
    local: Option<&'a FxHashMap<GlobalSymbolID, VarianceMap>>,
    engine: &'a TrackedEngine,
}

impl<'a> UseCollector<'a> {
    const fn new(
        owner_id: GlobalSymbolID,
        local: Option<&'a FxHashMap<GlobalSymbolID, VarianceMap>>,
        engine: &'a TrackedEngine,
    ) -> Self {
        Self { owner_id, local, engine }
    }

    /// Pushes each use of the owner's variables in `root`, which occurs at a
    /// position of variance `ambient`, onto `uses`.
    async fn collect(
        &self,
        root: &Interned<Ty>,
        ambient: Variance,
        uses: &mut Vec<(PolyVarID, Variance)>,
    ) {
        let mut pending = vec![(root.clone(), ambient)];

        while let Some((ty, variance)) = pending.pop() {
            // Nothing under a bivariant position is a use.
            if variance == Variance::Bivariant {
                continue;
            }

            match &*ty {
                Ty::PolyVar(poly_var) => {
                    if poly_var.parent_id() == self.owner_id {
                        uses.push((poly_var.id(), variance));
                    }
                }

                Ty::Application(application) => {
                    // NOTE: `foreign` is needed because `declared` is a Option<&VarianceMap> and we
                    // need to have a place to keep the foreign VarianceMap alive
                    let foreign;
                    let declared = match application.struct_id() {
                        Some(struct_id) => {
                            if let Some(variances) =
                                self.local.and_then(|local| local.get(&struct_id))
                            {
                                Some(variances)
                            } else {
                                foreign = self.engine.get_variance(struct_id).await;
                                Some(&*foreign)
                            }
                        }
                        None => None,
                    };

                    pending.extend(
                        application
                            .arguments_with_variance(declared)
                            .map(|(arg, position)| (arg.clone(), variance.xform(position))),
                    );
                }

                // Each label's arguments follow its effect's variances, and
                // the tail keeps the row's position.
                Ty::EffectRow(row) => {
                    for label in row.labels() {
                        let effect_id = label.effect_symbol_id();

                        // NOTE: `foreign` is needed because `declared` is a Option<&VarianceMap>
                        // and we need to have a place to keep the foreign
                        // VarianceMap alive
                        let foreign;
                        let declared = if let Some(variances) =
                            self.local.and_then(|local| local.get(&effect_id))
                        {
                            variances
                        } else {
                            foreign = self.engine.get_variance(effect_id).await;
                            &*foreign
                        };

                        pending.extend(
                            label
                                .arguments_with_variance(declared)
                                .map(|(arg, position)| (arg.clone(), variance.xform(position))),
                        );
                    }
                    pending.extend(row.tail().map(|tail| (tail.clone(), variance)));
                }

                Ty::Lifetime(_) | Ty::Inference(_) | Ty::SelfInstance(_) => {}
            }
        }
    }
}

/// The kinds of declarations whose parameters have a computed variance.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    StableHash,
    Encode,
    Decode,
    Identifiable,
)]
pub enum VarianceOwner {
    Struct,
    Effect,
}

impl VarianceOwner {
    /// Returns the owner kind of a symbol, or `None` if its parameters have
    /// no computed variance.
    #[must_use]
    pub const fn of(kind: SymbolKind) -> Option<Self> {
        match kind {
            SymbolKind::Strut => Some(Self::Struct),
            SymbolKind::Effect => Some(Self::Effect),
            SymbolKind::Def
            | SymbolKind::EffectOperation
            | SymbolKind::ExternDef
            | SymbolKind::Instance
            | SymbolKind::InstanceDef
            | SymbolKind::InstanceType
            | SymbolKind::Marker
            | SymbolKind::MarkerImplementation
            | SymbolKind::Module
            | SymbolKind::Trait
            | SymbolKind::TraitDef
            | SymbolKind::TraitType => None,
        }
    }
}

/// Where a use of a declared parameter occurs, for a diagnostic.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Identifiable,
)]
pub enum UsePosition {
    Field { name: Interned<str> },
    OperationParameter { operation: Interned<str> },
    OperationReturn { operation: Interned<str> },
}

impl UsePosition {
    /// Describes the position as the subject of a sentence.
    fn describe(&self) -> String {
        match self {
            Self::Field { name } => format!("field `{}`", &**name),
            Self::OperationParameter { operation } => {
                format!("a parameter of operation `{}`", &**operation)
            }
            Self::OperationReturn { operation } => {
                format!("the return type of operation `{}`", &**operation)
            }
        }
    }
}

/// A parameter declared with a variance, as in `+t`, that a use site uses
/// outside that variance.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Identifiable,
)]
pub struct VarianceMismatch {
    name: Interned<str>,
    declared: Variance,

    /// The join of the parameter's uses in the site.
    used: Variance,
    declaration_span: RelativeSpan,
    position: UsePosition,
    use_span: RelativeSpan,
}

impl Report for VarianceMismatch {
    async fn report(&self, engine: &TrackedEngine) -> Rendered<ByteIndex> {
        let name = &*self.name;
        let (declared, used) = (self.declared.name(), self.used.name());

        // The least variance that allows both the declaration and the uses.
        let suggested = self.declared.join(self.used);
        let marker = suggested.marker().expect("a join with a declared variance is not bivariant");

        Rendered::builder()
            .message(format!("`{name}` is declared {declared} but used {used}ly"))
            .help_message(format!(
                "consider declaring it {} as `{marker}{name}`, or removing the marker to infer \
                 its variance",
                suggested.name()
            ))
            .primary_highlight(
                Highlight::builder()
                    .span(engine.to_absolute_span(&self.use_span).await)
                    .message(format!("{} uses `{name}` {used}ly", self.position.describe()))
                    .build(),
            )
            .related(vec![
                Highlight::builder()
                    .span(engine.to_absolute_span(&self.declaration_span).await)
                    .message(format!("`{name}` is declared {declared} here"))
                    .build(),
            ])
            .build()
    }
}

/// A position in an owner's declaration whose type is checked.
enum UseSite<'a> {
    Field { name: &'a Interned<str>, span: RelativeSpan },
    OperationParameter { operation_id: GlobalSymbolID, span: Option<RelativeSpan> },
    OperationReturn { operation_id: GlobalSymbolID },
}

impl UseSite<'_> {
    /// Describes the site for a diagnostic, with its span if it has one.
    async fn resolve(&self, engine: &TrackedEngine) -> (UsePosition, Option<RelativeSpan>) {
        match self {
            Self::Field { name, span } => {
                (UsePosition::Field { name: (*name).clone() }, Some(*span))
            }
            Self::OperationParameter { operation_id, span } => {
                let operation = engine.get_name(*operation_id).await;
                let span = match span {
                    Some(span) => Some(*span),
                    None => engine.get_span(*operation_id).await,
                };
                (UsePosition::OperationParameter { operation }, span)
            }
            Self::OperationReturn { operation_id } => {
                let operation = engine.get_name(*operation_id).await;
                let span = match engine.get_return_type_syntax(*operation_id).await {
                    Some(return_type) => Some(return_type.span()),
                    None => engine.get_span(*operation_id).await,
                };
                (UsePosition::OperationReturn { operation }, span)
            }
        }
    }
}

/// Checks the uses of an owner's declared variables against their declared
/// variances, one use site at a time, once every variance is final.
struct DeclaredVarianceChecker<'a> {
    owner_id: GlobalSymbolID,
    poly_vars: &'a PolyVarMap,
    engine: &'a TrackedEngine,

    /// The uses found in the site being checked, kept to reuse its buffer.
    uses: Vec<(PolyVarID, Variance)>,
    mismatches: Vec<VarianceMismatch>,
}

impl<'a> DeclaredVarianceChecker<'a> {
    const fn new(
        owner_id: GlobalSymbolID,
        poly_vars: &'a PolyVarMap,
        engine: &'a TrackedEngine,
    ) -> Self {
        Self { owner_id, poly_vars, engine, uses: Vec::new(), mismatches: Vec::new() }
    }

    /// Checks every use site in the owner's declaration. Returns the
    /// mismatches in declaration order.
    async fn check_declaration(mut self, owner: VarianceOwner) -> Vec<VarianceMismatch> {
        match owner {
            VarianceOwner::Struct => {
                for (_, field) in self.engine.get_struct_body(self.owner_id).await.iter() {
                    let site = UseSite::Field { name: field.name(), span: field.span() };
                    self.check(field.ty(), Variance::Covariant, site).await;
                }
            }

            // The handler receives the arguments and supplies the result.
            VarianceOwner::Effect => {
                for operation_id in self.engine.get_members(self.owner_id).await.all_ids() {
                    let operation_id = self.owner_id.target_id.make_global(operation_id);
                    for (_, parameter) in self.engine.get_parameter_map(operation_id).await.iter() {
                        let site =
                            UseSite::OperationParameter { operation_id, span: parameter.span() };
                        self.check(parameter.ty(), Variance::Covariant, site).await;
                    }
                    let return_type = self.engine.get_return_type(operation_id).await;
                    let site = UseSite::OperationReturn { operation_id };
                    self.check(&return_type, Variance::Contravariant, site).await;
                }
            }
        }

        self.mismatches
    }

    /// Records each declared variable that `root`, which occurs at `site` at a
    /// position of variance `ambient`, uses outside its declared variance.
    async fn check(&mut self, root: &Interned<Ty>, ambient: Variance, site: UseSite<'_>) {
        // Every variance is final, so the collector reads them all from
        // `get_variance`.
        let collector = UseCollector::new(self.owner_id, None, self.engine);
        collector.collect(root, ambient, &mut self.uses).await;

        // Join the uses of each declared variable in the site, and describe
        // the site only once it has a mismatch.
        let mut resolved = None;
        for (id, poly_var) in self.poly_vars.iter() {
            let Some(declared) = poly_var.declared_variance() else {
                continue;
            };

            let used = self
                .uses
                .iter()
                .filter(|(use_id, _)| *use_id == id)
                .fold(Variance::Bivariant, |joined, (_, variance)| joined.join(*variance));

            if used.is_within(declared) {
                continue;
            }

            if resolved.is_none() {
                resolved = Some(site.resolve(self.engine).await);
            }
            let (position, span) = resolved.clone().expect("the site was just resolved");
            self.mismatches.push(VarianceMismatch {
                name: poly_var.name().clone(),
                declared,
                used,
                declaration_span: poly_var.span(),
                position,
                use_span: span.unwrap_or_else(|| poly_var.span()),
            });
        }

        self.uses.clear();
    }
}

/// Retrieves the uses of a struct's or `eff`'s declared parameters that are
/// outside their declared variances.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Query,
)]
#[value(Interned<[VarianceMismatch]>)]
pub struct VarianceMismatchKey {
    /// A `SymbolKind::Strut` or `SymbolKind::Effect` symbol.
    pub symbol_id: GlobalSymbolID,
}

#[executor(config = Config)]
async fn variance_mismatch_executor(
    &VarianceMismatchKey { symbol_id }: &VarianceMismatchKey,
    engine: &TrackedEngine,
) -> Interned<[VarianceMismatch]> {
    let owner = VarianceOwner::of(engine.get_symbol_kind(symbol_id).await)
        .expect("only structs and effects have computed variances");

    // Only a declared parameter can be misused, so skip the walk without one.
    let poly_vars = engine.get_poly_var_map(symbol_id).await;
    if poly_vars.iter().all(|(_, poly_var)| poly_var.declared_variance().is_none()) {
        return engine.intern_unsized(Vec::<VarianceMismatch>::new());
    }

    let checker = DeclaredVarianceChecker::new(symbol_id, &poly_vars, engine);
    engine.intern_unsized(checker.check_declaration(owner).await)
}

#[distributed_slice(RAY_PROGRAM)]
static VARIANCE_MISMATCH_EXECUTOR: Registration<Config> =
    Registration::new::<VarianceMismatchKey, VarianceMismatchExecutor>();
