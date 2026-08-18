use std::io;

use qbice::{Identifiable, StableHash, storage::intern::Interned};
use rayc_type::ty::{Ty, TyApplicationView};

use crate::generator::{
    Generator,
    instantiation::{CTuple, CTupleID},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash)]
pub enum Primitive {
    Bool,
    Float32,
    Int32,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash)]
pub struct Pointer {
    constness: bool,
    element_ty: Interned<CTy>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Identifiable)]
pub enum CTy {
    Primitive(Primitive),
    Tuple(CTupleID),
    Pointer(Pointer),
}

impl Generator {
    pub fn write_cty(&self, cty: &CTy, buf: &mut impl io::Write) -> std::io::Result<()> {
        match cty {
            CTy::Primitive(primitive) => match primitive {
                Primitive::Bool => write!(buf, "bool"),
                Primitive::Float32 => write!(buf, "float"),
                Primitive::Int32 => write!(buf, "int32_t"),
            },

            CTy::Tuple(id) => self.write_ctuple_t(*id, buf),

            CTy::Pointer(pointer) => {
                write!(buf, "{}", if pointer.constness { "const " } else { "" })?;
                self.write_cty(&pointer.element_ty, buf)?;
                write!(buf, "*")
            }
        }
    }

    pub fn ty_to_cty(&mut self, ty: &Interned<Ty>) -> Interned<CTy> {
        match &**ty {
            Ty::Application(ty_application) => match ty_application.view() {
                TyApplicationView::Primitive(primitive) => {
                    let prim = match primitive {
                        rayc_type::ty::Primitive::Int32 => Primitive::Int32,
                        rayc_type::ty::Primitive::Float32 => Primitive::Float32,
                        rayc_type::ty::Primitive::Bool => Primitive::Bool,
                    };

                    self.intern(CTy::Primitive(prim))
                }

                TyApplicationView::Tuple(tuple_view) => {
                    let mut args = Vec::with_capacity(tuple_view.args().len());

                    for arg in tuple_view.args() {
                        let cty = self.ty_to_cty(arg);
                        args.push(cty);
                    }

                    let args = self.intern_unsized(args);
                    let ctuple_id = self.get_ctuple_id(CTuple::builder().args(args).build());

                    self.intern(CTy::Tuple(ctuple_id))
                }

                TyApplicationView::Pointer(pointer_view) => {
                    let element_ty = self.ty_to_cty(pointer_view.pointee());

                    self.intern(CTy::Pointer(Pointer {
                        constness: pointer_view.mutability().constness(),
                        element_ty,
                    }))
                }

                TyApplicationView::Error => {
                    panic!("type error reached codegen, this should have been caught earlier")
                }
            },

            Ty::Inference(_) => {
                panic!("type inference should have been resolved before codegen")
            }
        }
    }
}
