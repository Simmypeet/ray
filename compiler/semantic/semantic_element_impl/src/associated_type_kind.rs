use linkme::distributed_slice;
use qbice::{executor, program::Registration};
use rayc_qbice::{Config, RAY_PROGRAM, TrackedEngine};
use rayc_symbol::syntax::get_kind_ascription_syntax;
use rayc_syntax::kind::{Kind, KindAscription};
use rayc_type::{associated_type_kind::Key, ty::TyKind};

pub(crate) fn resolve_kind(ascription: Option<KindAscription>) -> TyKind {
    match ascription.and_then(|ascription| ascription.kind()) {
        Some(Kind::Effect(_)) => TyKind::EffectRow,
        Some(Kind::Star(_)) | None => TyKind::Star,
    }
}

#[executor(config = Config)]
async fn associated_type_kind_executor(&Key { symbol_id }: &Key, engine: &TrackedEngine) -> TyKind {
    // Read only declaration syntax so opaque and recursive projections have a kind.
    resolve_kind(engine.get_kind_ascription_syntax(symbol_id).await)
}

#[distributed_slice(RAY_PROGRAM)]
static ASSOCIATED_TYPE_KIND_EXECUTOR: Registration<Config> =
    Registration::new::<Key, AssociatedTypeKindExecutor>();
