//! Structural discovery through the ordinary core symbol table.
use linkme::distributed_slice;
use qbice::{executor, program::Registration};
use rayc_qbice::{Config, RAY_PROGRAM, TrackedEngine};
use rayc_symbol::{
    GlobalSymbolID, calculate_core_root_target_module_id,
    core_item::{CoreItem, Key},
    member::get_members,
    name::get_name,
    symbol_kind::{SymbolKind, get_symbol_kind},
};
use rayc_target::TargetID;

async fn find(
    engine: &TrackedEngine,
    role: CoreItem,
    parent: GlobalSymbolID,
    name: &str,
    expected: SymbolKind,
) -> GlobalSymbolID {
    let members = engine.get_members(parent).await;

    // Include redefinitions kept in the ordinary table's unnamed member set.
    let mut matches = Vec::new();
    for id in members.all_ids() {
        let id = TargetID::CORE.make_global(id);
        if engine.get_name(id).await.as_ref() == name {
            matches.push(id);
        }
    }

    assert_eq!(matches.len(), 1, "invalid core item {role:?}: expected one {name} declaration");
    let id = matches[0];
    let actual = engine.get_symbol_kind(id).await;
    assert_eq!(actual, expected, "invalid core item {role:?}: wrong symbol kind for {name}");

    id
}

#[executor(config = Config)]
async fn core_item_executor(key: &Key, engine: &TrackedEngine) -> GlobalSymbolID {
    let root = TargetID::CORE.make_global(calculate_core_root_target_module_id());

    match key.role {
        CoreItem::DefTrait => find(engine, key.role, root, "Def", SymbolKind::Trait).await,

        CoreItem::DropTrait => find(engine, key.role, root, "Drop", SymbolKind::Trait).await,
        CoreItem::DropMethod => {
            let def = find(engine, key.role, root, "Drop", SymbolKind::Trait).await;
            find(engine, key.role, def, "drop", SymbolKind::TraitDef).await
        }
        CoreItem::NoDropStruct => find(engine, key.role, root, "NoDrop", SymbolKind::Strut).await,

        CoreItem::DefCall | CoreItem::DefArgs | CoreItem::DefReturn | CoreItem::DefEffect => {
            let def = find(engine, key.role, root, "Def", SymbolKind::Trait).await;
            match key.role {
                CoreItem::DefCall => {
                    find(engine, key.role, def, "call", SymbolKind::TraitDef).await
                }
                CoreItem::DefArgs => {
                    find(engine, key.role, def, "Args", SymbolKind::TraitType).await
                }
                CoreItem::DefReturn => {
                    find(engine, key.role, def, "Return", SymbolKind::TraitType).await
                }
                CoreItem::DefEffect => {
                    find(engine, key.role, def, "Effect", SymbolKind::TraitType).await
                }

                CoreItem::DropMethod
                | CoreItem::DropTrait
                | CoreItem::DefTrait
                | CoreItem::Copy
                | CoreItem::NoDropStruct => {
                    unreachable!()
                }
            }
        }
        CoreItem::Copy => find(engine, key.role, root, "Copy", SymbolKind::Marker).await,
    }
}

#[distributed_slice(RAY_PROGRAM)]
static CORE_ITEM_EXECUTOR: Registration<Config> = Registration::new::<Key, CoreItemExecutor>();
