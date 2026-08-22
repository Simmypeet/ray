use std::{fmt, io};

use bon::Builder;
use qbice::{
    StableHash,
    stable_hash::{Sip128Hasher, StableHasher},
    storage::intern::Interned,
};
use rayc_hash::FxHashMap;
use rayc_semantic_element::{
    parameter::{ParameterID, get_parameter_map},
    return_type::get_return_type,
};
use rayc_symbol::GlobalSymbolID;

use crate::{context::Context, ty::CTy};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Builder)]
pub struct CDef {
    def_id: GlobalSymbolID,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CDefDecl {
    def_id: GlobalSymbolID,

    parameters: Interned<[(ParameterID, Interned<CTy>)]>,
    return_type: Interned<CTy>,
}

impl CDefDecl {
    #[must_use]
    pub const fn def_id(&self) -> GlobalSymbolID { self.def_id }

    #[must_use]
    pub const fn return_type(&self) -> &Interned<CTy> { &self.return_type }

    pub fn parameters(&self) -> impl Iterator<Item = (ParameterID, &'_ Interned<CTy>)> {
        self.parameters.iter().map(|(param_id, param_ty)| (*param_id, param_ty))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash)]
pub struct CDefID(u128);

impl CDefID {
    fn for_cdef(cdef: &CDef) -> Self { Self(stable_codegen_id("rayc_c::CDefID:v1", cdef)) }
}

impl fmt::UpperHex for CDefID {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { fmt::UpperHex::fmt(&self.0, f) }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Builder)]
pub struct CTuple {
    args: Interned<[Interned<CTy>]>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CTupleDecl {
    args: Interned<[Interned<CTy>]>,
}

impl CTupleDecl {
    pub fn args(&self) -> impl Iterator<Item = &'_ Interned<CTy>> { self.args.iter() }

    #[must_use]
    pub fn is_unit(&self) -> bool { self.args.is_empty() }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash)]
pub struct CTupleID(u128);

impl CTupleID {
    fn for_ctuple(tuple: &CTuple) -> Self { Self(stable_codegen_id("rayc_c::CTupleID:v1", tuple)) }
}

impl fmt::UpperHex for CTupleID {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { fmt::UpperHex::fmt(&self.0, f) }
}

fn stable_codegen_id<T: StableHash>(domain: &'static str, value: &T) -> u128 {
    let mut hasher = Sip128Hasher::default();
    domain.stable_hash(&mut hasher);
    value.stable_hash(&mut hasher);
    hasher.finish()
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct InstantiationTable {
    def_table: FxHashMap<CDef, CDefID>,
    tuple_table: FxHashMap<CTuple, CTupleID>,

    def_decls: FxHashMap<CDefID, CDefDecl>,
    tuple_decls: FxHashMap<CTupleID, CTupleDecl>,
}

impl Context {
    pub fn get_ctuple_id(&mut self, ctuple: CTuple) -> CTupleID {
        if let Some(id) = self.inst_table.tuple_table.get(&ctuple) {
            return *id;
        }

        let id = CTupleID::for_ctuple(&ctuple);

        assert!(
            self.inst_table
                .tuple_decls
                .insert(id, CTupleDecl { args: ctuple.args.clone() })
                .is_none(),
            "compiler-internal duplicate CTupleID declaration insertion for {id:?}"
        );

        assert!(
            self.inst_table.tuple_table.insert(ctuple, id).is_none(),
            "compiler-internal duplicate CTuple key insertion for CTupleID {id:?}"
        );

        id
    }

    pub async fn get_cdef_id(&mut self, cdef: CDef) -> CDefID {
        if let Some(id) = self.inst_table.def_table.get(&cdef) {
            return *id;
        }

        let id = CDefID::for_cdef(&cdef);

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

        assert!(
            self.inst_table.def_decls.insert(id, decl).is_none(),
            "compiler-internal duplicate CDefID declaration insertion for {id:?}"
        );

        assert!(
            self.inst_table.def_table.insert(cdef, id).is_none(),
            "compiler-internal duplicate CDef key insertion for CDefID {id:?}"
        );

        id
    }

    pub fn write_ctuple_t(&self, id: CTupleID, buf: &mut impl io::Write) -> std::io::Result<()> {
        write!(buf, "RayTuple{id:X}_t")
    }

    pub fn write_ctuple_struct(
        &self,
        id: CTupleID,
        buf: &mut impl io::Write,
    ) -> std::io::Result<()> {
        write!(buf, "RayTuple{id:X}")
    }

    pub fn cdef_decl_ids(&self) -> impl Iterator<Item = CDefID> + '_ {
        self.inst_table.def_decls.keys().copied()
    }

    pub fn ctuple_decl_ids(&self) -> impl Iterator<Item = CTupleID> + '_ {
        self.inst_table.tuple_decls.keys().copied()
    }

    pub fn get_cdef_decl(&self, id: CDefID) -> &CDefDecl {
        self.inst_table.def_decls.get(&id).unwrap()
    }

    pub fn get_ctuple_decl(&self, id: CTupleID) -> &CTupleDecl {
        self.inst_table.tuple_decls.get(&id).unwrap()
    }
}
