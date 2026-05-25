use std::{
    ffi::c_void,
    panic::{AssertUnwindSafe, catch_unwind},
    ptr,
};

pub use nautilus_model::data::Bar;

pub const INIT_SYMBOL: &[u8] = b"init\0";
pub const DROP_SYMBOL: &[u8] = b"drop\0";
pub const ON_START_SYMBOL: &[u8] = b"on_start\0";
pub const ON_STOP_SYMBOL: &[u8] = b"on_stop\0";
pub const ON_BAR_SYMBOL: &[u8] = b"on_bar\0";

pub trait Strategy: Sized + 'static {
    fn init() -> Self;

    fn drop(&mut self) {}

    fn on_start(&mut self) {}

    fn on_stop(&mut self) {}

    fn on_bar(&mut self, _bar: &Bar) {}
}

#[doc(hidden)]
pub fn __strategy_init<T: Strategy>() -> *mut c_void {
    catch_unwind(AssertUnwindSafe(|| {
        let strategy = T::init();
        Box::into_raw(Box::new(strategy)) as *mut c_void
    }))
    .unwrap_or(ptr::null_mut())
}

#[doc(hidden)]
pub unsafe fn __strategy_drop<T: Strategy>(ptr: *mut c_void) {
    if ptr.is_null() {
        return;
    }

    let _ = catch_unwind(AssertUnwindSafe(|| {
        let mut boxed = unsafe { Box::from_raw(ptr.cast::<T>()) };
        Strategy::drop(&mut *boxed);
    }));
}

#[doc(hidden)]
pub unsafe fn __strategy_on_start<T: Strategy>(ptr: *mut c_void) {
    if ptr.is_null() {
        return;
    }

    let _ = catch_unwind(AssertUnwindSafe(|| unsafe {
        (&mut *ptr.cast::<T>()).on_start();
    }));
}

#[doc(hidden)]
pub unsafe fn __strategy_on_stop<T: Strategy>(ptr: *mut c_void) {
    if ptr.is_null() {
        return;
    }

    let _ = catch_unwind(AssertUnwindSafe(|| unsafe {
        (&mut *ptr.cast::<T>()).on_stop();
    }));
}

#[doc(hidden)]
pub unsafe fn __strategy_on_bar<T: Strategy>(ptr: *mut c_void, bar: *const Bar) {
    if ptr.is_null() || bar.is_null() {
        return;
    }

    let _ = catch_unwind(AssertUnwindSafe(|| unsafe {
        (&mut *ptr.cast::<T>()).on_bar(&*bar);
    }));
}

#[macro_export]
macro_rules! export_strategy {
    ($strategy_ty:ty) => {
        #[unsafe(no_mangle)]
        pub extern "C" fn init() -> *mut ::std::ffi::c_void {
            $crate::__strategy_init::<$strategy_ty>()
        }

        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn drop(ptr: *mut ::std::ffi::c_void) {
            unsafe { $crate::__strategy_drop::<$strategy_ty>(ptr) }
        }

        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn on_start(ptr: *mut ::std::ffi::c_void) {
            unsafe { $crate::__strategy_on_start::<$strategy_ty>(ptr) }
        }

        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn on_stop(ptr: *mut ::std::ffi::c_void) {
            unsafe { $crate::__strategy_on_stop::<$strategy_ty>(ptr) }
        }

        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn on_bar(
            ptr: *mut ::std::ffi::c_void,
            bar: *const $crate::Bar,
        ) {
            unsafe { $crate::__strategy_on_bar::<$strategy_ty>(ptr, bar) }
        }
    };
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use nautilus_core::UnixNanos;
    use nautilus_model::{
        data::{BarSpecification, BarType},
        enums::{AggregationSource, BarAggregation, PriceType},
        identifiers::InstrumentId,
        types::{Price, Quantity},
    };

    use super::*;

    static STARTS: AtomicUsize = AtomicUsize::new(0);
    static STOPS: AtomicUsize = AtomicUsize::new(0);
    static BARS: AtomicUsize = AtomicUsize::new(0);
    static DROPS: AtomicUsize = AtomicUsize::new(0);

    struct TestStrategy;

    impl Strategy for TestStrategy {
        fn init() -> Self {
            Self
        }

        fn drop(&mut self) {
            DROPS.fetch_add(1, Ordering::Relaxed);
        }

        fn on_start(&mut self) {
            STARTS.fetch_add(1, Ordering::Relaxed);
        }

        fn on_stop(&mut self) {
            STOPS.fetch_add(1, Ordering::Relaxed);
        }

        fn on_bar(&mut self, _bar: &Bar) {
            BARS.fetch_add(1, Ordering::Relaxed);
        }
    }

    fn test_bar() -> Bar {
        Bar::new(
            BarType::new(
                InstrumentId::from("AUDUSD.SIM"),
                BarSpecification::new(1, BarAggregation::Minute, PriceType::Last),
                AggregationSource::External,
            ),
            Price::from("1.0000"),
            Price::from("1.1000"),
            Price::from("0.9000"),
            Price::from("1.0500"),
            Quantity::from("1000"),
            UnixNanos::from(1),
            UnixNanos::from(1),
        )
    }

    #[test]
    fn wrapper_functions_dispatch_to_trait_impl() {
        let ptr = __strategy_init::<TestStrategy>();
        assert!(!ptr.is_null());

        unsafe {
            __strategy_on_start::<TestStrategy>(ptr);
            __strategy_on_bar::<TestStrategy>(ptr, &test_bar());
            __strategy_on_stop::<TestStrategy>(ptr);
            __strategy_drop::<TestStrategy>(ptr);
        }

        assert_eq!(STARTS.load(Ordering::Relaxed), 1);
        assert_eq!(BARS.load(Ordering::Relaxed), 1);
        assert_eq!(STOPS.load(Ordering::Relaxed), 1);
        assert_eq!(DROPS.load(Ordering::Relaxed), 1);
    }
}
