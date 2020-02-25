//use crate::infer::canonical::Canonical;
use crate::hir::exports::Export;
use crate::infer::canonical::{Canonical, QueryResponse};
use crate::middle::resolve_lifetime::ObjectLifetimeDefault;
use crate::mir;
use crate::mir::interpret;
use crate::traits;
use crate::traits::query::{DropckOutlivesResult, NoSolution, NormalizationResult, OutlivesBound};
use crate::traits::select::{EvaluationResult, OverflowError};
use crate::ty::fast_reject::SimplifiedType;
use crate::ty::sty::FnSig;
use crate::ty::subst::SubstsRef;
use crate::ty::Region;
use crate::ty::{self, Binder, GenericPredicates, Predicate, Ty};
use rustc_data_structures::fx::FxHashMap;
use rustc_data_structures::fx::FxHashSet;
use rustc_data_structures::fx::FxIndexMap;
use rustc_errors::ErrorReported;
use rustc_hir as hir;
use rustc_hir::def_id::{CrateNum, DefId, DefIndex};
use rustc_hir::hir_id::ItemLocalId;
use rustc_span::symbol::Symbol;
use std::mem::transmute;
use std::rc::Rc;
use std::sync::Arc;

pub(super) trait Erase {
    type Erased;

    fn erase(self) -> Self::Erased;
    unsafe fn recover(erased: Self::Erased) -> Self;
}

macro_rules! identity_impls {
    (impl $([$($params:tt)*] $ty:ty,)*) => {
        $(
            impl $($params)* Erase for $ty {
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
    ([] $($ty:ty,)*) => {
        identity_impls!(impl $([] $ty,)*);
    };
    ($params:tt $($ty:ty,)*) => {
        identity_impls!(impl $($params $ty,)*);
    };
}

macro_rules! copy_impls {
    (impl $([$($params:tt)*] $ty:ty, $ety:ty,)*) => {
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
    ([] $($ty:ty,)*) => {
        copy_impls!(impl $([] $ty, $ty,)*);
    };
    ($params:tt $($ty:ty, $ety:ty,)*) => {
        copy_impls!(impl $($params $ty, $ety,)*);
    };
}

copy_impls!([<'a, T>]
    &'a T, &'static (),
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
    traits::Environment<'tcx>, traits::Environment<'static>,
    interpret::ConstValue<'tcx>, interpret::ConstValue<'static>,
    std::result::Result<interpret::ConstValue<'tcx>, interpret::ErrorHandled>, std::result::Result<interpret::ConstValue<'static>, interpret::ErrorHandled>,
    ty::sty::Binder<ty::sty::FnSig<'tcx>>, ty::sty::Binder<ty::sty::FnSig<'static>>,
    Result<&Canonical<'tcx, QueryResponse<'tcx, Vec<OutlivesBound<'tcx>>>>, NoSolution>, Result<&Canonical<'static, QueryResponse<'static, Vec<OutlivesBound<'static>>>>, NoSolution>,
    Result<&Canonical<'tcx, QueryResponse<'tcx, NormalizationResult<'tcx>>>, NoSolution>, Result<&Canonical<'static, QueryResponse<'static, NormalizationResult<'static>>>, NoSolution>,
    Result<&Canonical<'tcx, QueryResponse<'tcx, DropckOutlivesResult<'tcx>>>, NoSolution>, Result<&Canonical<'static, QueryResponse<'static, DropckOutlivesResult<'static>>>, NoSolution>,
    Result<&Canonical<'tcx, QueryResponse<'tcx, ()>>, NoSolution>, Result<&Canonical<'static, QueryResponse<'static, ()>>, NoSolution>,
    Result<&Canonical<'tcx, QueryResponse<'tcx, Ty<'tcx>>>, NoSolution>, Result<&Canonical<'static, QueryResponse<'static, Ty<'static>>>, NoSolution>,
    Result<&Canonical<'tcx, QueryResponse<'tcx, Predicate<'tcx>>>, NoSolution>, Result<&Canonical<'static, QueryResponse<'static, Predicate<'static>>>, NoSolution>,
    Result<&Canonical<'tcx, QueryResponse<'tcx, FnSig<'tcx>>>, NoSolution>, Result<&Canonical<'static, QueryResponse<'static, FnSig<'static>>>, NoSolution>,
    Result<&Canonical<'tcx, QueryResponse<'tcx, Binder<FnSig<'tcx>>>>, NoSolution>, Result<&Canonical<'static, QueryResponse<'static, Binder<FnSig<'static>>>>, NoSolution>,
    Option<&'tcx FxHashMap<SubstsRef<'tcx>, CrateNum>>, Option<&'static FxHashMap<SubstsRef<'static>, CrateNum>>,
    Option<&'tcx [Export<hir::HirId>]>, Option<&'static [Export<hir::HirId>]>,
    Option<&'tcx FxHashMap<ItemLocalId, Region<'tcx>>>, Option<&'static FxHashMap<ItemLocalId, Region<'static>>>,
    Option<&'tcx FxHashSet<ItemLocalId>>, Option<&'static FxHashSet<ItemLocalId>>,
    Option<&'tcx FxHashMap<ItemLocalId, Vec<ObjectLifetimeDefault>>>, Option<&'static FxHashMap<ItemLocalId, Vec<ObjectLifetimeDefault>>>,
    Option<&'tcx FxIndexMap<hir::HirId, hir::Upvar>>, Option<&'static FxIndexMap<hir::HirId, hir::Upvar>>,
    GenericPredicates<'tcx>, GenericPredicates<'static>,
);

copy_impls!([]
    bool,
    CrateNum,
    Option<CrateNum>,
    DefIndex,
    DefId,
    (DefId, DefId),
    (CrateNum, DefId),
    (DefId, SimplifiedType),
    Symbol,
    (Symbol, u32, u32),
    ty::adjustment::CoerceUnsizedInfo,
    Result<(), ErrorReported>,
    (),
    usize,
    Result<EvaluationResult, OverflowError>,
    hir::Defaultness,
);

identity_impls!([<T: ?Sized>]
    Arc<T>,
    Rc<T>,
);

identity_impls!([<T>]
    Vec<T>,
);

identity_impls!([<'tcx>]
    traits::query::MethodAutoderefStepsResult<'tcx>,
);

identity_impls!([] 
    mir::UnsafetyCheckResult,
);
