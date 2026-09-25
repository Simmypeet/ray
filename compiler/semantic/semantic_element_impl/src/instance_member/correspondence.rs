//! Pairs the polymorphic variables of a trait member with those of its
//! implementation.

use qbice::storage::intern::Interned;
use rayc_hash::{FxHashMap, FxHashSet};
use rayc_qbice::TrackedEngine;
use rayc_semantic_element::{
    effect_row::get_effect_row,
    parameter::{ParameterMap, get_parameter_map},
    return_type::get_return_type,
};
use rayc_symbol::{
    GlobalSymbolID,
    parent::get_parent_global,
    symbol_kind::{SymbolKind, get_symbol_kind},
    syntax::get_type_parameter_list_syntax,
};
use rayc_type::{
    poly_var::{GlobalPolyVarID, PolyVarID, PolyVarMap, build_subst_from_args, get_poly_var_map},
    subst::{Subst, Substitutable},
    trait_ref::TraitRef,
    ty::{Ty, args::Args, self_instance::SelfInstance},
};

use super::diagnostic::{Compatibility, Mismatch};

/// How the type, effect and lifetime parameters of a trait member correspond
/// to those of its implementation. Dictionaries always correspond in
/// declaration order.
///
/// | Trait method           | Implementation | Style               |
/// |------------------------|----------------|---------------------|
/// | implicit               | any            | [`Self::Signature`] |
/// | explicit, pairable     | implicit       | [`Self::Signature`] |
/// | explicit, not pairable | implicit       | none: an error      |
/// | explicit               | explicit       | [`Self::Sequence`]  |
///
/// A method is *pairable* when its signature mentions every one of its
/// variables. A method that discovers its variables is always pairable.
enum PairingStyle {
    /// In declaration order. Both members declare their parameters, so the
    /// order is the one the programmer wrote; associated types always do.
    Sequence,

    /// By where the variables occur in the two signatures, independently of
    /// their names and order: for `def pair[a, b](first: a, second: b)` in the
    /// trait, `def pair(first: y, second: x)` implements it with `a` as `y` and
    /// `b` as `x`. Every variable must be paired this way.
    Signature { expected: MethodSignature, actual: MethodSignature },
}

impl PairingStyle {
    /// Chooses how the variables of `trait_member_id` correspond to those of
    /// `instance_member_id`.
    ///
    /// Returns the trait variable that makes pairing impossible when the trait
    /// method declares a variable its signature does not mention and the
    /// implementation discovers its variables.
    async fn new(
        engine: &TrackedEngine,
        trait_member_id: GlobalSymbolID,
        instance_member_id: GlobalSymbolID,
        trait_poly_vars: &PolyVarMap,
    ) -> Result<Self, PolyVarID> {
        // Associated types always declare their parameters, and so do methods
        // that both list theirs.
        if engine.get_symbol_kind(instance_member_id).await != SymbolKind::InstanceDef {
            return Ok(Self::Sequence);
        }
        let trait_explicit = engine.get_type_parameter_list_syntax(trait_member_id).await.is_some();
        let instance_explicit =
            engine.get_type_parameter_list_syntax(instance_member_id).await.is_some();
        if trait_explicit && instance_explicit {
            return Ok(Self::Sequence);
        }

        // A trait method that discovers its variables mentions all of them.
        // One that declares them must, for an implementation that discovers
        // its own to pair with them.
        let expected = MethodSignature::new(engine, trait_member_id).await;
        if trait_explicit
            && let Some((id, _)) = trait_poly_vars
                .type_parameters()
                .find(|(id, _)| !expected.mentions(GlobalPolyVarID::new(trait_member_id, *id)))
        {
            return Err(id);
        }

        Ok(Self::Signature {
            expected,
            actual: MethodSignature::new(engine, instance_member_id).await,
        })
    }
}

/// The types of a method signature by which its variables correspond.
struct MethodSignature {
    parameters: Interned<ParameterMap>,
    return_type: Interned<Ty>,
    effect_row: Interned<Ty>,
}

impl MethodSignature {
    async fn new(engine: &TrackedEngine, member_id: GlobalSymbolID) -> Self {
        Self {
            parameters: engine.get_parameter_map(member_id).await,
            return_type: engine.get_return_type(member_id).await,
            effect_row: engine.get_effect_row(member_id).await,
        }
    }

    /// Returns whether any type of the signature mentions `poly_var`.
    fn mentions(&self, poly_var: GlobalPolyVarID) -> bool {
        self.parameters
            .iter()
            .map(|(_, parameter)| parameter.ty())
            .chain([&self.return_type, &self.effect_row])
            .any(|ty| ty.recursive_iter().any(|ty| ty.as_poly_var() == Some(&poly_var)))
    }
}

/// Pairs the polymorphic variables of a trait member with those of its
/// implementation, one to one; see [`PairingStyle`].
struct Correspondence<'a> {
    trait_member_id: GlobalSymbolID,
    instance_member_id: GlobalSymbolID,
    trait_poly_vars: &'a PolyVarMap,
    instance_poly_vars: &'a PolyVarMap,

    pairs: FxHashMap<PolyVarID, PolyVarID>,

    // NOTE: we need an extra set to track the instance-side variables that have
    // been paired, because the `pairs` keyset tracks the trait-side variables.
    paired_instance_poly_vars: FxHashSet<PolyVarID>,
}

impl<'a> Correspondence<'a> {
    fn new(
        trait_member_id: GlobalSymbolID,
        instance_member_id: GlobalSymbolID,
        trait_poly_vars: &'a PolyVarMap,
        instance_poly_vars: &'a PolyVarMap,
    ) -> Self {
        Self {
            trait_member_id,
            instance_member_id,
            trait_poly_vars,
            instance_poly_vars,
            pairs: FxHashMap::default(),
            paired_instance_poly_vars: FxHashSet::default(),
        }
    }

    /// Pairs the type parameters in declaration order, whatever their kinds;
    /// a kind mismatch is reported afterwards.
    fn pair_type_parameters_in_sequence(&mut self) {
        for ((expected, _), (actual, _)) in
            self.trait_poly_vars.type_parameters().zip(self.instance_poly_vars.type_parameters())
        {
            self.pairs.insert(expected, actual);
            self.paired_instance_poly_vars.insert(actual);
        }
    }

    /// Pairs the dictionaries in declaration order.
    fn pair_dictionaries(&mut self) {
        for ((expected, _), (actual, _)) in
            self.trait_poly_vars.dictionaries().zip(self.instance_poly_vars.dictionaries())
        {
            self.pairs.insert(expected, actual);
            self.paired_instance_poly_vars.insert(actual);
        }
    }

    /// Pairs the type parameters that occur at the same positions of the two
    /// method signatures. `substitution` moves the trait's own variables and
    /// the paired dictionaries into the instance, so only the members' local
    /// type parameters remain to be paired.
    fn pair_signatures(
        &mut self,
        engine: &TrackedEngine,
        expected: &MethodSignature,
        actual: &MethodSignature,
        substitution: &Subst,
    ) {
        // TODO: Do we have to normalize the types before pairing? My thought
        // is that we do, because the when trait's signature are
        // applied with substitution to the instance's signature, the
        // types may be further reduced and may affect the pairing.
        for ((_, expected), (_, actual)) in expected.parameters.iter().zip(actual.parameters.iter())
        {
            self.pair_types(&expected.ty().apply_subst_or_clone(substitution, engine), actual.ty());
        }
        self.pair_types(
            &expected.return_type.apply_subst_or_clone(substitution, engine),
            &actual.return_type,
        );
        self.pair_types(
            &expected.effect_row.apply_subst_or_clone(substitution, engine),
            &actual.effect_row,
        );
    }

    /// Pairs the variables at the same positions of `expected` and `actual`,
    /// descending while both have the same shape. A difference in shape is
    /// reported later, when the signatures are compared.
    fn pair_types(&mut self, expected: &Ty, actual: &Ty) {
        match expected {
            Ty::PolyVar(expected) => {
                if let Ty::PolyVar(actual) = actual {
                    self.pair(*expected, *actual);
                }
            }
            Ty::Application(expected) => {
                if let Ty::Application(actual) = actual
                    && let Some(arguments) = expected.structural_match(actual)
                {
                    for (expected, actual) in arguments {
                        self.pair_types(expected, actual);
                    }
                }
            }
            Ty::EffectRow(expected) => {
                let Ty::EffectRow(actual) = actual else { return };

                // Labels match as scoped labels, as row unification matches
                // them; see `EffectRow::match_labels`.
                let labels = expected.match_labels(actual);
                for (expected, actual) in labels.matched() {
                    for (expected, actual) in
                        expected.structural_match(actual).into_iter().flatten()
                    {
                        self.pair_types(expected, actual);
                    }
                }

                // The tails correspond only when every label matches. Otherwise
                // one tail stands for the other row's remaining labels as
                // well, which is not a variable to pair with.
                if labels.is_exact()
                    && let (Some(expected), Some(actual)) = (expected.tail(), actual.tail())
                {
                    self.pair_types(expected, actual);
                }
            }
            Ty::Inference(_) | Ty::SelfInstance(_) | Ty::Lifetime(_) => {}
        }
    }

    /// Pairs two member-local type parameters of the same kind, unless either
    /// is already paired.
    fn pair(&mut self, expected: GlobalPolyVarID, actual: GlobalPolyVarID) {
        if expected.parent_id() != self.trait_member_id
            || actual.parent_id() != self.instance_member_id
            || self.pairs.contains_key(&expected.id())
            || self.paired_instance_poly_vars.contains(&actual.id())
            || self.trait_poly_vars.kind_of(expected.id())
                != self.instance_poly_vars.kind_of(actual.id())
        {
            return;
        }

        self.pairs.insert(expected.id(), actual.id());
        self.paired_instance_poly_vars.insert(actual.id());
    }

    /// Reports every trait variable without a counterpart, since none is
    /// paired implicitly, and every pair of different kinds. Returns whether
    /// the correspondence is complete and well-kinded.
    fn check(&self, compatibility: &Compatibility<'_>) -> bool {
        let mut compatible = true;

        // Every trait variable must have a counterpart.
        for (id, poly_var) in self.trait_poly_vars.iter() {
            if !self.pairs.contains_key(&id) {
                compatibility.report_at(
                    Mismatch::UnpairedPolyVar { name: poly_var.name().clone() },
                    poly_var.span(),
                    compatibility.instance_span(),
                );
                compatible = false;
            }
        }

        // Corresponding variables must have the same kind.
        for (index, (trait_id, instance_id)) in self.pairs().enumerate() {
            let expected = &self.trait_poly_vars[trait_id];
            let actual = &self.instance_poly_vars[instance_id];
            if expected.kind() != actual.kind() {
                compatibility.report_at(
                    Mismatch::PolyVarKind {
                        index,
                        expected: expected.kind(),
                        actual: actual.kind(),
                    },
                    expected.span(),
                    actual.span(),
                );
                compatible = false;
            }
        }

        compatible
    }

    /// Composes the mapping from each trait variable to its counterpart into
    /// `substitution`.
    fn compose_into(&self, substitution: &mut Subst, engine: &TrackedEngine) {
        let local_substitution = self
            .pairs()
            .map(|(trait_id, instance_id)| {
                (
                    GlobalPolyVarID::new(self.trait_member_id, trait_id),
                    Ty::new_poly_var(
                        GlobalPolyVarID::new(self.instance_member_id, instance_id),
                        engine,
                    ),
                )
            })
            .collect();
        substitution.compose(&local_substitution, engine);
    }

    /// Returns every paired trait variable, in declaration order, with its
    /// implementation variable.
    fn pairs(&self) -> impl Iterator<Item = (PolyVarID, PolyVarID)> + '_ {
        self.trait_poly_vars.iter().filter_map(|(id, _)| Some((id, *self.pairs.get(&id)?)))
    }
}

pub(super) async fn poly_var_substitution(
    engine: &TrackedEngine,
    trait_ref: &TraitRef,
    trait_member_id: GlobalSymbolID,
    instance_member_id: GlobalSymbolID,
    compatibility: &Compatibility<'_>,
) -> Option<Subst> {
    let trait_poly_vars = engine.get_poly_var_map(trait_member_id).await;
    let instance_poly_vars = engine.get_poly_var_map(instance_member_id).await;

    // Choose how the members' own variables correspond; see [`PairingStyle`].
    // A trait variable that cannot be paired explains a count mismatch too,
    // so it is reported first.
    let style = match PairingStyle::new(
        engine,
        trait_member_id,
        instance_member_id,
        &trait_poly_vars,
    )
    .await
    {
        Ok(style) => style,
        Err(id) => {
            let poly_var = &trait_poly_vars[id];
            compatibility.report_at(
                Mismatch::ExplicitTypeParametersRequired { name: poly_var.name().clone() },
                poly_var.span(),
                compatibility.instance_span(),
            );
            return None;
        }
    };

    // Both members must have as many type parameters and as many dictionaries.
    if !check_poly_var_counts(&trait_poly_vars, &instance_poly_vars, compatibility) {
        return None;
    }

    // Pair every variable, then check that the pairing is complete.
    let mut substitution = trait_substitution(engine, trait_ref, instance_member_id).await;
    let mut correspondence = Correspondence::new(
        trait_member_id,
        instance_member_id,
        &trait_poly_vars,
        &instance_poly_vars,
    );
    correspondence.pair_dictionaries();
    match &style {
        PairingStyle::Sequence => correspondence.pair_type_parameters_in_sequence(),
        PairingStyle::Signature { expected, actual } => {
            // The dictionaries are paired first, so that the signatures'
            // projections through them, such as `d.Item`, match.
            let mut known = substitution.clone();
            correspondence.compose_into(&mut known, engine);
            correspondence.pair_signatures(engine, expected, actual, &known);
        }
    }
    if !correspondence.check(compatibility) {
        return None;
    }
    correspondence.compose_into(&mut substitution, engine);

    Some(substitution)
}

/// Returns the substitution that maps the trait's own variables to the
/// arguments the instance of `instance_member_id` gives the trait, and `Self`
/// to that instance.
async fn trait_substitution(
    engine: &TrackedEngine,
    trait_ref: &TraitRef,
    instance_member_id: GlobalSymbolID,
) -> Subst {
    let mut substitution =
        engine.build_subst_from_args(trait_ref.trait_id(), trait_ref.args()).await;

    let instance_id = engine.get_parent_global(instance_member_id).await.expect("instance parent");
    let instance_parameters = engine.get_poly_var_map(instance_id).await;
    let identity = Args::new(
        instance_parameters
            .iter()
            .map(|(id, _)| Ty::new_poly_var(GlobalPolyVarID::new(instance_id, id), engine)),
        engine,
    );
    substitution.insert(
        SelfInstance::new(trait_ref.trait_id()),
        Ty::new_instance(instance_id, identity, engine),
    );

    substitution
}

/// Reports whether the members have different numbers of type parameters or
/// of dictionaries, and returns whether the counts agree.
fn check_poly_var_counts(
    trait_poly_vars: &PolyVarMap,
    instance_poly_vars: &PolyVarMap,
    compatibility: &Compatibility<'_>,
) -> bool {
    let expected = trait_poly_vars.type_parameters().count();
    let actual = instance_poly_vars.type_parameters().count();
    if expected != actual {
        compatibility.report(Mismatch::PolyVarCount { expected, actual });
        return false;
    }

    let expected = trait_poly_vars.dictionaries().count();
    let actual = instance_poly_vars.dictionaries().count();
    if expected != actual {
        // Callable parameters bring hidden dictionaries, which the diagnostic
        // explains.
        let callable = trait_poly_vars
            .dictionaries()
            .chain(instance_poly_vars.dictionaries())
            .any(|(_, poly_var)| !poly_var.is_source());
        compatibility.report(Mismatch::GivenParameterCount { expected, actual, callable });
        return false;
    }

    true
}
