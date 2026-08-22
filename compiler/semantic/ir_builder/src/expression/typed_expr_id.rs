use rayc_typed_ast::typed_expr::TypedExprID;

#[derive(Debug, Clone, Copy)]
pub struct TypedExprWithID<E> {
    node: E,
    id: TypedExprID,
}

impl<E> TypedExprWithID<E> {
    pub const fn new(node: E, id: TypedExprID) -> Self { Self { node, id } }

    pub const fn id(&self) -> TypedExprID { self.id }

    pub const fn node(&self) -> E
    where
        E: Copy,
    {
        self.node
    }
}
