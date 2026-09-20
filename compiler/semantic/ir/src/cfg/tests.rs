use super::*;

fn critical_edges(cfg: &Cfg) -> Vec<ControlFlowEdge> {
    cfg.blocks
        .iter()
        .flat_map(|(block_id, _)| cfg.outgoing_edges(block_id).expect("Block should exist"))
        .filter(|edge| {
            cfg.outgoing_edges(edge.source).expect("Source block should exist").count() >= 2
                && cfg.incoming_edges(edge.target).expect("Target block should exist").count() >= 2
        })
        .collect()
}

#[test]
fn splits_critical_edge_through_an_intermediate_block() {
    let mut cfg = Cfg::new();
    let entry = cfg.entry_block();
    let other = cfg.create_block();
    let merge = cfg.create_block();
    cfg.set_terminator(
        entry,
        Terminator::Conditional(Conditional::new(IRExprID::new(0), merge, other)),
    );
    cfg.set_terminator(other, Terminator::Jump(merge));
    cfg.set_terminator(merge, Terminator::Return(None));

    assert_eq!(cfg.split_critical_edges(), 1);

    let Terminator::Conditional(conditional) = cfg.terminator(entry).unwrap() else {
        panic!("Entry block should remain conditional");
    };
    let split_block = conditional.then_block();
    assert_ne!(split_block, merge);
    assert_eq!(conditional.else_block(), other);
    assert_eq!(cfg.terminator(split_block), Some(&Terminator::Jump(merge)));
    let incoming_sources =
        cfg.incoming_edges(merge).unwrap().map(|edge| edge.source()).collect::<FxHashSet<_>>();
    assert_eq!(incoming_sources, FxHashSet::from_iter([other, split_block]));
    assert!(critical_edges(&cfg).is_empty());
}

#[test]
fn splits_parallel_critical_edges_independently() {
    let mut cfg = Cfg::new();
    let entry = cfg.entry_block();
    let merge = cfg.create_block();
    cfg.set_terminator(
        entry,
        Terminator::Conditional(Conditional::new(IRExprID::new(0), merge, merge)),
    );
    cfg.set_terminator(merge, Terminator::Return(None));

    assert_eq!(cfg.split_critical_edges(), 2);

    let Terminator::Conditional(conditional) = cfg.terminator(entry).unwrap() else {
        panic!("Entry block should remain conditional");
    };
    assert_ne!(conditional.then_block(), conditional.else_block());
    assert_eq!(cfg.terminator(conditional.then_block()), Some(&Terminator::Jump(merge)));
    assert_eq!(cfg.terminator(conditional.else_block()), Some(&Terminator::Jump(merge)));
    let incoming_sources =
        cfg.incoming_edges(merge).unwrap().map(|edge| edge.source()).collect::<FxHashSet<_>>();
    assert_eq!(
        incoming_sources,
        FxHashSet::from_iter([conditional.then_block(), conditional.else_block()])
    );
    assert!(critical_edges(&cfg).is_empty());
}

#[test]
fn leaves_graph_without_critical_edges_unchanged() {
    let mut cfg = Cfg::new();
    let entry = cfg.entry_block();
    let exit = cfg.create_block();
    cfg.set_terminator(entry, Terminator::Jump(exit));
    cfg.set_terminator(exit, Terminator::Return(None));
    let original = cfg.clone();

    assert_eq!(cfg.split_critical_edges(), 0);
    assert_eq!(cfg, original);
}
