use std::io;

use qbice::{Identifiable, StableHash, storage::intern::Interned};
use rayc_type::ty::{Ty, application::View as ApplicationView};

use crate::context::{
    Context,
    instantiation::{CLambdaTypeID, CTupleID},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash)]
pub enum Primitive {
    Bool,
    Float32,
    Int32,
    CInt,
    CStr,
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
    Lambda(CLambdaTypeID),
    Pointer(Pointer),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CAbiReturn {
    Void,
    Value(Interned<CTy>),
}

impl Context {
    pub fn write_cty(&self, cty: &CTy, buf: &mut impl io::Write) -> std::io::Result<()> {
        match cty {
            CTy::Primitive(primitive) => match primitive {
                Primitive::Bool => write!(buf, "bool"),
                Primitive::Float32 => write!(buf, "float"),
                Primitive::Int32 => write!(buf, "int32_t"),
                Primitive::CInt => write!(buf, "int"),
                Primitive::CStr => write!(buf, "const char *"),
            },

            CTy::Tuple(id) => self.write_ctuple_t(*id, buf),

            CTy::Lambda(id) => self.write_clambda_t(*id, buf),

            CTy::Pointer(pointer) => {
                write!(buf, "{}", if pointer.constness { "const " } else { "" })?;
                self.write_cty(&pointer.element_ty, buf)?;
                write!(buf, "*")
            }
        }
    }

    pub fn unwrap_ty_as_ctuple_id(&self, ty: &Interned<Ty>) -> CTupleID {
        let view = ty.unwrap_as_application_view().unwrap_into_tuple_view();
        self.get_ctuple_id(view.args())
    }

    pub fn ty_to_cty(&self, ty: &Interned<Ty>) -> Interned<CTy> {
        match &**ty {
            Ty::Application(ty_application) => match ty_application.view() {
                ApplicationView::Primitive(primitive) => {
                    let prim = match primitive {
                        rayc_type::ty::Primitive::Int32 => Primitive::Int32,
                        rayc_type::ty::Primitive::Float32 => Primitive::Float32,
                        rayc_type::ty::Primitive::Bool => Primitive::Bool,
                        rayc_type::ty::Primitive::CInt => Primitive::CInt,
                        rayc_type::ty::Primitive::CStr => Primitive::CStr,
                    };

                    self.intern(CTy::Primitive(prim))
                }

                ApplicationView::Tuple(tuple_view) => {
                    let ctuple_id = self.get_ctuple_id(tuple_view.args());

                    self.intern(CTy::Tuple(ctuple_id))
                }

                ApplicationView::Lambda(lambda_view) => {
                    let id = self.get_clambda_type_id(
                        lambda_view.parameter_types(),
                        lambda_view.return_type(),
                    );
                    self.intern(CTy::Lambda(id))
                }

                ApplicationView::Pointer(pointer_view) => {
                    let element_ty = self.ty_to_cty(pointer_view.pointee());

                    self.intern(CTy::Pointer(Pointer {
                        constness: pointer_view.mutability().constness(),
                        element_ty,
                    }))
                }

                ApplicationView::InstanceAssociated(_) => {
                    panic!("unresolved associated type reached code generation")
                }
                ApplicationView::Instance(_) | ApplicationView::DefInstance(_) => {
                    panic!("an instance argument cannot be lowered as a C value type")
                }

                ApplicationView::Closure(_) => todo!("lower a closure type to a C type"),

                ApplicationView::Error => {
                    panic!("type error reached codegen, this should have been caught earlier")
                }
            },

            Ty::Inference(_) => {
                panic!("type inference should have been resolved before codegen")
            }

            Ty::PolyVar(_) | Ty::SelfInstance(_) => {
                panic!("polymorphic variables should have been instantiated before codegen")
            }

            Ty::EffectRow(_) => todo!("lower an effect-row type to a C type"),
        }
    }
}
