use rayc_typed_ast::typed_expr::TypedExprID;

pub mod statement;

pub trait Bind<S> {
    #[allow(async_fn_in_trait)]
    async fn bind(&mut self, syn: S) -> TypedExprID;
}
