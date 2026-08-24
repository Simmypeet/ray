use std::{fmt, io};

use qbice::{
    StableHash,
    stable_hash::{Sip128Hasher, StableHasher},
    storage::intern::Interned,
};
use rayc_mono::{MonoFunction, MonoTuple};
use rayc_type::ty::{Ty, TyApplicationView};

use crate::{context::Context, identifier::Identifier};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct MonoFunctionSubstID(u128);

impl MonoFunctionSubstID {
    pub(crate) fn for_function(function: &MonoFunction) -> Self {
        Self(stable_codegen_id("rayc_c::MonoFunctionSubst:v1", function.subst()))
    }

    pub(crate) const fn base62(self) -> Base62 { Base62(self.0) }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash)]
pub struct CTupleID(u128);

impl CTupleID {
    fn for_args(args: &[Interned<Ty>]) -> Self {
        Self(stable_codegen_id("rayc_c::CTupleID:v3", args))
    }

    fn for_tuple(tuple: &MonoTuple) -> Self {
        let mut hasher = Sip128Hasher::default();
        "rayc_c::CTupleID:v3".stable_hash(&mut hasher);

        let args = tuple.args();
        hasher.write_length_prefix(args.len());
        for arg in args {
            arg.stable_hash(&mut hasher);
        }

        Self(hasher.finish())
    }

    pub(crate) const fn base62(self) -> Base62 { Base62(self.0) }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Base62(u128);

impl fmt::Display for Base62 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        const ALPHABET: &[u8; 62] =
            b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
        const MAX_ENCODED_LEN: usize = 22;

        let mut value = self.0;
        let mut encoded = [0; MAX_ENCODED_LEN];
        let mut start = encoded.len();

        loop {
            start -= 1;
            encoded[start] = ALPHABET[(value % 62) as usize];
            value /= 62;

            if value == 0 {
                break;
            }
        }

        let encoded = str::from_utf8(&encoded[start..]).map_err(|_| fmt::Error)?;
        f.write_str(encoded)
    }
}

fn stable_codegen_id<T: StableHash + ?Sized>(domain: &'static str, value: &T) -> u128 {
    let mut hasher = Sip128Hasher::default();
    domain.stable_hash(&mut hasher);
    value.stable_hash(&mut hasher);
    hasher.finish()
}

fn tuple_dependency_depth<'ty>(args: impl Iterator<Item = &'ty Interned<Ty>>) -> usize {
    1 + args.map(|arg| by_value_tuple_depth(arg)).max().unwrap_or(0)
}

fn by_value_tuple_depth(ty: &Ty) -> usize {
    match ty {
        Ty::Application(application) => match application.view() {
            TyApplicationView::Tuple(tuple) => tuple_dependency_depth(tuple.args().iter()),
            TyApplicationView::Primitive(_)
            | TyApplicationView::Pointer(_)
            | TyApplicationView::Error => 0,
        },
        Ty::Inference(_) | Ty::PolyVar(_) => 0,
    }
}

impl Context {
    pub fn write_ctuple_t(&self, id: CTupleID, buf: &mut impl io::Write) -> io::Result<()> {
        write!(buf, "{}", Identifier::tuple_t(id))
    }

    pub fn write_ctuple_struct(&self, id: CTupleID, buf: &mut impl io::Write) -> io::Result<()> {
        write!(buf, "{}", Identifier::tuple_struct(id))
    }

    pub fn ctuple_instances(&self) -> Vec<(CTupleID, &'_ MonoTuple)> {
        let mut tuples = self
            .mono_program
            .tuples()
            .map(|tuple| (CTupleID::for_tuple(tuple), tuple))
            .collect::<Vec<_>>();
        tuples.sort_unstable_by_key(|(id, tuple)| (tuple_dependency_depth(tuple.args()), *id));
        tuples
    }

    pub fn get_ctuple_id(&self, args: &[Interned<Ty>]) -> CTupleID { CTupleID::for_args(args) }
}
