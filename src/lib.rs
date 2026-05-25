#[cfg(test)]
use std::cell::RefCell;

pub const HOST_API_OK: i32 = 0;
pub const HOST_API_ERR_INVALID_SIDE: i32 = 1;
pub const HOST_API_ERR_INVALID_QUANTITY: i32 = 2;
pub const HOST_API_ERR_INVALID_PRICE: i32 = 3;
pub const HOST_API_ERR_SUBMIT_FAILED: i32 = 4;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[repr(C)]
pub struct Bar {
    pub timestamp_ms: i64,
    pub open: f32,
    pub high: f32,
    pub low: f32,
    pub close: f32,
    pub volume: f64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(i32)]
pub enum OrderSide {
    Buy = 1,
    Sell = 2,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct StrategyContext;

impl StrategyContext {
    #[must_use]
    pub const fn new() -> Self {
        Self
    }

    pub fn submit_market_order(
        &self,
        side: OrderSide,
        quantity: f64,
        reduce_only: bool,
    ) -> Result<(), HostError> {
        self.submit_status(host_submit_market_order(
            side as i32,
            quantity,
            reduce_only as i32,
        ))
    }

    pub fn submit_limit_order(
        &self,
        side: OrderSide,
        quantity: f64,
        price: f64,
        reduce_only: bool,
    ) -> Result<(), HostError> {
        self.submit_status(host_submit_limit_order(
            side as i32,
            quantity,
            price,
            reduce_only as i32,
        ))
    }

    pub fn submit_stop_market_order(
        &self,
        side: OrderSide,
        quantity: f64,
        stop_price: f64,
        reduce_only: bool,
    ) -> Result<(), HostError> {
        self.submit_status(host_submit_stop_market_order(
            side as i32,
            quantity,
            stop_price,
            reduce_only as i32,
        ))
    }

    pub fn submit_stop_limit_order(
        &self,
        side: OrderSide,
        quantity: f64,
        limit_price: f64,
        stop_price: f64,
        reduce_only: bool,
    ) -> Result<(), HostError> {
        self.submit_status(host_submit_stop_limit_order(
            side as i32,
            quantity,
            limit_price,
            stop_price,
            reduce_only as i32,
        ))
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

    fn on_start(&mut self) {}

    fn on_stop(&mut self) {}

    fn on_bar(&mut self, _bar: &Bar) {}
}

#[cfg(not(test))]
#[link(wasm_import_module = "host")]
unsafe extern "C" {
    #[link_name = "submit_market_order"]
    fn raw_host_submit_market_order(side: i32, quantity: f64, reduce_only: i32) -> i32;
    #[link_name = "submit_limit_order"]
    fn raw_host_submit_limit_order(side: i32, quantity: f64, price: f64, reduce_only: i32) -> i32;
    #[link_name = "submit_stop_market_order"]
    fn raw_host_submit_stop_market_order(
        side: i32,
        quantity: f64,
        stop_price: f64,
        reduce_only: i32,
    ) -> i32;
    #[link_name = "submit_stop_limit_order"]
    fn raw_host_submit_stop_limit_order(
        side: i32,
        quantity: f64,
        limit_price: f64,
        stop_price: f64,
        reduce_only: i32,
    ) -> i32;
}

#[cfg(not(test))]
fn host_submit_market_order(side: i32, quantity: f64, reduce_only: i32) -> i32 {
    unsafe { raw_host_submit_market_order(side, quantity, reduce_only) }
}

#[cfg(not(test))]
fn host_submit_limit_order(side: i32, quantity: f64, price: f64, reduce_only: i32) -> i32 {
    unsafe { raw_host_submit_limit_order(side, quantity, price, reduce_only) }
}

#[cfg(not(test))]
fn host_submit_stop_market_order(
    side: i32,
    quantity: f64,
    stop_price: f64,
    reduce_only: i32,
) -> i32 {
    unsafe { raw_host_submit_stop_market_order(side, quantity, stop_price, reduce_only) }
}

#[cfg(not(test))]
fn host_submit_stop_limit_order(
    side: i32,
    quantity: f64,
    limit_price: f64,
    stop_price: f64,
    reduce_only: i32,
) -> i32 {
    unsafe {
        raw_host_submit_stop_limit_order(side, quantity, limit_price, stop_price, reduce_only)
    }
}

#[macro_export]
macro_rules! export_strategy {
    ($strategy_ty:ty) => {
        ::std::thread_local! {
            static STRATEGY_INSTANCE: ::std::cell::RefCell<Option<$strategy_ty>> =
                ::std::cell::RefCell::new(None);
        }

        #[unsafe(no_mangle)]
        pub extern "C" fn init() {
            let _ = ::std::panic::catch_unwind(::std::panic::AssertUnwindSafe(|| {
                STRATEGY_INSTANCE.with(|strategy| {
                    *strategy.borrow_mut() = Some(<$strategy_ty as $crate::Strategy>::init(
                        $crate::StrategyContext::new(),
                    ));
                });
            }));
        }

        #[unsafe(no_mangle)]
        pub extern "C" fn on_start() {
            let _ = ::std::panic::catch_unwind(::std::panic::AssertUnwindSafe(|| {
                STRATEGY_INSTANCE.with(|strategy| {
                    if let Some(strategy) = strategy.borrow_mut().as_mut() {
                        strategy.on_start();
                    }
                });
            }));
        }

        #[unsafe(no_mangle)]
        pub extern "C" fn on_stop() {
            let _ = ::std::panic::catch_unwind(::std::panic::AssertUnwindSafe(|| {
                STRATEGY_INSTANCE.with(|strategy| {
                    if let Some(strategy) = strategy.borrow_mut().as_mut() {
                        strategy.on_stop();
                    }
                });
            }));
        }

        #[unsafe(no_mangle)]
        pub extern "C" fn on_bar(
            timestamp_ms: i64,
            open: f32,
            high: f32,
            low: f32,
            close: f32,
            volume: f64,
        ) {
            let _ = ::std::panic::catch_unwind(::std::panic::AssertUnwindSafe(|| {
                let bar = $crate::Bar {
                    timestamp_ms,
                    open,
                    high,
                    low,
                    close,
                    volume,
                };
                STRATEGY_INSTANCE.with(|strategy| {
                    if let Some(strategy) = strategy.borrow_mut().as_mut() {
                        strategy.on_bar(&bar);
                    }
                });
            }));
        }
    };
}

#[cfg(test)]
thread_local! {
    static TEST_MARKET_ORDERS: RefCell<Vec<(i32, f64, i32)>> = const { RefCell::new(Vec::new()) };
}

#[cfg(test)]
fn host_submit_market_order(side: i32, quantity: f64, reduce_only: i32) -> i32 {
    TEST_MARKET_ORDERS.with(|orders| orders.borrow_mut().push((side, quantity, reduce_only)));
    HOST_API_OK
}

#[cfg(test)]
fn host_submit_limit_order(_side: i32, _quantity: f64, _price: f64, _reduce_only: i32) -> i32 {
    HOST_API_OK
}

#[cfg(test)]
fn host_submit_stop_market_order(
    _side: i32,
    _quantity: f64,
    _stop_price: f64,
    _reduce_only: i32,
) -> i32 {
    HOST_API_OK
}

#[cfg(test)]
fn host_submit_stop_limit_order(
    _side: i32,
    _quantity: f64,
    _limit_price: f64,
    _stop_price: f64,
    _reduce_only: i32,
) -> i32 {
    HOST_API_OK
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    static STARTS: AtomicUsize = AtomicUsize::new(0);
    static STOPS: AtomicUsize = AtomicUsize::new(0);
    static BARS: AtomicUsize = AtomicUsize::new(0);

    struct TestStrategy {
        ctx: StrategyContext,
    }

    impl Strategy for TestStrategy {
        fn init(ctx: StrategyContext) -> Self {
            Self { ctx }
        }

        fn on_start(&mut self) {
            STARTS.fetch_add(1, Ordering::Relaxed);
        }

        fn on_stop(&mut self) {
            STOPS.fetch_add(1, Ordering::Relaxed);
        }

        fn on_bar(&mut self, bar: &Bar) {
            assert_eq!(bar.timestamp_ms, 1);
            assert_eq!(bar.close, 1.05);
            BARS.fetch_add(1, Ordering::Relaxed);
            self.ctx
                .submit_market_order(OrderSide::Buy, 1.0, false)
                .unwrap();
        }
    }

    export_strategy!(TestStrategy);

    #[test]
    fn wasm_exports_dispatch_to_strategy() {
        init();
        on_start();
        on_bar(1, 1.0, 1.1, 0.9, 1.05, 4.0);
        on_stop();

        assert_eq!(STARTS.load(Ordering::Relaxed), 1);
        assert_eq!(STOPS.load(Ordering::Relaxed), 1);
        assert_eq!(BARS.load(Ordering::Relaxed), 1);
        TEST_MARKET_ORDERS.with(|orders| {
            let orders = orders.borrow();
            assert_eq!(orders.len(), 1);
            assert_eq!(orders[0], (OrderSide::Buy as i32, 1.0, 0));
        });
    }
}
