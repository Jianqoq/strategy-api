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

pub const HOST_API_OK: i32 = 0;
pub const HOST_API_ERR_INVALID_SIDE: i32 = 1;
pub const HOST_API_ERR_INVALID_QUANTITY: i32 = 2;
pub const HOST_API_ERR_INVALID_PRICE: i32 = 3;
pub const HOST_API_ERR_SUBMIT_FAILED: i32 = 4;

#[repr(C)]
pub struct HostApi {
    pub submit_market_order: unsafe extern "C" fn(
        host_ctx: *mut c_void,
        side: u8,
        quantity: f64,
        reduce_only: bool,
    ) -> i32,
    pub submit_limit_order: unsafe extern "C" fn(
        host_ctx: *mut c_void,
        side: u8,
        quantity: f64,
        price: f64,
        reduce_only: bool,
    ) -> i32,
    pub submit_stop_market_order: unsafe extern "C" fn(
        host_ctx: *mut c_void,
        side: u8,
        quantity: f64,
        stop_price: f64,
        reduce_only: bool,
    ) -> i32,
    pub submit_stop_limit_order: unsafe extern "C" fn(
        host_ctx: *mut c_void,
        side: u8,
        quantity: f64,
        limit_price: f64,
        stop_price: f64,
        reduce_only: bool,
    ) -> i32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum OrderSide {
    Buy = 1,
    Sell = 2,
}

#[derive(Clone, Copy)]
pub struct StrategyContext {
    host_ctx: *mut c_void,
    host_api: *const HostApi,
}

impl StrategyContext {
    #[must_use]
    pub const fn new(host_ctx: *mut c_void, host_api: *const HostApi) -> Self {
        Self { host_ctx, host_api }
    }

    #[must_use]
    pub const fn host_ctx(&self) -> *mut c_void {
        self.host_ctx
    }

    #[must_use]
    pub const fn host_api(&self) -> *const HostApi {
        self.host_api
    }

    pub fn submit_market_order(
        &self,
        side: OrderSide,
        quantity: f64,
        reduce_only: bool,
    ) -> Result<(), HostError> {
        self.submit_status(unsafe {
            ((*self.host_api).submit_market_order)(self.host_ctx, side as u8, quantity, reduce_only)
        })
    }

    pub fn submit_limit_order(
        &self,
        side: OrderSide,
        quantity: f64,
        price: f64,
        reduce_only: bool,
    ) -> Result<(), HostError> {
        self.submit_status(unsafe {
            ((*self.host_api).submit_limit_order)(
                self.host_ctx,
                side as u8,
                quantity,
                price,
                reduce_only,
            )
        })
    }

    pub fn submit_stop_market_order(
        &self,
        side: OrderSide,
        quantity: f64,
        stop_price: f64,
        reduce_only: bool,
    ) -> Result<(), HostError> {
        self.submit_status(unsafe {
            ((*self.host_api).submit_stop_market_order)(
                self.host_ctx,
                side as u8,
                quantity,
                stop_price,
                reduce_only,
            )
        })
    }

    pub fn submit_stop_limit_order(
        &self,
        side: OrderSide,
        quantity: f64,
        limit_price: f64,
        stop_price: f64,
        reduce_only: bool,
    ) -> Result<(), HostError> {
        self.submit_status(unsafe {
            ((*self.host_api).submit_stop_limit_order)(
                self.host_ctx,
                side as u8,
                quantity,
                limit_price,
                stop_price,
                reduce_only,
            )
        })
    }

    fn submit_status(&self, status: i32) -> Result<(), HostError> {
        match status {
            HOST_API_OK => Ok(()),
            HOST_API_ERR_INVALID_SIDE => Err(HostError::InvalidSide),
            HOST_API_ERR_INVALID_QUANTITY => Err(HostError::InvalidQuantity),
            HOST_API_ERR_INVALID_PRICE => Err(HostError::InvalidPrice),
            HOST_API_ERR_SUBMIT_FAILED => Err(HostError::SubmitFailed),
            code => Err(HostError::Unknown(code)),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HostError {
    InvalidSide,
    InvalidQuantity,
    InvalidPrice,
    SubmitFailed,
    Unknown(i32),
}

pub trait Strategy: Sized + 'static {
    fn init(ctx: StrategyContext) -> Self;

    fn drop(&mut self) {}

    fn on_start(&mut self) {}

    fn on_stop(&mut self) {}

    fn on_bar(&mut self, _bar: &Bar) {}
}

#[doc(hidden)]
pub fn __strategy_init<T: Strategy>(
    host_ctx: *mut c_void,
    host_api: *const HostApi,
) -> *mut c_void {
    catch_unwind(AssertUnwindSafe(|| {
        let strategy = T::init(StrategyContext::new(host_ctx, host_api));
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
        pub extern "C" fn init(
            host_ctx: *mut ::std::ffi::c_void,
            host_api: *const $crate::HostApi,
        ) -> *mut ::std::ffi::c_void {
            $crate::__strategy_init::<$strategy_ty>(host_ctx, host_api)
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
        pub unsafe extern "C" fn on_bar(ptr: *mut ::std::ffi::c_void, bar: *const $crate::Bar) {
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
    static MARKET_SUBMITS: AtomicUsize = AtomicUsize::new(0);

    struct TestStrategy {
        ctx: StrategyContext,
    }

    impl Strategy for TestStrategy {
        fn init(ctx: StrategyContext) -> Self {
            Self { ctx }
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
            self.ctx
                .submit_market_order(OrderSide::Buy, 1.0, false)
                .unwrap();
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

    unsafe extern "C" fn test_submit_market_order(
        _host_ctx: *mut c_void,
        side: u8,
        quantity: f64,
        reduce_only: bool,
    ) -> i32 {
        assert_eq!(side, OrderSide::Buy as u8);
        assert_eq!(quantity, 1.0);
        assert!(!reduce_only);
        MARKET_SUBMITS.fetch_add(1, Ordering::Relaxed);
        HOST_API_OK
    }

    unsafe extern "C" fn test_submit_limit_order(
        _host_ctx: *mut c_void,
        _side: u8,
        _quantity: f64,
        _price: f64,
        _reduce_only: bool,
    ) -> i32 {
        HOST_API_OK
    }

    unsafe extern "C" fn test_submit_stop_market_order(
        _host_ctx: *mut c_void,
        _side: u8,
        _quantity: f64,
        _stop_price: f64,
        _reduce_only: bool,
    ) -> i32 {
        HOST_API_OK
    }

    unsafe extern "C" fn test_submit_stop_limit_order(
        _host_ctx: *mut c_void,
        _side: u8,
        _quantity: f64,
        _limit_price: f64,
        _stop_price: f64,
        _reduce_only: bool,
    ) -> i32 {
        HOST_API_OK
    }

    #[test]
    fn wrapper_functions_dispatch_to_trait_impl() {
        let host_api = HostApi {
            submit_market_order: test_submit_market_order,
            submit_limit_order: test_submit_limit_order,
            submit_stop_market_order: test_submit_stop_market_order,
            submit_stop_limit_order: test_submit_stop_limit_order,
        };
        let ptr = __strategy_init::<TestStrategy>(ptr::null_mut(), &host_api);
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
        assert_eq!(MARKET_SUBMITS.load(Ordering::Relaxed), 1);
    }
}
