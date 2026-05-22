use crate::{Bar, Context};

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

// ── Metadata ──────────────────────────────────────────────────────────────────

#[repr(C)]
pub struct StrategyInfo {
    pub name: *const u8,
    pub name_len: usize,
    pub description: *const u8,
    pub description_len: usize,
    pub version: *const u8,
    pub version_len: usize,
}

// ── Fn-pointer types (used by the host via libloading) ────────────────────────

pub type FnStrategyInfo = unsafe extern "C" fn() -> StrategyInfo;
pub type FnOnInit = unsafe extern "C" fn(total_bars: usize);
pub type FnOnBar = unsafe extern "C" fn(bar: *const Bar, index: usize, ctx: *mut Context);
pub type FnOnFinish = unsafe extern "C" fn(ctx: *mut Context);

// ── OnBar trait ────────────────────────────────────────────────────────────

/// Implement this trait in your DLL, then call `export_strategy!(YourType)`.
///
/// All methods are called from the host's main thread in order:
///   `new` → `init` → `on_bar` × N → `on_finish`
pub trait OnBar {
    /// Construct the strategy. Called once when the DLL singleton is first accessed.
    fn new() -> Self
    where
        Self: Sized;

    /// Display name shown in the UI strategy list.
    fn name() -> &'static str
    where
        Self: Sized;

    /// One-line description (may be empty).
    fn description() -> &'static str
    where
        Self: Sized;

    /// Semantic version string, e.g. `"1.0.0"`.
    fn version() -> &'static str
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
    fn on_bar(&mut self, bar: &Bar, index: usize, ctx: &mut Context);

    /// Called once after the last bar. Use this to draw final annotations.
    fn on_finish(&mut self, ctx: &mut Context) -> Vec<u8>;
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
            fn assert_strategy<T: $crate::OnBar>() {}
            assert_strategy::<$ty>();
        };

        static INSTANCE: ::std::sync::OnceLock<::std::sync::Mutex<$ty>> =
            ::std::sync::OnceLock::new();

        fn get_instance() -> ::std::sync::MutexGuard<'static, $ty> {
            INSTANCE
                .get_or_init(|| ::std::sync::Mutex::new(<$ty as $crate::OnBar>::new()))
                .lock()
                .expect("strategy mutex poisoned")
        }

        #[unsafe(no_mangle)]
        pub extern "C" fn strategy_info() -> $crate::StrategyInfo {
            let name = <$ty as $crate::OnBar>::name();
            let desc = <$ty as $crate::OnBar>::description();
            let ver = <$ty as $crate::OnBar>::version();
            $crate::StrategyInfo {
                name: name.as_ptr(),
                name_len: name.len(),
                description: desc.as_ptr(),
                description_len: desc.len(),
                version: ver.as_ptr(),
                version_len: ver.len(),
            }
        }

        #[unsafe(no_mangle)]
        pub extern "C" fn on_init(total_bars: usize) {
            <$ty as $crate::OnBar>::init(&mut *get_instance(), total_bars);
        }

        #[unsafe(no_mangle)]
        pub extern "C" fn restore_state(state: $crate::StateBlob) {
            <$ty as $crate::OnBar>::restore_state(&mut *get_instance(), state);
        }

        #[unsafe(no_mangle)]
        pub extern "C" fn on_bar(bar: *const $crate::Bar, index: usize, ctx: *mut $crate::Context) {
            let bar = unsafe { &*bar };
            let ctx = unsafe { &mut *ctx };
            <$ty as $crate::OnBar>::on_bar(&mut *get_instance(), bar, index, ctx)
        }

        #[unsafe(no_mangle)]
        pub extern "C" fn on_finish(ctx: *mut $crate::Context) -> $crate::StateBlob {
            let ctx = unsafe { &mut *ctx };
            let state = <$ty as $crate::OnBar>::on_finish(&mut *get_instance(), ctx);
            let mut bytes = std::mem::ManuallyDrop::new(state);
            $crate::StateBlob {
                ptr: bytes.as_mut_ptr(),
                len: bytes.len(),
            }
        }
    };
}
