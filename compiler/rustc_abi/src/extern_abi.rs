use std::cmp::Ordering;
use std::fmt;
use std::hash::{Hash, Hasher};

// Keep `inspect::Value` out of the default namespace to avoid name collisions
// with other `Value` types used in the compiler. Use fully-qualified paths
// when constructing inspect values inside `structure()`.
#[cfg(feature = "nightly")]
use rustc_data_structures::stable_hasher::{HashStable, StableHasher, StableOrd, StructureState};
#[cfg(feature = "nightly")]
use rustc_macros::{Decodable, Encodable};
#[cfg(feature = "nightly")]
use rustc_span::Symbol;

use crate::AbiFromStrErr;

#[cfg(test)]
mod tests;

/// ABI we expect to see within `extern "{abi}"`
#[derive(Clone, Copy, Debug)]
#[cfg_attr(feature = "nightly", derive(Encodable, Decodable))]
pub enum ExternAbi {
    /* universal */
    /// presumed C ABI for the platform
    C {
        unwind: bool,
    },
    /// ABI of the "system" interface, e.g. the Win32 API, always "aliasing"
    System {
        unwind: bool,
    },

    /// that's us!
    Rust,
    /// the mostly-unused `unboxed_closures` ABI, effectively now an impl detail unless someone
    /// puts in the work to make it viable again... but would we need a special ABI?
    RustCall,
    /// For things unlikely to be called, where reducing register pressure in
    /// `extern "Rust"` callers is worth paying extra cost in the callee.
    /// Stronger than just `#[cold]` because `fn` pointers might be incompatible.
    RustCold,

    /// An always-invalid ABI that's used to test "this ABI is not supported by this platform"
    /// in a platform-agnostic way.
    RustInvalid,

    /// Preserves no registers.
    ///
    /// Note, that this ABI is not stable in the registers it uses, is intended as an optimization
    /// and may fall-back to a more conservative calling convention if the backend does not support
    /// forcing callers to save all registers.
    RustPreserveNone,

    /// Unstable impl detail that directly uses Rust types to describe the ABI to LLVM.
    /// Even normally-compatible Rust types can become ABI-incompatible with this ABI!
    Unadjusted,

    /// An ABI that rustc does not know how to call or define. Functions with this ABI can
    /// only be created using `#[naked]` functions or `extern "custom"` blocks, and can only
    /// be called from inline assembly.
    Custom,

    /// UEFI ABI, usually an alias of C, but sometimes an arch-specific alias
    /// and only valid on platforms that have a UEFI standard
    EfiApi,

    /* arm */
    /// Arm Architecture Procedure Call Standard, sometimes `ExternAbi::C` is an alias for this
    Aapcs {
        unwind: bool,
    },
    /// extremely constrained barely-C ABI for TrustZone
    CmseNonSecureCall,
    /// extremely constrained barely-C ABI for TrustZone
    CmseNonSecureEntry,

    /* gpu */
    /// An entry-point function called by the GPU's host
    GpuKernel,
    /// An entry-point function called by the GPU's host
    // FIXME: why do we have two of these?
    PtxKernel,

    /* interrupt */
    AvrInterrupt,
    AvrNonBlockingInterrupt,
    Msp430Interrupt,
    RiscvInterruptM,
    RiscvInterruptS,
    X86Interrupt,

    /* x86 */
    /// `ExternAbi::C` but spelled funny because x86
    Cdecl {
        unwind: bool,
    },
    /// gnu-stdcall on "unix" and win-stdcall on "windows"
    Stdcall {
        unwind: bool,
    },
    /// gnu-fastcall on "unix" and win-fastcall on "windows"
    Fastcall {
        unwind: bool,
    },
    /// windows C++ ABI
    Thiscall {
        unwind: bool,
    },
    /// uses AVX and stuff
    Vectorcall {
        unwind: bool,
    },

    /* x86_64 */
    SysV64 {
        unwind: bool,
    },
    Win64 {
        unwind: bool,
    },
}

macro_rules! abi_impls {
    ($e_name:ident = {
        $($variant:ident $({ unwind: $uw:literal })? =><= $tok:literal,)*
    }) => {
        impl $e_name {
            pub const ALL_VARIANTS: &[Self] = &[
                $($e_name::$variant $({ unwind: $uw })*,)*
            ];
            pub const fn as_str(&self) -> &'static str {
                match self {
                    $($e_name::$variant $( { unwind: $uw } )* => $tok,)*
                }
            }
        }

        impl ::core::str::FromStr for $e_name {
            type Err = AbiFromStrErr;
            fn from_str(s: &str) -> Result<$e_name, Self::Err> {
                match s {
                    $($tok => Ok($e_name::$variant $({ unwind: $uw })*),)*
                    _ => Err(AbiFromStrErr::Unknown),
                }
            }
        }
    }
}

abi_impls! {
    ExternAbi = {
            C { unwind: false } =><= "C",
            C { unwind: true } =><= "C-unwind",
            Rust =><= "Rust",
            Aapcs { unwind: false } =><= "aapcs",
            Aapcs { unwind: true } =><= "aapcs-unwind",
            AvrInterrupt =><= "avr-interrupt",
            AvrNonBlockingInterrupt =><= "avr-non-blocking-interrupt",
            Cdecl { unwind: false } =><= "cdecl",
            Cdecl { unwind: true } =><= "cdecl-unwind",
            CmseNonSecureCall =><= "cmse-nonsecure-call",
            CmseNonSecureEntry =><= "cmse-nonsecure-entry",
            Custom =><= "custom",
            EfiApi =><= "efiapi",
            Fastcall { unwind: false } =><= "fastcall",
            Fastcall { unwind: true } =><= "fastcall-unwind",
            GpuKernel =><= "gpu-kernel",
            Msp430Interrupt =><= "msp430-interrupt",
            PtxKernel =><= "ptx-kernel",
            RiscvInterruptM =><= "riscv-interrupt-m",
            RiscvInterruptS =><= "riscv-interrupt-s",
            RustCall =><= "rust-call",
            RustCold =><= "rust-cold",
            RustInvalid =><= "rust-invalid",
            RustPreserveNone =><= "rust-preserve-none",
            Stdcall { unwind: false } =><= "stdcall",
            Stdcall { unwind: true } =><= "stdcall-unwind",
            System { unwind: false } =><= "system",
            System { unwind: true } =><= "system-unwind",
            SysV64 { unwind: false } =><= "sysv64",
            SysV64 { unwind: true } =><= "sysv64-unwind",
            Thiscall { unwind: false } =><= "thiscall",
            Thiscall { unwind: true } =><= "thiscall-unwind",
            Unadjusted =><= "unadjusted",
            Vectorcall { unwind: false } =><= "vectorcall",
            Vectorcall { unwind: true } =><= "vectorcall-unwind",
            Win64 { unwind: false } =><= "win64",
            Win64 { unwind: true } =><= "win64-unwind",
            X86Interrupt =><= "x86-interrupt",
    }
}

impl Ord for ExternAbi {
    fn cmp(&self, rhs: &Self) -> Ordering {
        self.as_str().cmp(rhs.as_str())
    }
}

impl PartialOrd for ExternAbi {
    fn partial_cmp(&self, rhs: &Self) -> Option<Ordering> {
        Some(self.cmp(rhs))
    }
}

impl PartialEq for ExternAbi {
    fn eq(&self, rhs: &Self) -> bool {
        self.cmp(rhs) == Ordering::Equal
    }
}

impl Eq for ExternAbi {}

impl Hash for ExternAbi {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.as_str().hash(state);
        // double-assurance of a prefix breaker
        u32::from_be_bytes(*b"ABI\0").hash(state);
    }
}

#[cfg(feature = "nightly")]
impl<C> HashStable<C> for ExternAbi {
    #[inline]
    fn structure<W: ::rustc_data_structures::inspect::Write>(
        &self,
        _state: &mut StructureState<'_, C, W>,
    ) -> ::rustc_data_structures::inspect::Value {
        // Preserve enum semantics in inspection output.

        static SCHEMA_C: ::rustc_data_structures::inspect::SchemaRef =
            ::rustc_data_structures::inspect::SchemaRef::new(
                ::rustc_data_structures::inspect::Schema::Enum {
                    path: "rustc_abi::extern_abi::ExternAbi",
                    variant_name: "C",
                    variant: ::rustc_data_structures::inspect::EnumVariantSchema::Named(&[
                        "unwind",
                    ]),
                },
            );
        static SCHEMA_SYSTEM: ::rustc_data_structures::inspect::SchemaRef =
            ::rustc_data_structures::inspect::SchemaRef::new(
                ::rustc_data_structures::inspect::Schema::Enum {
                    path: "rustc_abi::extern_abi::ExternAbi",
                    variant_name: "System",
                    variant: ::rustc_data_structures::inspect::EnumVariantSchema::Named(&[
                        "unwind",
                    ]),
                },
            );

        // Unit variants share one schema per variant.
        macro_rules! unit_schema {
            ($name:literal) => {
                ::rustc_data_structures::inspect::SchemaRef::new(
                    ::rustc_data_structures::inspect::Schema::Enum {
                        path: "rustc_abi::extern_abi::ExternAbi",
                        variant_name: $name,
                        variant: ::rustc_data_structures::inspect::EnumVariantSchema::Unit,
                    },
                )
            };
        }

        static SCHEMA_RUST: ::rustc_data_structures::inspect::SchemaRef = unit_schema!("Rust");
        static SCHEMA_RUSTCALL: ::rustc_data_structures::inspect::SchemaRef =
            unit_schema!("RustCall");
        static SCHEMA_RUSTCOLD: ::rustc_data_structures::inspect::SchemaRef =
            unit_schema!("RustCold");
        static SCHEMA_RUSTINVALID: ::rustc_data_structures::inspect::SchemaRef =
            unit_schema!("RustInvalid");
        static SCHEMA_RUSTPRESERVENONE: ::rustc_data_structures::inspect::SchemaRef =
            unit_schema!("RustPreserveNone");
        static SCHEMA_UNADJUSTED: ::rustc_data_structures::inspect::SchemaRef =
            unit_schema!("Unadjusted");
        static SCHEMA_CUSTOM: ::rustc_data_structures::inspect::SchemaRef = unit_schema!("Custom");
        static SCHEMA_EFIAPI: ::rustc_data_structures::inspect::SchemaRef = unit_schema!("EfiApi");
        static SCHEMA_CMSE_NSC: ::rustc_data_structures::inspect::SchemaRef =
            unit_schema!("CmseNonSecureCall");
        static SCHEMA_CMSE_NSE: ::rustc_data_structures::inspect::SchemaRef =
            unit_schema!("CmseNonSecureEntry");
        static SCHEMA_GPUKERNEL: ::rustc_data_structures::inspect::SchemaRef =
            unit_schema!("GpuKernel");
        static SCHEMA_PTXKERNEL: ::rustc_data_structures::inspect::SchemaRef =
            unit_schema!("PtxKernel");
        static SCHEMA_AVRINT: ::rustc_data_structures::inspect::SchemaRef =
            unit_schema!("AvrInterrupt");
        static SCHEMA_AVRNBI: ::rustc_data_structures::inspect::SchemaRef =
            unit_schema!("AvrNonBlockingInterrupt");
        static SCHEMA_MSP430INT: ::rustc_data_structures::inspect::SchemaRef =
            unit_schema!("Msp430Interrupt");
        static SCHEMA_RISCVIM: ::rustc_data_structures::inspect::SchemaRef =
            unit_schema!("RiscvInterruptM");
        static SCHEMA_RISCVIS: ::rustc_data_structures::inspect::SchemaRef =
            unit_schema!("RiscvInterruptS");
        static SCHEMA_X86INT: ::rustc_data_structures::inspect::SchemaRef =
            unit_schema!("X86Interrupt");

        static SCHEMA_CDECL: ::rustc_data_structures::inspect::SchemaRef =
            ::rustc_data_structures::inspect::SchemaRef::new(
                ::rustc_data_structures::inspect::Schema::Enum {
                    path: "rustc_abi::extern_abi::ExternAbi",
                    variant_name: "Cdecl",
                    variant: ::rustc_data_structures::inspect::EnumVariantSchema::Named(&[
                        "unwind",
                    ]),
                },
            );
        static SCHEMA_STDCALL: ::rustc_data_structures::inspect::SchemaRef =
            ::rustc_data_structures::inspect::SchemaRef::new(
                ::rustc_data_structures::inspect::Schema::Enum {
                    path: "rustc_abi::extern_abi::ExternAbi",
                    variant_name: "Stdcall",
                    variant: ::rustc_data_structures::inspect::EnumVariantSchema::Named(&[
                        "unwind",
                    ]),
                },
            );
        static SCHEMA_FASTCALL: ::rustc_data_structures::inspect::SchemaRef =
            ::rustc_data_structures::inspect::SchemaRef::new(
                ::rustc_data_structures::inspect::Schema::Enum {
                    path: "rustc_abi::extern_abi::ExternAbi",
                    variant_name: "Fastcall",
                    variant: ::rustc_data_structures::inspect::EnumVariantSchema::Named(&[
                        "unwind",
                    ]),
                },
            );
        static SCHEMA_THISCALL: ::rustc_data_structures::inspect::SchemaRef =
            ::rustc_data_structures::inspect::SchemaRef::new(
                ::rustc_data_structures::inspect::Schema::Enum {
                    path: "rustc_abi::extern_abi::ExternAbi",
                    variant_name: "Thiscall",
                    variant: ::rustc_data_structures::inspect::EnumVariantSchema::Named(&[
                        "unwind",
                    ]),
                },
            );
        static SCHEMA_VECTORCALL: ::rustc_data_structures::inspect::SchemaRef =
            ::rustc_data_structures::inspect::SchemaRef::new(
                ::rustc_data_structures::inspect::Schema::Enum {
                    path: "rustc_abi::extern_abi::ExternAbi",
                    variant_name: "Vectorcall",
                    variant: ::rustc_data_structures::inspect::EnumVariantSchema::Named(&[
                        "unwind",
                    ]),
                },
            );
        static SCHEMA_SYSV64: ::rustc_data_structures::inspect::SchemaRef =
            ::rustc_data_structures::inspect::SchemaRef::new(
                ::rustc_data_structures::inspect::Schema::Enum {
                    path: "rustc_abi::extern_abi::ExternAbi",
                    variant_name: "SysV64",
                    variant: ::rustc_data_structures::inspect::EnumVariantSchema::Named(&[
                        "unwind",
                    ]),
                },
            );
        static SCHEMA_WIN64: ::rustc_data_structures::inspect::SchemaRef =
            ::rustc_data_structures::inspect::SchemaRef::new(
                ::rustc_data_structures::inspect::Schema::Enum {
                    path: "rustc_abi::extern_abi::ExternAbi",
                    variant_name: "Win64",
                    variant: ::rustc_data_structures::inspect::EnumVariantSchema::Named(&[
                        "unwind",
                    ]),
                },
            );

        static SCHEMA_AAPCS: ::rustc_data_structures::inspect::SchemaRef =
            ::rustc_data_structures::inspect::SchemaRef::new(
                ::rustc_data_structures::inspect::Schema::Enum {
                    path: "rustc_abi::extern_abi::ExternAbi",
                    variant_name: "Aapcs",
                    variant: ::rustc_data_structures::inspect::EnumVariantSchema::Named(&[
                        "unwind",
                    ]),
                },
            );

        let (schema, values): (
            &'static ::rustc_data_structures::inspect::SchemaRef,
            Vec<::rustc_data_structures::inspect::Value>,
        ) = match self {
            ExternAbi::C { unwind } => {
                (&SCHEMA_C, vec![::rustc_data_structures::inspect::Value::Bool(*unwind)])
            }
            ExternAbi::System { unwind } => {
                (&SCHEMA_SYSTEM, vec![::rustc_data_structures::inspect::Value::Bool(*unwind)])
            }
            ExternAbi::Aapcs { unwind } => {
                (&SCHEMA_AAPCS, vec![::rustc_data_structures::inspect::Value::Bool(*unwind)])
            }
            ExternAbi::Rust => (&SCHEMA_RUST, Vec::new()),
            ExternAbi::RustCall => (&SCHEMA_RUSTCALL, Vec::new()),
            ExternAbi::RustCold => (&SCHEMA_RUSTCOLD, Vec::new()),
            ExternAbi::RustInvalid => (&SCHEMA_RUSTINVALID, Vec::new()),
            ExternAbi::RustPreserveNone => (&SCHEMA_RUSTPRESERVENONE, Vec::new()),
            ExternAbi::Unadjusted => (&SCHEMA_UNADJUSTED, Vec::new()),
            ExternAbi::Custom => (&SCHEMA_CUSTOM, Vec::new()),
            ExternAbi::EfiApi => (&SCHEMA_EFIAPI, Vec::new()),
            ExternAbi::CmseNonSecureCall => (&SCHEMA_CMSE_NSC, Vec::new()),
            ExternAbi::CmseNonSecureEntry => (&SCHEMA_CMSE_NSE, Vec::new()),
            ExternAbi::GpuKernel => (&SCHEMA_GPUKERNEL, Vec::new()),
            ExternAbi::PtxKernel => (&SCHEMA_PTXKERNEL, Vec::new()),
            ExternAbi::AvrInterrupt => (&SCHEMA_AVRINT, Vec::new()),
            ExternAbi::AvrNonBlockingInterrupt => (&SCHEMA_AVRNBI, Vec::new()),
            ExternAbi::Msp430Interrupt => (&SCHEMA_MSP430INT, Vec::new()),
            ExternAbi::RiscvInterruptM => (&SCHEMA_RISCVIM, Vec::new()),
            ExternAbi::RiscvInterruptS => (&SCHEMA_RISCVIS, Vec::new()),
            ExternAbi::X86Interrupt => (&SCHEMA_X86INT, Vec::new()),

            ExternAbi::Cdecl { unwind } => {
                (&SCHEMA_CDECL, vec![::rustc_data_structures::inspect::Value::Bool(*unwind)])
            }
            ExternAbi::Stdcall { unwind } => {
                (&SCHEMA_STDCALL, vec![::rustc_data_structures::inspect::Value::Bool(*unwind)])
            }
            ExternAbi::Fastcall { unwind } => {
                (&SCHEMA_FASTCALL, vec![::rustc_data_structures::inspect::Value::Bool(*unwind)])
            }
            ExternAbi::Thiscall { unwind } => {
                (&SCHEMA_THISCALL, vec![::rustc_data_structures::inspect::Value::Bool(*unwind)])
            }
            ExternAbi::Vectorcall { unwind } => {
                (&SCHEMA_VECTORCALL, vec![::rustc_data_structures::inspect::Value::Bool(*unwind)])
            }
            ExternAbi::SysV64 { unwind } => {
                (&SCHEMA_SYSV64, vec![::rustc_data_structures::inspect::Value::Bool(*unwind)])
            }
            ExternAbi::Win64 { unwind } => {
                (&SCHEMA_WIN64, vec![::rustc_data_structures::inspect::Value::Bool(*unwind)])
            }
        };

        let id = _state.intern_schema(schema);
        ::rustc_data_structures::inspect::Value::Schema { id, values }
    }

    #[inline]
    fn hash_stable(&self, _: &mut C, hasher: &mut StableHasher) {
        Hash::hash(self, hasher);
    }
}

#[cfg(feature = "nightly")]
impl StableOrd for ExternAbi {
    const CAN_USE_UNSTABLE_SORT: bool = true;

    // because each ABI is hashed like a string, there is no possible instability
    const THIS_IMPLEMENTATION_HAS_BEEN_TRIPLE_CHECKED: () = ();
}

#[cfg(feature = "nightly")]
rustc_error_messages::into_diag_arg_using_display!(ExternAbi);

#[cfg(feature = "nightly")]
pub enum CVariadicStatus {
    NotSupported,
    Stable,
    Unstable { feature: Symbol },
}

impl ExternAbi {
    /// An ABI "like Rust"
    ///
    /// These ABIs are fully controlled by the Rust compiler, which means they
    /// - support unwinding with `-Cpanic=unwind`, unlike `extern "C"`
    /// - often diverge from the C ABI
    /// - are subject to change between compiler versions
    pub fn is_rustic_abi(self) -> bool {
        use ExternAbi::*;
        matches!(self, Rust | RustCall | RustCold | RustPreserveNone)
    }

    /// Returns whether the ABI supports C variadics. This only controls whether we allow *imports*
    /// of such functions via `extern` blocks; there's a separate check during AST construction
    /// guarding *definitions* of variadic functions.
    #[cfg(feature = "nightly")]
    pub fn supports_c_variadic(self) -> CVariadicStatus {
        // * C and Cdecl obviously support varargs.
        // * C can be based on Aapcs, SysV64 or Win64, so they must support varargs.
        // * EfiApi is based on Win64 or C, so it also supports it.
        // * System automatically falls back to C when used with variadics, therefore supports it.
        //
        // * Stdcall does not, because it would be impossible for the callee to clean
        //   up the arguments. (callee doesn't know how many arguments are there)
        // * Same for Fastcall, Vectorcall and Thiscall.
        // * Other calling conventions are related to hardware or the compiler itself.
        //
        // All of the supported ones must have a test in `tests/codegen/cffi/c-variadic-ffi.rs`.
        match self {
            Self::C { .. }
            | Self::Cdecl { .. }
            | Self::Aapcs { .. }
            | Self::Win64 { .. }
            | Self::SysV64 { .. }
            | Self::EfiApi
            | Self::System { .. } => CVariadicStatus::Stable,
            _ => CVariadicStatus::NotSupported,
        }
    }

    /// Returns whether the ABI supports guaranteed tail calls.
    #[cfg(feature = "nightly")]
    pub fn supports_guaranteed_tail_call(self) -> bool {
        match self {
            Self::CmseNonSecureCall | Self::CmseNonSecureEntry => {
                // See https://godbolt.org/z/9jhdeqErv. The CMSE calling conventions clear registers
                // before returning, and hence cannot guarantee a tail call.
                false
            }
            Self::AvrInterrupt
            | Self::AvrNonBlockingInterrupt
            | Self::Msp430Interrupt
            | Self::RiscvInterruptM
            | Self::RiscvInterruptS
            | Self::X86Interrupt => {
                // See https://godbolt.org/z/Edfjnxxcq. Interrupts cannot be called directly.
                false
            }
            Self::GpuKernel | Self::PtxKernel => {
                // See https://godbolt.org/z/jq5TE5jK1.
                false
            }
            Self::Custom => {
                // This ABI does not support calls at all (except via assembly).
                false
            }
            Self::C { .. }
            | Self::System { .. }
            | Self::Rust
            | Self::RustCall
            | Self::RustCold
            | Self::RustInvalid
            | Self::Unadjusted
            | Self::EfiApi
            | Self::Aapcs { .. }
            | Self::Cdecl { .. }
            | Self::Stdcall { .. }
            | Self::Fastcall { .. }
            | Self::Thiscall { .. }
            | Self::Vectorcall { .. }
            | Self::SysV64 { .. }
            | Self::Win64 { .. }
            | Self::RustPreserveNone => true,
        }
    }
}

pub fn all_names() -> Vec<&'static str> {
    ExternAbi::ALL_VARIANTS.iter().map(|abi| abi.as_str()).collect()
}

impl ExternAbi {
    /// Default ABI chosen for `extern fn` declarations without an explicit ABI.
    pub const FALLBACK: ExternAbi = ExternAbi::C { unwind: false };

    pub fn name(self) -> &'static str {
        self.as_str()
    }
}

impl fmt::Display for ExternAbi {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "\"{}\"", self.as_str())
    }
}
