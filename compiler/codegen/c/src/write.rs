use std::io;

use rayc_mono::{MonoFunction, MonoTuple};

use crate::{
    context::{
        Context,
        instantiation::{CTupleID, MonoFunctionSubstID},
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
        let return_type = self.get_mono_return_cty(function).await;
        self.write_cty(&return_type, buf)?;

        let name = self.get_def_name(function.def_id()).await;
        let subst_id = MonoFunctionSubstID::for_function(function);
        write!(buf, " {}", Identifier::def(&name, subst_id))?;

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
}
