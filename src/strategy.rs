use crate::{Bar, Strategy, ctx::Indicator};

// ── Signal ────────────────────────────────────────────────────────────────────

pub type Signal = i32;

pub const BUY: Signal = 1;
pub const SELL: Signal = -1;
pub const HOLD: Signal = 0;

#[repr(C)]
pub struct StateBlob {
    pub ptr: *mut u8,
    pub len: usize,
}

// ── Fn-pointer types (used by the host via libloading) ────────────────────────

pub type FnOnInit = unsafe extern "C" fn(total_bars: usize);
pub type FnOnStrategyBar = unsafe extern "C" fn(bar: *const Bar, index: usize, ctx: *mut Strategy);
pub type FnOnRestoreState = unsafe extern "C" fn(state: StateBlob);
pub type FnOnStrategyFinish = unsafe extern "C" fn(ctx: *mut Strategy) -> StateBlob;
pub type FnOnIndicatorBar =
    unsafe extern "C" fn(bar: *const Bar, index: usize, ctx: *mut Indicator);
pub type FnOnIndicatorFinish = unsafe extern "C" fn(ctx: *mut Indicator) -> StateBlob;

/// Implement this trait in your DLL, then call `export_strategy!(YourType)`.
///
/// All methods are called from the host's main thread in order:
///   `new` → `init` → `on_bar` × N → `on_finish`
pub trait OnStrategy {
    /// Construct the strategy. Called once when the DLL singleton is first accessed.
    fn new() -> Self
    where
        Self: Sized;

    fn restore_state(&mut self, state: StateBlob);

    /// Called once before the first bar. Use this to reset any state.
    fn init(&mut self, total_bars: usize);

    /// Called for every bar in order.
    ///
    /// - `bar`   — OHLCV data for this bar
    /// - `index` — zero-based position in the full dataset
    /// - `ctx`   — drawing context; call `ctx.line()`, `ctx.circle()`, etc.
    ///
    /// Return `BUY`, `SELL`, or `HOLD`.
    fn on_bar(&mut self, bar: &Bar, index: usize, ctx: &mut Strategy);

    /// Called once after the last bar. Use this to draw final annotations.
    fn on_finish(&mut self, ctx: &mut Strategy) -> Vec<u8>;
}

pub trait OnIndicator {
    /// Construct the indicator. Called once when the DLL singleton is first accessed.
    fn new() -> Self
    where
        Self: Sized;

    fn restore_state(&mut self, state: StateBlob);

    /// Called once before the first bar. Use this to reset any state.
    fn init(&mut self, total_bars: usize);

    /// Called for every bar in order.
    ///
    /// - `bar`   — OHLCV data for this bar
    /// - `index` — zero-based position in the full dataset
    /// - `ctx`   — drawing context; call `ctx.line()`, `ctx.circle()`, etc.
    ///
    /// Return `BUY`, `SELL`, or `HOLD`.
    fn on_bar(&mut self, bar: &Bar, index: usize, ctx: &mut Indicator);

    /// Called once after the last bar. Use this to draw final annotations.
    fn on_finish(&mut self, ctx: &mut Indicator) -> Vec<u8>;
}

// ── Export macro ──────────────────────────────────────────────────────────────

/// Wire up a [`OnBar`] impl as a loadable DLL.
///
/// # Example
///
/// ```rust,ignore
/// use strategy_api::{Bar, BacktestCtx, OrderKind, Side, OnBar, export_strategy};
///
/// struct MyStrategy {
///     prev_close: f32,
/// }
///
/// impl OnBar for MyStrategy {
///     fn new() -> Self { Self { prev_close: 0.0 } }
///     fn name()        -> &'static str { "My OnBar" }
///     fn description() -> &'static str { "Buy on up-close, sell on down-close." }
///     fn version()     -> &'static str { "1.0.0" }
///
///     fn init(&mut self, _total_bars: usize) {
///         self.prev_close = 0.0;
///     }
///
///     fn on_bar(&mut self, bar: &Bar, _index: usize, ctx: &mut BacktestCtx) {
///         if bar.close > self.prev_close {
///             ctx.submit_order(Side::Long, OrderKind::Market, 1.0);
///             ctx.mark_buy(bar.close);
///         } else if bar.close < self.prev_close {
///             ctx.submit_order(Side::Short, OrderKind::Market, 1.0);
///             ctx.mark_sell(bar.close);
///         }
///         self.prev_close = bar.close;
///     }
///
///     fn on_finish(&mut self, _ctx: &mut BacktestCtx) {}
/// }
///
/// export_strategy!(MyStrategy);
/// ```
#[macro_export]
macro_rules! export_strategy {
    ($ty:ty) => {
        const _: fn() = || {
            fn assert_strategy<T: $crate::OnStrategy>() {}
            assert_strategy::<$ty>();
        };

        static STRATEGY_INSTANCE: ::std::sync::OnceLock<::std::sync::Mutex<$ty>> =
            ::std::sync::OnceLock::new();

        fn get_strategy_instance() -> ::std::sync::MutexGuard<'static, $ty> {
            STRATEGY_INSTANCE
                .get_or_init(|| ::std::sync::Mutex::new(<$ty as $crate::OnStrategy>::new()))
                .lock()
                .expect("strategy mutex poisoned")
        }

        #[unsafe(no_mangle)]
        pub extern "C" fn strategy_on_init(total_bars: usize) {
            <$ty as $crate::OnStrategy>::init(&mut *get_strategy_instance(), total_bars);
        }

        #[unsafe(no_mangle)]
        pub extern "C" fn strategy_restore_state(state: $crate::StateBlob) {
            <$ty as $crate::OnStrategy>::restore_state(&mut *get_strategy_instance(), state);
        }

        #[unsafe(no_mangle)]
        pub extern "C" fn strategy_on_bar(
            bar: *const $crate::Bar,
            index: usize,
            ctx: *mut $crate::Strategy,
        ) {
            let bar = unsafe { &*bar };
            let ctx = unsafe { &mut *ctx };
            <$ty as $crate::OnStrategy>::on_bar(&mut *get_strategy_instance(), bar, index, ctx)
        }

        #[unsafe(no_mangle)]
        pub extern "C" fn strategy_on_finish(ctx: *mut $crate::Strategy) -> $crate::StateBlob {
            let ctx = unsafe { &mut *ctx };
            let state = <$ty as $crate::OnStrategy>::on_finish(&mut *get_strategy_instance(), ctx);
            let mut bytes = std::mem::ManuallyDrop::new(state);
            $crate::StateBlob {
                ptr: bytes.as_mut_ptr(),
                len: bytes.len(),
            }
        }
    };
}

#[macro_export]
macro_rules! export_indicator {
    ($ty:ty) => {
        const _: fn() = || {
            fn assert_indicator<T: $crate::OnIndicator>() {}
            assert_indicator::<$ty>();
        };

        static INDICATOR_INSTANCE: ::std::sync::OnceLock<::std::sync::Mutex<$ty>> =
            ::std::sync::OnceLock::new();

        fn get_indicator_instance() -> ::std::sync::MutexGuard<'static, $ty> {
            INDICATOR_INSTANCE
                .get_or_init(|| ::std::sync::Mutex::new(<$ty as $crate::OnIndicator>::new()))
                .lock()
                .expect("indicator mutex poisoned")
        }

        #[unsafe(no_mangle)]
        pub extern "C" fn indicator_on_init(total_bars: usize) {
            <$ty as $crate::OnIndicator>::init(&mut *get_indicator_instance(), total_bars);
        }

        #[unsafe(no_mangle)]
        pub extern "C" fn indicator_restore_state(state: $crate::StateBlob) {
            <$ty as $crate::OnIndicator>::restore_state(&mut *get_indicator_instance(), state);
        }

        #[unsafe(no_mangle)]
        pub extern "C" fn indicator_on_bar(
            bar: *const $crate::Bar,
            index: usize,
            ctx: *mut $crate::Indicator,
        ) {
            let bar = unsafe { &*bar };
            let ctx = unsafe { &mut *ctx };
            <$ty as $crate::OnIndicator>::on_bar(&mut *get_indicator_instance(), bar, index, ctx)
        }

        #[unsafe(no_mangle)]
        pub extern "C" fn indicator_on_finish(ctx: *mut $crate::Indicator) -> $crate::StateBlob {
            let ctx = unsafe { &mut *ctx };
            let state =
                <$ty as $crate::OnIndicator>::on_finish(&mut *get_indicator_instance(), ctx);
            let mut bytes = std::mem::ManuallyDrop::new(state);
            $crate::StateBlob {
                ptr: bytes.as_mut_ptr(),
                len: bytes.len(),
            }
        }
    };
}
