use std::io;

use bon::Builder;
use qbice::{
    StableHash,
    stable_hash::{Sip128Hasher, StableHasher},
    storage::intern::Interned,
};
use rayc_arena::ID;
use rayc_hash::FxHashMap;
use rayc_semantic_element::{
    parameter::{ParameterID, get_parameter_map},
    return_type::get_return_type,
};
use rayc_symbol::{GlobalSymbolID, name::get_name};

use crate::{generator::Generator, ty::CTy};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CDef {
    def_id: GlobalSymbolID,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CDefDecl {
    def_id: GlobalSymbolID,

    parameters: Interned<[(ParameterID, Interned<CTy>)]>,
    return_type: Interned<CTy>,
}

pub type CDefID = ID<CDef>;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Builder)]
pub struct CTuple {
    args: Interned<[Interned<CTy>]>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CTupleDecl {
    args: Interned<[Interned<CTy>]>,
    hash: u128,
}

pub type CTupleID = ID<CTuple>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstantiationTable {
    def_table: FxHashMap<CDef, CDefID>,
    tuple_table: FxHashMap<CTuple, CTupleID>,

    def_decls: FxHashMap<CDefID, CDefDecl>,
    tuple_decls: FxHashMap<CTupleID, CTupleDecl>,
}

impl Generator {
    pub fn get_ctuple_id(&mut self, ctuple: CTuple) -> CTupleID {
        if let Some(id) = self.inst_table.tuple_table.get(&ctuple) {
            return *id;
        }

        let id = self.inst_table.tuple_table.len() as u64;
        let id = ID::new(id);

        self.inst_table.tuple_table.insert(ctuple.clone(), id);

        let hash = {
            let mut hasher = Sip128Hasher::new();
            ctuple.args.stable_hash(&mut hasher);
            hasher.finish()
        };

        let decl = CTupleDecl { args: ctuple.args, hash };
        self.inst_table.tuple_decls.insert(id, decl);

        id
    }

    pub async fn get_cdef_id(&mut self, cdef: CDef) -> CDefID {
        if let Some(id) = self.inst_table.def_table.get(&cdef) {
            return *id;
        }

        let id = self.inst_table.def_table.len() as u64;
        let id = ID::new(id);

        let return_ty = self.ty_to_cty(&self.engine.get_return_type(cdef.def_id).await);
        let parameters = {
            let parameter_map = self.engine.get_parameter_map(cdef.def_id).await;
            let mut parameters = Vec::with_capacity(parameter_map.len());

            for (param_id, param) in parameter_map.iter() {
                let param_ty = self.ty_to_cty(param.ty());
                parameters.push((param_id, param_ty));
            }

            self.intern_unsized(parameters)
        };

        let decl = CDefDecl { def_id: cdef.def_id, parameters, return_type: return_ty };

        self.inst_table.def_table.insert(cdef, id);
        self.inst_table.def_decls.insert(id, decl);

        id
    }

    pub fn write_ctuple(&self, id: CTupleID, buf: &mut impl io::Write) -> std::io::Result<()> {
        let decl = self.inst_table.tuple_decls.get(&id).unwrap();

        write!(buf, "ray_tuple_{:X}", decl.hash)
    }

    pub async fn write_cdef_decl(
        &self,
        id: CDefID,
        buf: &mut impl io::Write,
    ) -> std::io::Result<()> {
        let cdecl = self.inst_table.def_decls.get(&id).unwrap();

        self.write_cty(&cdecl.return_type, buf)?;

        let name = self.engine.get_name(cdecl.def_id).await;

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

        for (param_id, param_ty) in cdef_decl.parameters.iter() {
            if !first {
                write!(buf, ", ")?;
            }

            self.write_cty(param_ty, buf)?;

            write!(buf, " param_{:X}", param_id.index())?;

            first = false;
        }

        Ok(())
    }
}
