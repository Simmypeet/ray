use rayc_ir::ir_expr::IRExprID;

#[derive(Debug, Clone, Copy)]
pub struct ExpressionWithID<E> {
    node: E,
    id: IRExprID,
}

impl<E> ExpressionWithID<E> {
    pub const fn new(node: E, id: IRExprID) -> Self { Self { node, id } }

    pub const fn id(&self) -> IRExprID { self.id }

    pub const fn node(&self) -> E
    where
        E: Copy,
    {
        self.node
    }
}
