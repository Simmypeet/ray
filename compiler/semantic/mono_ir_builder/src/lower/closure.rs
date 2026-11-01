use rayc_ir::ir_expr::{IRExprID, closure::Closure};

use crate::function_lowering::FunctionLowering;

impl FunctionLowering<'_> {
    pub(super) fn lower_closure(&mut self, closure: &Closure, expression_id: IRExprID) {
        // Captures have already been evaluated in semantic IR order.
        let environment_abi = self.function_environment_abi(closure.function_id());
        let fields =
            closure.captures().iter().map(|capture| self.expression_operand(*capture)).collect();

        // Effect handlers are passed as additional arguments to the function,
        // not embedded in the environment.
        assert_eq!(environment_abi.captured_effects().len(), 0);

        let destination = self.expression_place(expression_id);
        self.initialize_environment(&destination, fields);
    }
}
