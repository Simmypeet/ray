use std::io;

use rayc_ir::function::{Context as FunctionContext, FunctionMap};
use rayc_mono::{MonoFunction, MonoFunctionKind, MonoTuple};

use crate::{
    context::{
        Context,
        instantiation::{CLambdaTypeID, CTupleID, MonoFunctionSubstID},
    },
    identifier::Identifier,
};

impl Context {
    const TAB: &'static str = "    ";

    pub fn write_forward_decl_tuples(&self, buf: &mut impl io::Write) -> io::Result<()> {
        for (id, _) in self.ctuple_instances() {
            write!(buf, "typedef struct ")?;
            self.write_ctuple_struct(id, buf)?;
            write!(buf, " ")?;
            self.write_ctuple_t(id, buf)?;
            writeln!(buf, ";")?;
        }

        Ok(())
    }

    pub fn write_forward_decl_lambda_types(&self, buf: &mut impl io::Write) -> io::Result<()> {
        for (id, _) in self.clambda_type_instances() {
            write!(buf, "typedef struct ")?;
            self.write_clambda_struct(id, buf)?;
            write!(buf, " ")?;
            self.write_clambda_t(id, buf)?;
            writeln!(buf, ";")?;
        }
        Ok(())
    }

    pub fn write_lambda_type_defs(&self, buf: &mut impl io::Write) -> io::Result<()> {
        for (id, lambda_type) in self.clambda_type_instances() {
            self.write_lambda_type_def(id, lambda_type, buf)?;
            writeln!(buf)?;
        }
        Ok(())
    }

    pub fn write_tuple_struct_defs(&self, buf: &mut impl io::Write) -> io::Result<()> {
        for (id, tuple) in self.ctuple_instances() {
            self.write_tuple_struct_def(id, tuple, buf)?;
            writeln!(buf)?;
        }

        Ok(())
    }

    pub async fn write_mono_function_decl(
        &self,
        function: &MonoFunction,
        buf: &mut impl io::Write,
    ) -> io::Result<()> {
        match function.kind() {
            MonoFunctionKind::Def => {
                let return_type = self.get_mono_return_cty(function).await;
                self.write_cty(&return_type, buf)?;
                write!(buf, " ")?;
                self.write_mono_function_name(function, buf).await?;

                write!(buf, "(")?;
                let parameters = self.get_mono_parameters(function).await;
                let mut first = true;

                for (parameter_id, parameter_type) in parameters {
                    if !first {
                        write!(buf, ", ")?;
                    }

                    self.write_cty(&parameter_type, buf)?;
                    write!(buf, " {}", Identifier::param(parameter_id))?;

                    first = false;
                }

                if first {
                    write!(buf, "void")?;
                }

                write!(buf, ")")
            }
            MonoFunctionKind::Lambda(function_id) => {
                let functions = self.get_ir(function.def_id()).await;
                let lambda = match functions.get_function(function_id).context() {
                    FunctionContext::Def => panic!(
                        "compiler-internal invariant violation: monomorphized lambda {} selects a \
                         def",
                        function_id.index()
                    ),
                    FunctionContext::Lambda(lambda) => lambda,
                };

                let return_type = self.instantiate_type(lambda.return_ty(), function);
                let return_type = self.ty_to_cty(&return_type);
                self.write_cty(&return_type, buf)?;
                write!(buf, " ")?;
                self.write_mono_function_name(function, buf).await?;
                write!(buf, "(void *{}", Identifier::lambda_raw_environment())?;

                for (parameter_id, parameter) in lambda.parameters() {
                    write!(buf, ", ")?;
                    let parameter_type = self.instantiate_type(parameter.ty(), function);
                    let parameter_type = self.ty_to_cty(&parameter_type);
                    self.write_cty(&parameter_type, buf)?;
                    write!(buf, " {}", Identifier::lambda_param(parameter_id))?;
                }

                write!(buf, ")")
            }
        }
    }

    pub(crate) async fn write_mono_function_name(
        &self,
        function: &MonoFunction,
        buf: &mut impl io::Write,
    ) -> io::Result<()> {
        let name = self.get_def_name(function.def_id()).await;
        let subst = MonoFunctionSubstID::for_function(function);
        match function.kind() {
            MonoFunctionKind::Def => write!(buf, "{}", Identifier::def(&name, subst)),
            MonoFunctionKind::Lambda(function_id) => {
                write!(buf, "{}", Identifier::lambda_def(&name, subst, function_id))
            }
        }
    }

    pub(crate) async fn write_lambda_environment_name(
        &self,
        function: &MonoFunction,
        buf: &mut impl io::Write,
    ) -> io::Result<()> {
        let MonoFunctionKind::Lambda(function_id) = function.kind() else {
            panic!("compiler-internal invariant violation: a def has no lambda environment");
        };
        let name = self.get_def_name(function.def_id()).await;
        let subst = MonoFunctionSubstID::for_function(function);
        write!(buf, "{}", Identifier::lambda_environment(&name, subst, function_id))
    }

    pub async fn write_lambda_environment_defs(&self, buf: &mut impl io::Write) -> io::Result<()> {
        for function in self.mono_function_instances() {
            let MonoFunctionKind::Lambda(function_id) = function.kind() else {
                continue;
            };
            let functions = self.get_ir(function.def_id()).await;
            let lambda = match functions.get_function(function_id).context() {
                FunctionContext::Def => panic!(
                    "compiler-internal invariant violation: monomorphized lambda {} selects a def",
                    function_id.index()
                ),
                FunctionContext::Lambda(lambda) => lambda,
            };
            if lambda.captures().next().is_none() {
                continue;
            }

            self.write_lambda_environment_def(function, &functions, buf).await?;
            writeln!(buf)?;
            writeln!(buf)?;
        }
        Ok(())
    }

    async fn write_lambda_environment_def(
        &self,
        function: &MonoFunction,
        functions: &FunctionMap,
        buf: &mut impl io::Write,
    ) -> io::Result<()> {
        let MonoFunctionKind::Lambda(function_id) = function.kind() else {
            panic!("compiler-internal invariant violation: a def has no lambda environment");
        };
        let lambda = match functions.get_function(function_id).context() {
            FunctionContext::Def => panic!(
                "compiler-internal invariant violation: monomorphized lambda {} selects a def",
                function_id.index()
            ),
            FunctionContext::Lambda(lambda) => lambda,
        };
        assert!(
            lambda.captures().next().is_some(),
            "compiler-internal invariant violation: captureless lambda has an environment struct"
        );

        write!(buf, "struct ")?;
        self.write_lambda_environment_name(function, buf).await?;
        writeln!(buf, " {{")?;
        for (capture_id, capture) in lambda.captures() {
            write!(buf, "{}", Self::TAB)?;
            let pointer_type = self.instantiate_capture_pointer_type(capture, function);
            let pointer_type = self.ty_to_cty(&pointer_type);
            self.write_cty(&pointer_type, buf)?;
            writeln!(buf, " {};", Identifier::capture_field(capture_id))?;
        }
        write!(buf, "}};")
    }

    pub fn write_tuple_struct_def(
        &self,
        id: CTupleID,
        tuple: &MonoTuple,
        buf: &mut impl io::Write,
    ) -> io::Result<()> {
        write!(buf, "struct ")?;
        self.write_ctuple_struct(id, buf)?;

        writeln!(buf, " {{")?;

        if tuple.args().next().is_none() {
            write!(buf, "{}", Self::TAB)?;
            writeln!(buf, "uint8_t {};", Identifier::unit_field())?;
        }

        for (i, arg) in tuple.args().enumerate() {
            write!(buf, "{}", Self::TAB)?;
            let cty = self.ty_to_cty(arg);
            self.write_cty(&cty, buf)?;

            writeln!(buf, " {};", Identifier::tuple_elem(i))?;
        }

        write!(buf, "}};")
    }

    pub fn write_lambda_type_def(
        &self,
        id: CLambdaTypeID,
        lambda_type: &rayc_mono::MonoLambdaType,
        buf: &mut impl io::Write,
    ) -> io::Result<()> {
        write!(buf, "struct ")?;
        self.write_clambda_struct(id, buf)?;
        writeln!(buf, " {{")?;
        write!(buf, "{}", Self::TAB)?;
        let return_type = self.ty_to_cty(lambda_type.return_type());
        self.write_cty(&return_type, buf)?;
        write!(
            buf,
            " (*{})(void *{}",
            Identifier::lambda_call_field(),
            Identifier::lambda_env_field()
        )?;
        for parameter_type in lambda_type.parameter_types() {
            write!(buf, ", ")?;
            let parameter_type = self.ty_to_cty(parameter_type);
            self.write_cty(&parameter_type, buf)?;
        }
        writeln!(buf, ");")?;
        writeln!(buf, "{}void *{};", Self::TAB, Identifier::lambda_env_field())?;
        write!(buf, "}};")
    }
}
