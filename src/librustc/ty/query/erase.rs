//use crate::infer::canonical::Canonical;
use crate::mir;
use crate::traits;
use crate::ty::fast_reject::SimplifiedType;
use crate::ty::subst::SubstsRef;
use crate::ty::{self};
use rustc_hir::def_id::{CrateNum, DefId, DefIndex};
use rustc_span::symbol::Symbol;
use std::mem::transmute;

pub(super) trait Erase {
    type Erased;

    fn erase(self) -> Self::Erased;
    unsafe fn recover(erased: Self::Erased) -> Self;
}
/*
macro_rules! identity_impls {
    ([$(lifetimes:tt)*] $($ty:ty),*) => {
        $(
            impl $($lifetimes)* Erase for $ty {
                type Erased = Self;

                #[inline(always)]
                fn erase(self) -> Self::Erased {
                    self
                }

                #[inline(always)]
                unsafe fn recover(erased: Self::Erased) -> Self {
                    erased
                }
            }
        )*
    };
}
*/
macro_rules! copy_impls {
    (impl $([$($params:tt)*] $ty:ty, $ety:ty),*) => {
        $(
            impl $($params)* Erase for $ty {
                type Erased = [u8; std::mem::size_of::<$ety>()];

                #[inline(always)]
                fn erase(self) -> Self::Erased {
                    unsafe { transmute(self) }
                }

                #[inline(always)]
                unsafe fn recover(erased: Self::Erased) -> Self {
                    transmute(erased)
                }
            }
        )*
    };
    ([] $($ty:ty),*) => {
        copy_impls!(impl $([] $ty, $ty),*);
    };
    ($params:tt $($ty:ty, $ety:ty),*) => {
        copy_impls!(impl $($params $ty, $ety),*);
    };
}

copy_impls!([<'a, T>]
    &'a T, &'static ()
);
/*
copy_impls!([<'tcx, T>]
    Canonical<'tcx, T>,
    ty::ParamEnvAnd<'tcx, T>
);
*/
copy_impls!([<'tcx>]
    ty::InstanceDef<'tcx>, ty::InstanceDef<'static>,
    ty::Instance<'tcx>, ty::Instance<'static>,
    mir::interpret::GlobalId<'tcx>, mir::interpret::GlobalId<'static>,
    mir::interpret::LitToConstInput<'tcx>, mir::interpret::LitToConstInput<'static>,
    (DefId, SubstsRef<'tcx>), (DefId, SubstsRef<'static>),
    (ty::ParamEnv<'tcx>, ty::PolyTraitRef<'tcx>), (ty::ParamEnv<'static>, ty::PolyTraitRef<'static>),
    (&'tcx ty::Const<'tcx>, mir::Field), (&'static ty::Const<'static>, mir::Field),
    ty::PolyTraitRef<'tcx>, ty::PolyTraitRef<'static>,
    ty::ParamEnv<'tcx>, ty::ParamEnv<'static>,
    traits::Environment<'tcx>, traits::Environment<'static>
);

copy_impls!([]
    CrateNum,
    DefIndex,
    DefId,
    (DefId, DefId),
    (CrateNum, DefId),
    (DefId, SimplifiedType),
    Symbol,
    (Symbol, u32, u32)
);
