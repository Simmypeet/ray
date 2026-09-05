use linkme::distributed_slice;
use qbice::{executor, program::Registration, storage::intern::Interned};
use rayc_hash::FxHashSet;
use rayc_qbice::{Config, RAY_PROGRAM, TrackedEngine};
use rayc_semantic_element::{
    all_instance_implements_trait::AllInstanceImplementsTrait,
    instance_trait_ref::get_instance_trait_ref,
};
use rayc_symbol::{GlobalSymbolID, symbol_kind::get_all_instance_ids};
use rayc_type::{
    poly_var::{GlobalPolyVarID, get_poly_var_map},
    reduce::Reduce,
    ty::{Ty, TyKind},
};

#[executor(config = Config)]
async fn all_instance_implements_trait_executor(
    &AllInstanceImplementsTrait { trait_id, target_id }: &AllInstanceImplementsTrait,
    engine: &TrackedEngine,
) -> Interned<[GlobalSymbolID]> {
    let instance_ids = engine.get_all_instance_ids(target_id).await;
    let mut instances = Vec::new();

    for &id in instance_ids.iter() {
        let symbol_id = target_id.make_global(id);
        let Some(trait_ref) = engine.get_instance_trait_ref(symbol_id).await else {
            continue;
        };
        if trait_ref.trait_id() != trait_id {
            continue;
        }

        let head = trait_ref.normalize(engine);
        if head.contains_error() || head.contains_inference() {
            continue;
        }

        let head_poly_vars: FxHashSet<_> = head
            .args()
            .iter()
            .flat_map(Ty::recursive_iter)
            .filter_map(|ty| match ty {
                Ty::PolyVar(id) => Some(*id),
                Ty::Application(_) | Ty::Inference(_) | Ty::EffectRow(_) => None,
            })
            .collect();
        let parameters = engine.get_poly_var_map(symbol_id).await;
        // Head matching must determine every ordinary argument before given
        // premises can be resolved. Occurrences in those premises do not count.
        let eligible = parameters.iter().all(|(id, parameter)| match parameter.kind() {
            TyKind::Instance => true,
            TyKind::Star | TyKind::EffectRow => {
                head_poly_vars.contains(&GlobalPolyVarID::new(symbol_id, id))
            }
        });
        if eligible {
            instances.push(symbol_id);
        }
    }

    engine.intern_unsized(instances)
}

#[distributed_slice(RAY_PROGRAM)]
static ALL_INSTANCE_IMPLEMENTS_TRAIT_EXECUTOR: Registration<Config> =
    Registration::new::<AllInstanceImplementsTrait, AllInstanceImplementsTraitExecutor>();
