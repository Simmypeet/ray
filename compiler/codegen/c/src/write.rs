use std::io;

use crate::context::{
    Context,
    instantiation::{CDefDecl, CDefID, CTupleID},
};

impl Context {
    const TAB: &'static str = "    ";

    pub fn write_forward_decl_tuples(&self, buf: &mut impl std::io::Write) -> std::io::Result<()> {
        for id in self.ctuple_decl_ids() {
            write!(buf, "typedef struct ")?;
            self.write_ctuple_struct(id, buf)?;
            write!(buf, " ")?;
            self.write_ctuple_t(id, buf)?;
            writeln!(buf, ";")?;
        }

        Ok(())
    }

    pub fn write_tuple_struct_defs(&self, buf: &mut impl std::io::Write) -> std::io::Result<()> {
        for id in self.ctuple_decl_ids() {
            self.write_tuple_struct_def(id, buf)?;
            writeln!(buf)?;
        }

        Ok(())
    }

    pub async fn write_forward_decl_cdefs(
        &self,
        buf: &mut impl std::io::Write,
    ) -> std::io::Result<()> {
        for id in self.cdef_decl_ids() {
            self.write_cdef_decl(id, buf).await?;
            writeln!(buf, ";")?;
        }

        Ok(())
    }

    pub async fn write_cdef_decl(
        &self,
        id: CDefID,
        buf: &mut impl io::Write,
    ) -> std::io::Result<()> {
        let cdecl = self.get_cdef_decl(id);

        self.write_cty(cdecl.return_type(), buf)?;

        let name = self.get_def_name(cdecl.def_id()).await;

        write!(buf, " ray_{}", &*name)?;

        self.write_parameter_list(cdecl, buf)
    }

    fn write_parameter_list(
        &self,
        cdef_decl: &CDefDecl,
        buf: &mut impl io::Write,
    ) -> std::io::Result<()> {
        write!(buf, "(")?;
        let mut first = true;

        for (param_id, param_ty) in cdef_decl.parameters() {
            if !first {
                write!(buf, ", ")?;
            }

            self.write_cty(param_ty, buf)?;

            write!(buf, " ray_param_{:X}", param_id.index())?;

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
        buf: &mut impl io::Write,
    ) -> std::io::Result<()> {
        write!(buf, "struct ")?;
        self.write_ctuple_struct(id, buf)?;

        writeln!(buf, " {{")?;

        let tuple_decl = self.get_ctuple_decl(id);

        if tuple_decl.is_unit() {
            write!(buf, "{}", Self::TAB)?;
            writeln!(buf, "uint8_t _unit;")?;
        }

        for (i, arg) in tuple_decl.args().enumerate() {
            write!(buf, "{}", Self::TAB)?;
            self.write_cty(arg, buf)?;

            writeln!(buf, " elem{i:X};")?;
        }

        write!(buf, "}};")
    }
}
