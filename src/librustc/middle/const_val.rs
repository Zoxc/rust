// Copyright 2012-2016 The Rust Project Developers. See the COPYRIGHT
// file at the top-level directory of this distribution and at
// http://rust-lang.org/COPYRIGHT.
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

use hir::def_id::DefId;
use ty::{self, TyCtxt, layout};
use ty::subst::Substs;
use rustc_const_math::*;
use mir::interpret::{Allocation, PrimVal, Value, MemoryPointer};
use errors::DiagnosticBuilder;

use graphviz::IntoCow;
use syntax_pos::Span;

use std::borrow::Cow;
use rustc_data_structures::sync::Lrc;

pub type EvalResult<'tcx> = Result<&'tcx ty::Const<'tcx>, ConstEvalErr<'tcx>>;

#[derive(Copy, Clone, Debug, Hash, RustcEncodable, RustcDecodable, Eq, PartialEq)]
pub enum ConstVal<'tcx> {
    Unevaluated(DefId, &'tcx Substs<'tcx>),
    Value(ConstValue<'tcx>),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, RustcEncodable, RustcDecodable, Hash)]
pub enum ConstValue<'tcx> {
    // Used only for types with layout::abi::Scalar ABI and ZSTs which use PrimVal::Undef
    ByVal(PrimVal),
    // Used only for types with layout::abi::ScalarPair
    ByValPair(PrimVal, PrimVal),
    // Used only for the remaining cases
    ByRef(&'tcx Allocation),
}

impl<'tcx> ConstValue<'tcx> {
    #[inline]
    pub fn from_byval_value(val: Value) -> Self {
        match val {
            Value::ByRef(..) => bug!(),
            Value::ByValPair(a, b) => ConstValue::ByValPair(a, b),
            Value::ByVal(val) => ConstValue::ByVal(val),
        }
    }

    #[inline]
    pub fn to_byval_value(&self) -> Option<Value> {
        match *self {
            ConstValue::ByRef(..) => None,
            ConstValue::ByValPair(a, b) => Some(Value::ByValPair(a, b)),
            ConstValue::ByVal(val) => Some(Value::ByVal(val)),
        }
    }

    #[inline]
    pub fn from_primval(val: PrimVal) -> Self {
        ConstValue::ByVal(val)
    }

    #[inline]
    pub fn to_primval(&self) -> Option<PrimVal> {
        match *self {
            ConstValue::ByRef(..) => None,
            ConstValue::ByValPair(..) => None,
            ConstValue::ByVal(val) => Some(val),
        }
    }

    #[inline]
    pub fn to_bits(&self) -> Option<u128> {
        match self.to_primval() {
            Some(PrimVal::Bytes(val)) => Some(val),
            _ => None,
        }
    }

    #[inline]
    pub fn to_ptr(&self) -> Option<MemoryPointer> {
        match self.to_primval() {
            Some(PrimVal::Ptr(ptr)) => Some(ptr),
            _ => None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct ConstEvalErr<'tcx> {
    pub span: Span,
    pub kind: Lrc<ErrKind<'tcx>>,
}

#[derive(Clone, Debug)]
pub enum ErrKind<'tcx> {

    NonConstPath,
    UnimplementedConstVal(&'static str),
    IndexOutOfBounds { len: u64, index: u64 },

    Math(ConstMathErr),
    LayoutError(layout::LayoutError<'tcx>),

    TypeckError,
    CheckMatchError,
    Miri(::mir::interpret::EvalError<'tcx>, Vec<FrameInfo>),
}

#[derive(Clone, Debug)]
pub struct FrameInfo {
    pub span: Span,
    pub location: String,
}

impl<'tcx> From<ConstMathErr> for ErrKind<'tcx> {
    fn from(err: ConstMathErr) -> ErrKind<'tcx> {
        match err {
            ConstMathErr::UnsignedNegation => ErrKind::TypeckError,
            _ => ErrKind::Math(err)
        }
    }
}

#[derive(Clone, Debug)]
pub enum ConstEvalErrDescription<'a, 'tcx: 'a> {
    Simple(Cow<'a, str>),
    Backtrace(&'a ::mir::interpret::EvalError<'tcx>, &'a [FrameInfo]),
}

impl<'a, 'tcx> ConstEvalErrDescription<'a, 'tcx> {
    /// Return a one-line description of the error, for lints and such
    pub fn into_oneline(self) -> Cow<'a, str> {
        match self {
            ConstEvalErrDescription::Simple(simple) => simple,
            ConstEvalErrDescription::Backtrace(miri, _) => format!("{}", miri).into_cow(),
        }
    }
}

impl<'a, 'gcx, 'tcx> ConstEvalErr<'tcx> {
    pub fn description(&'a self) -> ConstEvalErrDescription<'a, 'tcx> {
        use self::ErrKind::*;
        use self::ConstEvalErrDescription::*;

        macro_rules! simple {
            ($msg:expr) => ({ Simple($msg.into_cow()) });
            ($fmt:expr, $($arg:tt)+) => ({
                Simple(format!($fmt, $($arg)+).into_cow())
            })
        }

        match *self.kind {
            NonConstPath        => simple!("non-constant path in constant expression"),
            UnimplementedConstVal(what) =>
                simple!("unimplemented constant expression: {}", what),
            IndexOutOfBounds { len, index } => {
                simple!("index out of bounds: the len is {} but the index is {}",
                        len, index)
            }

            Math(ref err) => Simple(err.description().into_cow()),
            LayoutError(ref err) => Simple(err.to_string().into_cow()),

            TypeckError => simple!("type-checking failed"),
            CheckMatchError => simple!("match-checking failed"),
            Miri(ref err, ref trace) => Backtrace(err, trace),
        }
    }

    pub fn struct_error(&self,
        tcx: TyCtxt<'a, 'gcx, 'tcx>,
        primary_span: Span,
        primary_kind: &str)
        -> DiagnosticBuilder<'gcx>
    {
        let mut diag = struct_error(tcx, self.span, "constant evaluation error");
        self.note(tcx, primary_span, primary_kind, &mut diag);
        diag
    }

    pub fn note(&self,
        _tcx: TyCtxt<'a, 'gcx, 'tcx>,
        primary_span: Span,
        primary_kind: &str,
        diag: &mut DiagnosticBuilder)
    {
        match self.description() {
            ConstEvalErrDescription::Simple(message) => {
                diag.span_label(self.span, message);
            }
            ConstEvalErrDescription::Backtrace(miri, frames) => {
                diag.span_label(self.span, format!("{}", miri));
                for frame in frames {
                    diag.span_label(frame.span, format!("inside call to `{}`", frame.location));
                }
            }
        }

        if !primary_span.contains(self.span) {
            diag.span_note(primary_span,
                        &format!("for {} here", primary_kind));
        }
    }

    pub fn report(&self,
        tcx: TyCtxt<'a, 'gcx, 'tcx>,
        primary_span: Span,
        primary_kind: &str)
    {
        match *self.kind {
            ErrKind::TypeckError | ErrKind::CheckMatchError => return,
            ErrKind::Miri(ref miri, _) => {
                match miri.kind {
                    ::mir::interpret::EvalErrorKind::TypeckError |
                    ::mir::interpret::EvalErrorKind::Layout(_) => return,
                    _ => {},
                }
            }
            _ => {}
        }
        self.struct_error(tcx, primary_span, primary_kind).emit();
    }
}

pub fn struct_error<'a, 'gcx, 'tcx>(
    tcx: TyCtxt<'a, 'gcx, 'tcx>,
    span: Span,
    msg: &str,
) -> DiagnosticBuilder<'gcx> {
    struct_span_err!(tcx.sess, span, E0080, "{}", msg)
}
