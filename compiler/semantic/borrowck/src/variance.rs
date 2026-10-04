//! The variance of every lifetime in the IR, which orients the liveness edges
//! of its region. A lifetime at several positions has the join of them.

use qbice::storage::intern::Interned;
use rayc_hash::{FxHashMap, FxHashSet};
use rayc_ir::ir_function::IRFunctionMap;
use rayc_qbice::TrackedEngine;
use rayc_type::{ty::Ty, variance::Variance};

/// The variance of every lifetime in the types of an [`IRFunctionMap`].
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LifetimeVariances {
    /// The join of the variances of every position each lifetime occurs at.
    variances: FxHashMap<Interned<Ty>, Variance>,
}

impl LifetimeVariances {
    /// Computes the variance of every lifetime in the types stored in `ir`.
    pub async fn compute(ir: &IRFunctionMap, engine: &TrackedEngine) -> Self {
        // Collect the distinct types first, since the visitor cannot await.
        let mut types = FxHashSet::default();
        ir.visit_types(&mut |ty: &Interned<Ty>, _| {
            types.insert(ty.clone());
        });

        // Walk each type from a covariant position.
        let mut collector = VarianceCollector { engine, variances: FxHashMap::default() };
        for ty in types {
            collector.walk(ty, Variance::Covariant).await;
        }

        Self { variances: collector.variances }
    }

    /// Returns the variance of a lifetime, or `None` if it does not occur in
    /// any type of the IR.
    #[must_use]
    pub fn get(&self, lifetime: &Interned<Ty>) -> Option<Variance> {
        self.variances.get(lifetime).copied()
    }

    /// Iterates over every lifetime in the IR together with its variance.
    #[must_use]
    pub fn iter(&self) -> impl ExactSizeIterator<Item = (&Interned<Ty>, Variance)> {
        self.variances.iter().map(|(lifetime, variance)| (lifetime, *variance))
    }
}

/// Joins the variance of every lifetime position in the walked types.
struct VarianceCollector<'e> {
    engine: &'e TrackedEngine,
    variances: FxHashMap<Interned<Ty>, Variance>,
}

impl VarianceCollector<'_> {
    /// Joins the variance of every lifetime in `root`, which occurs at a
    /// position of variance `ambient`.
    async fn walk(&mut self, root: Interned<Ty>, ambient: Variance) {
        let mut pending = vec![(root, ambient)];

        while let Some((ty, variance)) = pending.pop() {
            match &*ty {
                Ty::Lifetime(_) => self.join(&ty, variance),

                // A lifetime parameter is a lifetime; any other poly var
                // contains none.
                Ty::PolyVar(_) => {
                    if ty.is_lifetime(self.engine).await {
                        self.join(&ty, variance);
                    }
                }

                Ty::Application(application) => {
                    // An error or a projection of kind lifetime is a
                    // lifetime position as well.
                    if ty.is_lifetime(self.engine).await {
                        self.join(&ty, variance);
                    }

                    pending.extend(
                        application
                            .arguments_with_ambient_variance(variance, self.engine)
                            .await
                            .map(|(arg, position)| (arg.clone(), position)),
                    );
                }

                // Each label's arguments follow its effect's variances, and
                // the tail keeps the row's position.
                Ty::EffectRow(row) => {
                    for label in row.labels() {
                        pending.extend(
                            label
                                .arguments_with_ambient_variance(variance, self.engine)
                                .await
                                .map(|(arg, position)| (arg.clone(), position)),
                        );
                    }
                    pending.extend(row.tail().map(|tail| (tail.clone(), variance)));
                }

                // Neither holds a lifetime: there are no inference variables
                // on the IR, and a self instance has no arguments of its own.
                Ty::Inference(_) | Ty::SelfInstance(_) => {}
            }
        }
    }

    /// Joins `variance` into the variance of `lifetime`.
    fn join(&mut self, lifetime: &Interned<Ty>, variance: Variance) {
        self.variances
            .entry(lifetime.clone())
            .and_modify(|joined| *joined = joined.join(variance))
            .or_insert(variance);
    }
}
