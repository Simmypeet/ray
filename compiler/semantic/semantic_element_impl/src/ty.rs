use rayc_extend::extend;
use rayc_qbice::TrackedEngine;
use rayc_syntax::r#type::{Primitive as PrimitiveSyntax, Type as TySyntax};
use rayc_type::ty::{Primitive, Ty};
use qbice::storage::intern::Interned;

#[extend]
#[allow(clippy::similar_names)]
pub fn resolve_ty(self: &TrackedEngine, ty: &TySyntax) -> Interned<Ty> {
    match ty {
        TySyntax::Primitive(primitive) => {
            let prim = match primitive {
                PrimitiveSyntax::Int32(_) => Primitive::Int32,
                PrimitiveSyntax::Bool(_) => Primitive::Bool,
                PrimitiveSyntax::Float32(_) => Primitive::Float32,
            };

            Ty::new_primitive(prim, self)
        }
        TySyntax::Pointer(pointer) => {
            let pointee_ty = pointer
                .pointed_type()
                .map_or_else(|| Ty::new_error(self), |pointed_ty| self.resolve_ty(&pointed_ty));

            Ty::new_pointer(pointee_ty, self)
        }

        TySyntax::Tuple(tuple) => {
            let mut args = Vec::new();

            for elem in tuple.elements() {
                args.push(self.resolve_ty(&elem));
            }

            Ty::new_tuple(self.intern_unsized(args), self)
        }
    }
}
