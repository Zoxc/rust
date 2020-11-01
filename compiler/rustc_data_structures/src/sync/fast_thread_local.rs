/// A macro which allows you to use `#[thread_local]` for faster access, but fall back to the
/// standard `thread_local` macro if it's unavailable.
///
/// A caveat with using `#[thread_local]` is that is has bugs with inlining so you must prevent
/// code that accesses the thread local from being generated in other crates using
/// `#[inline(never)]` and not accessing it in generic code.
#[macro_export]
#[allow_internal_unstable(cfg_target_thread_local, thread_local)]
macro_rules! fast_thread_local {
    ($(#[$attr:meta])* $vis:vis static $name:ident: $t:ty = $init:expr;) => (
        $vis const $name: $crate::sync::fast_thread_local::LocalKey<$t> = {
            #[cfg(target_thread_local)]
            {
                // A function which allows us to access the thread local in the `LocalKey` type.
                #[inline(always)]
                fn with(f: &mut dyn FnMut(&$t)) {
                    #[thread_local]
                    static LOCAL: $t = $init;

                    f(&LOCAL)
                }

                $crate::sync::fast_thread_local::LocalKey { with }
            }

            #[cfg(not(target_thread_local))]
            {
                // Ensure that $init can be const evaluated.
                const _: $t = $init;

                thread_local! {
                    $vis static LOCAL: $t = $init;
                }

                $crate::sync::fast_thread_local::LocalKey { inner: LOCAL }
            }
        };
    );
}

pub struct LocalKey<T: 'static> {
    #[cfg(target_thread_local)]
    pub with: fn(&mut dyn FnMut(&T)),
    #[cfg(not(target_thread_local))]
    pub inner: std::thread::LocalKey<T>,
}

impl<T> LocalKey<T> {
    #[inline]
    pub fn with<R>(&'static self, f: impl FnOnce(&T) -> R) -> R {
        #[cfg(target_thread_local)]
        {
            // Boilerplate calling `self.with` passing `f` and returning its result.
            let mut f = Some(f);
            let result = std::lazy::OnceCell::new();
            (self.with)(&mut |value| {
                result.set((f.take().unwrap())(value)).ok();
            });
            result.into_inner().unwrap()
        }

        #[cfg(not(target_thread_local))]
        {
            self.inner.with(f)
        }
    }
}
