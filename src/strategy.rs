use std::convert::TryFrom;

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;

use crate::Bar;
use crate::bar::Point;
use crate::indicator::Indicator;
use crate::order_sys::execution_report::ExecutionReport;
use crate::order_sys::fill::Fill;
use crate::order_sys::holding::{Holding, LotReliefMethod};
use crate::order_sys::order::{Order, OrderSide, OrderType, TimeInForce};
use crate::order_sys::system::{
    AppliedFill, FlattenPositionRequest, OrderSystem, OrderSystemError, SubmitCloseOrderRequest,
    SubmitOrderRequest, SubmittedOrder,
};
use crate::order_sys::{FillId, OrderId};

/// Strategy-facing exposure direction for opening orders.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    /// Open or add long exposure.
    Long,
    /// Open or add short exposure.
    Short,
}

impl Side {
    /// Converts the strategy-side intent into a concrete order side.
    fn to_open_order_side(self) -> OrderSide {
        match self {
            Self::Long => OrderSide::Buy,
            Self::Short => OrderSide::Sell,
        }
    }
}

/// Strategy-facing order style.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum OrderKind {
    /// Immediate execution at the best available price.
    Market,
    /// Execute only at the given limit price or better.
    Limit {
        /// Limit price requested by the strategy.
        limit_price: f64,
    },
    /// Trigger when the stop price is crossed.
    Stop {
        /// Stop price requested by the strategy.
        stop_price: f64,
    },
    /// Trigger at a stop price and then behave like a limit order.
    StopLimit {
        /// Stop trigger price.
        stop_price: f64,
        /// Limit price after the stop triggers.
        limit_price: f64,
    },
}

impl OrderKind {
    /// Converts the strategy-facing order style into order-system parameters.
    fn to_order_params(
        self,
    ) -> Result<(OrderType, Option<Decimal>, Option<Decimal>), StrategyError> {
        match self {
            Self::Market => Ok((OrderType::Market, None, None)),
            Self::Limit { limit_price } => Ok((
                OrderType::Limit,
                Some(decimal_from_f64("limit_price", limit_price)?),
                None,
            )),
            Self::Stop { stop_price } => Ok((
                OrderType::Stop,
                None,
                Some(decimal_from_f64("stop_price", stop_price)?),
            )),
            Self::StopLimit {
                stop_price,
                limit_price,
            } => Ok((
                OrderType::StopLimit,
                Some(decimal_from_f64("limit_price", limit_price)?),
                Some(decimal_from_f64("stop_price", stop_price)?),
            )),
        }
    }
}

/// Errors raised by the strategy convenience layer.
#[derive(Clone, Debug, PartialEq)]
pub enum StrategyError {
    /// A context-dependent method was called before any bar was provided.
    MissingBarContext,
    /// A symbol-dependent method was called without setting a default symbol.
    MissingDefaultSymbol,
    /// A floating-point input could not be converted into a finite decimal.
    InvalidNumber {
        /// Name of the invalid field.
        field: &'static str,
        /// Original floating-point value.
        value: f64,
    },
    /// Wrapped error bubbled up from the order system.
    OrderSystem(OrderSystemError),
}

impl std::fmt::Display for StrategyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingBarContext => write!(
                f,
                "strategy context has no current bar; call from `on_bar` or set time explicitly through the lower-level order system"
            ),
            Self::MissingDefaultSymbol => write!(
                f,
                "strategy context has no default symbol; call `set_default_symbol` or use the lower-level order system"
            ),
            Self::InvalidNumber { field, value } => {
                write!(
                    f,
                    "field `{field}` contains an invalid numeric value {value}"
                )
            }
            Self::OrderSystem(err) => err.fmt(f),
        }
    }
}

impl std::error::Error for StrategyError {}

impl From<OrderSystemError> for StrategyError {
    fn from(value: OrderSystemError) -> Self {
        Self::OrderSystem(value)
    }
}

/// Strategy runtime context passed into [`OnStrategy::on_bar`].
///
/// This type is intentionally separate from the user-defined strategy state.
/// It behaves like a backtest/live-trading context that owns:
///
/// - an [`OrderSystem`] facade,
/// - the current bar timestamp/index,
/// - simple visual markers such as buy/sell dots.
///
/// The host provides `&mut Strategy` to every `on_bar` callback. The export
/// macro updates the current bar context automatically before your strategy
/// code runs, so common calls such as [`Self::submit_order`], [`Self::close`],
/// and [`Self::flatten`] can infer the submission timestamp from the active bar.
#[derive(Clone, Debug)]
pub struct Strategy {
    /// High-level order-system facade.
    order_system: OrderSystem,
    /// Optional default symbol used by shorthand methods.
    default_symbol: Option<String>,
    /// Timestamp of the current bar, if the host has entered one.
    current_bar_time: Option<DateTime<Utc>>,
    /// Index of the current bar, if the host has entered one.
    current_bar_index: Option<usize>,
    /// Buy markers recorded by the strategy.
    buy_markers: Vec<Point>,
    /// Sell markers recorded by the strategy.
    sell_markers: Vec<Point>,
}

impl Default for Strategy {
    fn default() -> Self {
        Self::new(LotReliefMethod::Fifo)
    }
}

impl Strategy {
    /// Creates a new strategy runtime context.
    pub fn new(default_lot_relief_method: LotReliefMethod) -> Self {
        Self {
            order_system: OrderSystem::new(default_lot_relief_method),
            default_symbol: None,
            current_bar_time: None,
            current_bar_index: None,
            buy_markers: Vec::new(),
            sell_markers: Vec::new(),
        }
    }

    /// Replaces the default symbol used by shorthand order methods.
    pub fn set_default_symbol(&mut self, symbol: impl Into<String>) {
        self.default_symbol = Some(symbol.into());
    }

    /// Returns the default symbol used by shorthand order methods.
    pub fn default_symbol(&self) -> Option<&str> {
        self.default_symbol.as_deref()
    }

    /// Returns the timestamp of the current bar if one is active.
    pub fn current_bar_time(&self) -> Option<DateTime<Utc>> {
        self.current_bar_time
    }

    /// Returns the index of the current bar if one is active.
    pub fn current_bar_index(&self) -> Option<usize> {
        self.current_bar_index
    }

    /// Returns the underlying order system for advanced workflows.
    pub fn order_system(&self) -> &OrderSystem {
        &self.order_system
    }

    /// Returns mutable access to the underlying order system for advanced workflows.
    pub fn order_system_mut(&mut self) -> &mut OrderSystem {
        &mut self.order_system
    }

    /// Returns one tracked order.
    pub fn order(&self, order_id: OrderId) -> Option<&Order> {
        self.order_system.order(order_id)
    }

    /// Returns one applied fill.
    pub fn fill(&self, fill_id: FillId) -> Option<&Fill> {
        self.order_system.fill(fill_id)
    }

    /// Returns one symbol holding.
    pub fn holding(&self, symbol: &str) -> Option<&Holding> {
        self.order_system.holding(symbol)
    }

    /// Returns the execution report history.
    pub fn execution_reports(&self) -> &[ExecutionReport] {
        self.order_system.execution_reports()
    }

    /// Returns the recorded buy markers.
    pub fn buy_markers(&self) -> &[Point] {
        &self.buy_markers
    }

    /// Returns the recorded sell markers.
    pub fn sell_markers(&self) -> &[Point] {
        &self.sell_markers
    }

    /// Clears all stored buy and sell markers.
    pub fn clear_markers(&mut self) {
        self.buy_markers.clear();
        self.sell_markers.clear();
    }

    /// Submits an opening order for the default symbol at the current bar time.
    pub fn submit_order(
        &mut self,
        side: Side,
        order_kind: OrderKind,
        qty: f64,
        allow_reversal: bool,
    ) -> Result<SubmittedOrder, StrategyError> {
        let symbol = self.require_default_symbol()?.to_owned();
        self.submit_order_for(symbol, side, order_kind, qty, allow_reversal)
    }

    /// Submits an opening order for an explicit symbol at the current bar time.
    pub fn submit_order_for(
        &mut self,
        symbol: impl Into<String>,
        side: Side,
        order_kind: OrderKind,
        qty: f64,
        allow_reversal: bool,
    ) -> Result<SubmittedOrder, StrategyError> {
        let submitted_at = self.require_current_bar_time()?;
        let (order_type, limit_price, stop_price) = order_kind.to_order_params()?;
        let qty = decimal_from_f64("qty", qty)?;

        self.order_system
            .submit_order(
                SubmitOrderRequest::new(
                    symbol,
                    side.to_open_order_side(),
                    crate::order_sys::order::PositionEffect::Open,
                    order_type,
                    TimeInForce::Day,
                    submitted_at,
                    qty,
                    limit_price,
                    stop_price,
                )
                .with_allow_reversal(allow_reversal),
            )
            .map_err(StrategyError::from)
    }

    /// Submits a closing order for the default symbol at the current bar time.
    pub fn close(
        &mut self,
        qty: f64,
        order_kind: OrderKind,
    ) -> Result<SubmittedOrder, StrategyError> {
        let symbol = self.require_default_symbol()?.to_owned();
        self.close_for(symbol, qty, order_kind)
    }

    /// Submits a closing order for an explicit symbol at the current bar time.
    pub fn close_for(
        &mut self,
        symbol: impl Into<String>,
        qty: f64,
        order_kind: OrderKind,
    ) -> Result<SubmittedOrder, StrategyError> {
        let submitted_at = self.require_current_bar_time()?;
        let (order_type, limit_price, stop_price) = order_kind.to_order_params()?;
        let qty = decimal_from_f64("qty", qty)?;

        self.order_system
            .submit_close_order(SubmitCloseOrderRequest::new(
                symbol,
                submitted_at,
                qty,
                order_type,
                TimeInForce::Day,
                limit_price,
                stop_price,
            ))
            .map_err(StrategyError::from)
    }

    /// Submits a flatten request for the default symbol at the current bar time.
    pub fn flatten(&mut self, order_kind: OrderKind) -> Result<SubmittedOrder, StrategyError> {
        let symbol = self.require_default_symbol()?.to_owned();
        self.flatten_for(symbol, order_kind)
    }

    /// Submits a flatten request for an explicit symbol at the current bar time.
    pub fn flatten_for(
        &mut self,
        symbol: impl Into<String>,
        order_kind: OrderKind,
    ) -> Result<SubmittedOrder, StrategyError> {
        let submitted_at = self.require_current_bar_time()?;
        let (order_type, limit_price, stop_price) = order_kind.to_order_params()?;
        let symbol = symbol.into();
        let mut request =
            FlattenPositionRequest::new(symbol, submitted_at, order_type, TimeInForce::Day);
        if let Some(limit_price) = limit_price {
            request = request.with_limit_price(limit_price);
        }
        if let Some(stop_price) = stop_price {
            request = request.with_stop_price(stop_price);
        }

        self.order_system
            .flatten_position(request)
            .map_err(StrategyError::from)
    }

    /// Applies one fill to the underlying order system.
    pub fn apply_fill(&mut self, fill: Fill) -> Result<AppliedFill, StrategyError> {
        self.order_system
            .apply_fill(fill)
            .map_err(StrategyError::from)
    }

    /// Marks one order as acknowledged.
    pub fn acknowledge_order(
        &mut self,
        order_id: OrderId,
        acknowledged_at: DateTime<Utc>,
    ) -> Result<ExecutionReport, StrategyError> {
        self.order_system
            .acknowledge_order(order_id, acknowledged_at)
            .map_err(StrategyError::from)
    }

    /// Cancels one order.
    pub fn cancel_order(
        &mut self,
        order_id: OrderId,
        canceled_at: DateTime<Utc>,
    ) -> Result<ExecutionReport, StrategyError> {
        self.order_system
            .cancel_order(order_id, canceled_at)
            .map_err(StrategyError::from)
    }

    /// Rejects one order with a reason.
    pub fn reject_order(
        &mut self,
        order_id: OrderId,
        rejected_at: DateTime<Utc>,
        reason: impl Into<String>,
    ) -> Result<ExecutionReport, StrategyError> {
        self.order_system
            .reject_order(order_id, rejected_at, reason)
            .map_err(StrategyError::from)
    }

    /// Returns unreserved closeable quantity for one symbol.
    pub fn available_close_qty(&self, symbol: &str) -> Result<Decimal, StrategyError> {
        self.order_system
            .available_close_qty(symbol)
            .map_err(StrategyError::from)
    }

    /// Records a buy marker at the current bar index.
    pub fn mark_buy(&mut self, price: f32) -> Result<(), StrategyError> {
        let index = self.require_current_bar_index()?;
        self.buy_markers.push(Point {
            index,
            price,
            color: [34, 197, 94, 255],
        });
        Ok(())
    }

    /// Records a sell marker at the current bar index.
    pub fn mark_sell(&mut self, price: f32) -> Result<(), StrategyError> {
        let index = self.require_current_bar_index()?;
        self.sell_markers.push(Point {
            index,
            price,
            color: [239, 68, 68, 255],
        });
        Ok(())
    }

    /// Internal hook used by the export macro to update bar-scoped context.
    pub fn begin_bar(&mut self, bar: &Bar, index: usize) {
        self.current_bar_time = Some(bar.time);
        self.current_bar_index = Some(index);
    }

    /// Returns the default symbol or fails with a strategy-level error.
    fn require_default_symbol(&self) -> Result<&str, StrategyError> {
        self.default_symbol
            .as_deref()
            .ok_or(StrategyError::MissingDefaultSymbol)
    }

    /// Returns the current bar timestamp or fails with a strategy-level error.
    fn require_current_bar_time(&self) -> Result<DateTime<Utc>, StrategyError> {
        self.current_bar_time
            .ok_or(StrategyError::MissingBarContext)
    }

    /// Returns the current bar index or fails with a strategy-level error.
    fn require_current_bar_index(&self) -> Result<usize, StrategyError> {
        self.current_bar_index
            .ok_or(StrategyError::MissingBarContext)
    }
}

/// Converts one floating-point value into a trusted decimal.
fn decimal_from_f64(field: &'static str, value: f64) -> Result<Decimal, StrategyError> {
    if !value.is_finite() {
        return Err(StrategyError::InvalidNumber { field, value });
    }

    Decimal::try_from(value).map_err(|_| StrategyError::InvalidNumber { field, value })
}

// Signal ----------------------------------------------------------------------------

pub type Signal = i32;

pub const BUY: Signal = 1;
pub const SELL: Signal = -1;
pub const HOLD: Signal = 0;

#[repr(C)]
pub struct StateBlob {
    pub ptr: *mut u8,
    pub len: usize,
}

// Fn-pointer types (used by the host via libloading) --------------------------------

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
/// `new` -> `init` -> `on_bar` x N -> `on_finish`
pub trait OnStrategy {
    /// Construct the strategy. Called once when the DLL singleton is first accessed.
    fn new() -> Self
    where
        Self: Sized;

    fn restore_state(&mut self, state: StateBlob);

    /// Called once before the first bar. Use this to reset any state.
    fn init(&mut self, total_bars: usize);

    /// Called for every bar in order.
    fn on_bar(&mut self, bar: &Bar, index: usize, ctx: &mut Strategy);

    /// Called once after the last bar.
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
    fn on_bar(&mut self, bar: &Bar, index: usize, ctx: &mut Indicator);

    /// Called once after the last bar.
    fn on_finish(&mut self, ctx: &mut Indicator) -> Vec<u8>;
}

// Export macro ----------------------------------------------------------------------

/// Wire up an [`OnStrategy`] impl as a loadable DLL.
///
/// # Example
///
/// ```rust,ignore
/// use strategy_api::{Bar, OnStrategy, OrderKind, Side, Strategy, StateBlob, export_strategy};
///
/// struct MyStrategy {
///     prev_close: f32,
/// }
///
/// impl OnStrategy for MyStrategy {
///     fn new() -> Self {
///         Self { prev_close: 0.0 }
///     }
///
///     fn restore_state(&mut self, _state: StateBlob) {}
///
///     fn init(&mut self, _total_bars: usize) {
///         self.prev_close = 0.0;
///     }
///
///     fn on_bar(&mut self, bar: &Bar, _index: usize, ctx: &mut Strategy) {
///         ctx.set_default_symbol("AAPL");
///
///         if bar.close > self.prev_close {
///             let _ = ctx.submit_order(Side::Long, OrderKind::Market, 1.0, false);
///             let _ = ctx.mark_buy(bar.close);
///         } else if bar.close < self.prev_close {
///             let _ = ctx.close(1.0, OrderKind::Market);
///             let _ = ctx.mark_sell(bar.close);
///         }
///
///         self.prev_close = bar.close;
///     }
///
///     fn on_finish(&mut self, _ctx: &mut Strategy) -> Vec<u8> {
///         Vec::new()
///     }
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

        static INSTANCE: ::std::sync::OnceLock<::std::sync::Mutex<$ty>> =
            ::std::sync::OnceLock::new();

        fn get_instance() -> ::std::sync::MutexGuard<'static, $ty> {
            INSTANCE
                .get_or_init(|| ::std::sync::Mutex::new(<$ty as $crate::OnStrategy>::new()))
                .lock()
                .expect("strategy mutex poisoned")
        }

        #[unsafe(no_mangle)]
        pub extern "C" fn on_init(total_bars: usize) {
            <$ty as $crate::OnStrategy>::init(&mut *get_instance(), total_bars);
        }

        #[unsafe(no_mangle)]
        pub extern "C" fn restore_state(state: $crate::StateBlob) {
            <$ty as $crate::OnStrategy>::restore_state(&mut *get_instance(), state);
        }

        #[unsafe(no_mangle)]
        pub extern "C" fn on_bar(
            bar: *const $crate::Bar,
            index: usize,
            ctx: *mut $crate::Strategy,
        ) {
            let bar = unsafe { &*bar };
            let ctx = unsafe { &mut *ctx };
            ctx.begin_bar(bar, index);
            <$ty as $crate::OnStrategy>::on_bar(&mut *get_instance(), bar, index, ctx)
        }

        #[unsafe(no_mangle)]
        pub extern "C" fn on_finish(ctx: *mut $crate::Strategy) -> $crate::StateBlob {
            let ctx = unsafe { &mut *ctx };
            let state = <$ty as $crate::OnStrategy>::on_finish(&mut *get_instance(), ctx);
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

        static INSTANCE: ::std::sync::OnceLock<::std::sync::Mutex<$ty>> =
            ::std::sync::OnceLock::new();

        fn get_instance() -> ::std::sync::MutexGuard<'static, $ty> {
            INSTANCE
                .get_or_init(|| ::std::sync::Mutex::new(<$ty as $crate::OnIndicator>::new()))
                .lock()
                .expect("indicator mutex poisoned")
        }

        #[unsafe(no_mangle)]
        pub extern "C" fn on_init(total_bars: usize) {
            <$ty as $crate::OnIndicator>::init(&mut *get_instance(), total_bars);
        }

        #[unsafe(no_mangle)]
        pub extern "C" fn restore_state(state: $crate::StateBlob) {
            <$ty as $crate::OnIndicator>::restore_state(&mut *get_instance(), state);
        }

        #[unsafe(no_mangle)]
        pub extern "C" fn on_bar(
            bar: *const $crate::Bar,
            index: usize,
            ctx: *mut $crate::Indicator,
        ) {
            let bar = unsafe { &*bar };
            let ctx = unsafe { &mut *ctx };
            <$ty as $crate::OnIndicator>::on_bar(&mut *get_instance(), bar, index, ctx)
        }

        #[unsafe(no_mangle)]
        pub extern "C" fn on_finish(ctx: *mut $crate::Indicator) -> $crate::StateBlob {
            let ctx = unsafe { &mut *ctx };
            let state = <$ty as $crate::OnIndicator>::on_finish(&mut *get_instance(), ctx);
            let mut bytes = std::mem::ManuallyDrop::new(state);
            $crate::StateBlob {
                ptr: bytes.as_mut_ptr(),
                len: bytes.len(),
            }
        }
    };
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};
    use rust_decimal::Decimal;

    use super::{OrderKind, Side, Strategy, StrategyError};
    use crate::Bar;
    use crate::order_sys::fill::Fill;
    use crate::order_sys::holding::LotReliefMethod;
    use crate::order_sys::order::{OrderSide, OrderStatus, PositionEffect};
    use crate::order_sys::{FillId, OrderId};

    fn bar(day: u32, close: f32) -> Bar {
        Bar {
            time: Utc.with_ymd_and_hms(2026, 5, day, 9, 30, 0).unwrap(),
            open: close,
            high: close,
            low: close,
            close,
            volume: 1.0,
        }
    }

    fn open_fill(order_id: u64, fill_id: u64, day: u32, qty: i64, price: i64) -> Fill {
        Fill::new(
            FillId::new(fill_id),
            OrderId::new(order_id),
            "AAPL",
            OrderSide::Buy,
            PositionEffect::Open,
            Utc.with_ymd_and_hms(2026, 5, day, 9, 31, 0).unwrap(),
            Decimal::new(qty, 0),
            Decimal::new(price, 0),
            Decimal::ZERO,
        )
        .unwrap()
    }

    #[test]
    fn strategy_submit_order_uses_default_symbol_and_bar_context() {
        let mut ctx = Strategy::new(LotReliefMethod::Fifo);
        ctx.set_default_symbol("AAPL");
        ctx.begin_bar(&bar(24, 100.0), 7);

        let submission = ctx
            .submit_order(Side::Long, OrderKind::Market, 1.0, false)
            .unwrap();

        assert_eq!(submission.order.symbol(), "AAPL");
        assert_eq!(submission.order.side(), OrderSide::Buy);
        assert_eq!(submission.order.status(), OrderStatus::Pending);
        assert!(submission.auto_close_order.is_none());
        assert_eq!(ctx.current_bar_index(), Some(7));
    }

    #[test]
    fn strategy_close_and_flatten_delegate_to_order_system() {
        let mut ctx = Strategy::new(LotReliefMethod::Fifo);
        ctx.set_default_symbol("AAPL");
        ctx.begin_bar(&bar(24, 100.0), 0);

        let open = ctx
            .submit_order(Side::Long, OrderKind::Market, 2.0, false)
            .unwrap();
        ctx.apply_fill(open_fill(open.order.id().value(), 10, 24, 2, 100))
            .unwrap();

        ctx.begin_bar(&bar(25, 101.0), 1);
        let close = ctx.close(1.0, OrderKind::Market).unwrap();
        let flatten = ctx.flatten(OrderKind::Market).unwrap();

        assert_eq!(close.order.position_effect(), PositionEffect::Close);
        assert_eq!(close.order.side(), OrderSide::Sell);
        assert_eq!(flatten.order.requested_qty(), Decimal::ONE);
    }

    #[test]
    fn strategy_submit_order_can_request_auto_reversal() {
        let mut ctx = Strategy::new(LotReliefMethod::Fifo);
        ctx.set_default_symbol("AAPL");
        ctx.begin_bar(&bar(24, 100.0), 0);

        let open = ctx
            .submit_order(Side::Long, OrderKind::Market, 2.0, false)
            .unwrap();
        ctx.apply_fill(open_fill(open.order.id().value(), 10, 24, 2, 100))
            .unwrap();

        ctx.begin_bar(&bar(25, 99.0), 1);
        let reversal = ctx
            .submit_order(Side::Short, OrderKind::Market, 1.0, true)
            .unwrap();

        assert_eq!(reversal.order.side(), OrderSide::Sell);
        assert_eq!(reversal.order.position_effect(), PositionEffect::Open);
        assert!(reversal.auto_close_order.is_some());
        assert_eq!(
            reversal
                .auto_close_order
                .as_ref()
                .expect("reversal should stage an auto-close order")
                .order
                .position_effect(),
            PositionEffect::Close
        );
    }

    #[test]
    fn strategy_markers_track_current_bar_index() {
        let mut ctx = Strategy::default();
        ctx.begin_bar(&bar(24, 100.0), 3);

        ctx.mark_buy(100.0).unwrap();
        ctx.mark_sell(99.0).unwrap();

        assert_eq!(ctx.buy_markers()[0].index, 3);
        assert_eq!(ctx.sell_markers()[0].index, 3);
    }

    #[test]
    fn strategy_rejects_contextless_shorthand_calls() {
        let mut ctx = Strategy::default();

        let err = ctx
            .submit_order(Side::Long, OrderKind::Market, 1.0, false)
            .unwrap_err();
        assert_eq!(err, StrategyError::MissingDefaultSymbol);

        ctx.set_default_symbol("AAPL");
        let err = ctx
            .submit_order(Side::Long, OrderKind::Market, 1.0, false)
            .unwrap_err();
        assert_eq!(err, StrategyError::MissingBarContext);
    }
}
