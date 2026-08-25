use rayc_ir::ir_expr::ExpressionID;

#[derive(Debug, Clone, Copy)]
pub struct ExpressionWithID<E> {
    node: E,
    id: ExpressionID,
}

impl<E> ExpressionWithID<E> {
    pub const fn new(node: E, id: ExpressionID) -> Self { Self { node, id } }

    pub const fn id(&self) -> ExpressionID { self.id }

    pub const fn node(&self) -> E
    where
        E: Copy,
    {
        self.node
    }
}
