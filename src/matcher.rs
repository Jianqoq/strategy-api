//! Execution-model helpers that translate market data into fills.
//!
//! [`crate::order_sys::system::OrderSystem`] is intentionally not a matching
//! engine. This module provides thin execution models that:
//!
//! - inspect currently live orders,
//! - decide whether one market event would execute them,
//! - build immutable [`crate::order_sys::fill::Fill`] values, and
//! - feed those fills back into the order system.
//!
//! The initial implementation keeps the surface area small:
//!
//! - [`BarMatcher`] supports OHLC-bar matching with conservative stop-limit
//!   semantics. A stop-limit touched intrabar becomes active only from the next
//!   bar onward. Gap-through openings can still trigger and fill immediately.
//! - [`TickMatcher`] supports direct tick-by-tick matching. Stop-limit orders
//!   may trigger and fill on the same tick because the event ordering is no
//!   longer ambiguous.

use std::collections::HashSet;
use std::convert::TryFrom;

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use thiserror::Error;

use crate::Bar;
use crate::order_sys::execution_report::ExecutionReport;
use crate::order_sys::fill::{Fill, FillError};
use crate::order_sys::order::{Order, OrderSide, OrderStatus, OrderType};
use crate::order_sys::system::{AppliedFill, OrderSystem, OrderSystemError};
use crate::order_sys::{FillId, OrderId};

/// Whether touching a price level is sufficient to execute, or whether the
/// market must strictly trade through the price.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PriceTrigger {
    /// The first touch of the relevant price level executes the order.
    Touch,
    /// The market must move strictly beyond the relevant price level.
    TradeThrough,
}

/// Price used for market-order execution in bar mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BarMarketFillPrice {
    /// Fill at the bar open once the order becomes eligible.
    Open,
    /// Fill at the bar close once the order becomes eligible.
    Close,
}

/// Configuration for [`BarMatcher`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BarMatcherConfig {
    /// Whether touches are enough to execute limit/stop orders intrabar.
    pub price_trigger: PriceTrigger,
    /// Price used to fill market orders in bar mode.
    pub market_fill_price: BarMarketFillPrice,
    /// Whether orders submitted with the same timestamp as the current bar may
    /// execute on that bar.
    pub allow_same_bar_fill: bool,
}

impl Default for BarMatcherConfig {
    fn default() -> Self {
        Self {
            price_trigger: PriceTrigger::Touch,
            market_fill_price: BarMarketFillPrice::Open,
            allow_same_bar_fill: false,
        }
    }
}

/// Configuration for [`TickMatcher`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TickMatcherConfig {
    /// Whether touching a price level is enough to execute.
    pub price_trigger: PriceTrigger,
    /// Whether orders submitted with the same timestamp as the current tick may
    /// execute on that tick.
    pub allow_same_tick_fill: bool,
}

impl Default for TickMatcherConfig {
    fn default() -> Self {
        Self {
            price_trigger: PriceTrigger::Touch,
            allow_same_tick_fill: false,
        }
    }
}

/// Immutable top-of-book style market event for tick-based matching.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tick {
    /// UTC timestamp of the market event.
    pub time: DateTime<Utc>,
    /// Best bid price.
    pub bid: Decimal,
    /// Best ask price.
    pub ask: Decimal,
    /// Last traded price, if known.
    pub last: Option<Decimal>,
    /// Event volume, if known.
    pub volume: Option<Decimal>,
}

impl Tick {
    /// Creates one validated tick event.
    pub fn new(
        time: DateTime<Utc>,
        bid: Decimal,
        ask: Decimal,
        last: Option<Decimal>,
        volume: Option<Decimal>,
    ) -> Result<Self, TickError> {
        if bid <= Decimal::ZERO {
            return Err(TickError::NonPositiveBid { bid });
        }
        if ask <= Decimal::ZERO {
            return Err(TickError::NonPositiveAsk { ask });
        }
        if ask < bid {
            return Err(TickError::CrossedBook { bid, ask });
        }
        if let Some(last) = last
            && last <= Decimal::ZERO
        {
            return Err(TickError::NonPositiveLast { last });
        }
        if let Some(volume) = volume
            && volume < Decimal::ZERO
        {
            return Err(TickError::NegativeVolume { volume });
        }

        Ok(Self {
            time,
            bid,
            ask,
            last,
            volume,
        })
    }
}

/// Errors raised while constructing or matching ticks.
#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum TickError {
    /// Best bid must be strictly positive.
    #[error("tick bid must be positive, got {bid}")]
    NonPositiveBid {
        /// Invalid bid price.
        bid: Decimal,
    },
    /// Best ask must be strictly positive.
    #[error("tick ask must be positive, got {ask}")]
    NonPositiveAsk {
        /// Invalid ask price.
        ask: Decimal,
    },
    /// Best ask must not be below best bid.
    #[error("tick ask {ask} is below bid {bid}")]
    CrossedBook {
        /// Bid price.
        bid: Decimal,
        /// Ask price.
        ask: Decimal,
    },
    /// Last-traded price must be strictly positive when present.
    #[error("tick last price must be positive, got {last}")]
    NonPositiveLast {
        /// Invalid last price.
        last: Decimal,
    },
    /// Volume must not be negative when present.
    #[error("tick volume cannot be negative, got {volume}")]
    NegativeVolume {
        /// Invalid volume.
        volume: Decimal,
    },
}

/// Reports produced while matching one market event.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MatchResult {
    /// `New` execution reports emitted when pending orders first become live.
    pub acknowledged_reports: Vec<ExecutionReport>,
    /// Trade applications emitted while processing the event.
    pub applied_fills: Vec<AppliedFill>,
}

/// Errors raised by bar- or tick-based matching.
#[derive(Debug, Error)]
pub enum MatchError {
    /// One bar field contained an invalid numeric value.
    #[error("bar field `{field}` contains an invalid numeric value {value}")]
    InvalidBarPrice {
        /// Name of the invalid bar field.
        field: &'static str,
        /// Original invalid value.
        value: f64,
    },
    /// Wrapped fill-construction error.
    #[error(transparent)]
    Fill(#[from] FillError),
    /// Wrapped tick-construction error.
    #[error(transparent)]
    Tick(#[from] TickError),
    /// Wrapped order-system error.
    #[error(transparent)]
    OrderSystem(#[from] OrderSystemError),
}

/// OHLC-bar matcher that converts eligible orders into fills.
#[derive(Clone, Debug)]
pub struct BarMatcher {
    config: BarMatcherConfig,
    triggered_stop_limits: HashSet<OrderId>,
}

impl Default for BarMatcher {
    fn default() -> Self {
        Self::new(BarMatcherConfig::default())
    }
}

impl BarMatcher {
    /// Creates one bar matcher with the supplied configuration.
    pub fn new(config: BarMatcherConfig) -> Self {
        Self {
            config,
            triggered_stop_limits: HashSet::new(),
        }
    }

    /// Returns the current matcher configuration.
    pub fn config(&self) -> BarMatcherConfig {
        self.config
    }

    /// Matches one bar against currently live orders and applies any generated fills.
    pub fn process_bar(
        &mut self,
        bar: &Bar,
        system: &mut OrderSystem,
    ) -> Result<MatchResult, MatchError> {
        self.cleanup_triggered_stop_limits(system);

        let prices = BarPrices::try_from(bar)?;
        let candidates = collect_live_orders(system);
        let mut next_fill_value = next_fill_value(system);
        let mut result = MatchResult::default();

        for order in candidates {
            if !is_event_eligible(
                order.submitted_at(),
                prices.time,
                self.config.allow_same_bar_fill,
            ) {
                continue;
            }

            if !acknowledge_pending_order(system, &order, prices.time, &mut result)? {
                continue;
            }

            let Some(fill_price) = self.match_bar_order(&order, &prices) else {
                continue;
            };

            let fill = build_fill(
                &order,
                FillId::new(next_fill_value),
                prices.time,
                fill_price,
            )?;
            next_fill_value += 1;

            match system.apply_fill(fill) {
                Ok(applied_fill) => {
                    self.triggered_stop_limits.remove(&order.id());
                    result.applied_fills.push(applied_fill);
                }
                Err(OrderSystemError::OrderNotActive { .. }) => continue,
                Err(err) => return Err(err.into()),
            }
        }

        Ok(result)
    }

    fn match_bar_order(&mut self, order: &Order, prices: &BarPrices) -> Option<Decimal> {
        match order.order_type() {
            OrderType::Market => Some(match self.config.market_fill_price {
                BarMarketFillPrice::Open => prices.open,
                BarMarketFillPrice::Close => prices.close,
            }),
            OrderType::Limit => {
                let limit_price = order
                    .limit_price()
                    .expect("limit orders must carry a limit price");
                match_bar_limit(order.side(), limit_price, prices, self.config.price_trigger)
            }
            OrderType::Stop => {
                let stop_price = order
                    .stop_price()
                    .expect("stop orders must carry a stop price");
                match_bar_stop(order.side(), stop_price, prices, self.config.price_trigger)
            }
            OrderType::StopLimit => self.match_bar_stop_limit(order, prices),
        }
    }

    fn match_bar_stop_limit(&mut self, order: &Order, prices: &BarPrices) -> Option<Decimal> {
        let stop_price = order
            .stop_price()
            .expect("stop-limit orders must carry a stop price");
        let limit_price = order
            .limit_price()
            .expect("stop-limit orders must carry a limit price");

        if self.triggered_stop_limits.contains(&order.id()) {
            return match_bar_limit(order.side(), limit_price, prices, self.config.price_trigger);
        }

        if stop_triggers_at_open(order.side(), stop_price, prices.open) {
            self.triggered_stop_limits.insert(order.id());
            return match_bar_limit(order.side(), limit_price, prices, self.config.price_trigger);
        }

        if stop_triggers_intrabar(order.side(), stop_price, prices, self.config.price_trigger) {
            self.triggered_stop_limits.insert(order.id());
        }

        None
    }

    fn cleanup_triggered_stop_limits(&mut self, system: &OrderSystem) {
        self.triggered_stop_limits.retain(|order_id| {
            system.order(*order_id).is_some_and(|order| {
                matches!(
                    order.status(),
                    OrderStatus::Pending | OrderStatus::Working | OrderStatus::PartiallyFilled
                ) && order.order_type() == OrderType::StopLimit
            })
        });
    }
}

/// Tick-by-tick matcher that converts eligible orders into fills.
#[derive(Clone, Debug)]
pub struct TickMatcher {
    config: TickMatcherConfig,
    triggered_stop_limits: HashSet<OrderId>,
}

impl Default for TickMatcher {
    fn default() -> Self {
        Self::new(TickMatcherConfig::default())
    }
}

impl TickMatcher {
    /// Creates one tick matcher with the supplied configuration.
    pub fn new(config: TickMatcherConfig) -> Self {
        Self {
            config,
            triggered_stop_limits: HashSet::new(),
        }
    }

    /// Returns the current matcher configuration.
    pub fn config(&self) -> TickMatcherConfig {
        self.config
    }

    /// Matches one tick against currently live orders and applies any generated fills.
    pub fn process_tick(
        &mut self,
        tick: &Tick,
        system: &mut OrderSystem,
    ) -> Result<MatchResult, MatchError> {
        self.cleanup_triggered_stop_limits(system);

        let candidates = collect_live_orders(system);
        let mut next_fill_value = next_fill_value(system);
        let mut result = MatchResult::default();

        for order in candidates {
            if !is_event_eligible(
                order.submitted_at(),
                tick.time,
                self.config.allow_same_tick_fill,
            ) {
                continue;
            }

            if !acknowledge_pending_order(system, &order, tick.time, &mut result)? {
                continue;
            }

            let Some(fill_price) = self.match_tick_order(&order, tick) else {
                continue;
            };

            let fill = build_fill(&order, FillId::new(next_fill_value), tick.time, fill_price)?;
            next_fill_value += 1;

            match system.apply_fill(fill) {
                Ok(applied_fill) => {
                    self.triggered_stop_limits.remove(&order.id());
                    result.applied_fills.push(applied_fill);
                }
                Err(OrderSystemError::OrderNotActive { .. }) => continue,
                Err(err) => return Err(err.into()),
            }
        }

        Ok(result)
    }

    fn match_tick_order(&mut self, order: &Order, tick: &Tick) -> Option<Decimal> {
        match order.order_type() {
            OrderType::Market => Some(match order.side() {
                OrderSide::Buy => tick.ask,
                OrderSide::Sell => tick.bid,
            }),
            OrderType::Limit => {
                let limit_price = order
                    .limit_price()
                    .expect("limit orders must carry a limit price");
                match_tick_limit(order.side(), limit_price, tick, self.config.price_trigger)
            }
            OrderType::Stop => {
                let stop_price = order
                    .stop_price()
                    .expect("stop orders must carry a stop price");
                match_tick_stop(order.side(), stop_price, tick, self.config.price_trigger)
            }
            OrderType::StopLimit => self.match_tick_stop_limit(order, tick),
        }
    }

    fn match_tick_stop_limit(&mut self, order: &Order, tick: &Tick) -> Option<Decimal> {
        let stop_price = order
            .stop_price()
            .expect("stop-limit orders must carry a stop price");
        let limit_price = order
            .limit_price()
            .expect("stop-limit orders must carry a limit price");

        if self.triggered_stop_limits.contains(&order.id()) {
            return match_tick_limit(order.side(), limit_price, tick, self.config.price_trigger);
        }

        if stop_triggers_on_tick(order.side(), stop_price, tick, self.config.price_trigger) {
            self.triggered_stop_limits.insert(order.id());
            return match_tick_limit(order.side(), limit_price, tick, self.config.price_trigger);
        }

        None
    }

    fn cleanup_triggered_stop_limits(&mut self, system: &OrderSystem) {
        self.triggered_stop_limits.retain(|order_id| {
            system.order(*order_id).is_some_and(|order| {
                matches!(
                    order.status(),
                    OrderStatus::Pending | OrderStatus::Working | OrderStatus::PartiallyFilled
                ) && order.order_type() == OrderType::StopLimit
            })
        });
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct BarPrices {
    time: DateTime<Utc>,
    open: Decimal,
    high: Decimal,
    low: Decimal,
    close: Decimal,
}

impl TryFrom<&Bar> for BarPrices {
    type Error = MatchError;

    fn try_from(bar: &Bar) -> Result<Self, Self::Error> {
        Ok(Self {
            time: bar.time,
            open: decimal_from_f32("open", bar.open)?,
            high: decimal_from_f32("high", bar.high)?,
            low: decimal_from_f32("low", bar.low)?,
            close: decimal_from_f32("close", bar.close)?,
        })
    }
}

fn decimal_from_f32(field: &'static str, value: f32) -> Result<Decimal, MatchError> {
    Decimal::try_from(f64::from(value)).map_err(|_| MatchError::InvalidBarPrice {
        field,
        value: f64::from(value),
    })
}

fn collect_live_orders(system: &OrderSystem) -> Vec<Order> {
    let mut orders: Vec<Order> = system
        .orders()
        .filter(|order| is_live_status(order.status()))
        .cloned()
        .collect();
    orders.sort_by(|left, right| {
        left.submitted_at()
            .cmp(&right.submitted_at())
            .then_with(|| left.id().cmp(&right.id()))
    });
    orders
}

fn is_live_status(status: OrderStatus) -> bool {
    matches!(
        status,
        OrderStatus::Pending | OrderStatus::Working | OrderStatus::PartiallyFilled
    )
}

fn is_event_eligible(
    submitted_at: DateTime<Utc>,
    event_time: DateTime<Utc>,
    allow_same: bool,
) -> bool {
    submitted_at < event_time || (allow_same && submitted_at == event_time)
}

fn next_fill_value(system: &OrderSystem) -> u64 {
    system
        .fills()
        .map(|fill| fill.id().value())
        .max()
        .unwrap_or(0)
        + 1
}

fn acknowledge_pending_order(
    system: &mut OrderSystem,
    order: &Order,
    acknowledged_at: DateTime<Utc>,
    result: &mut MatchResult,
) -> Result<bool, MatchError> {
    if order.status() != OrderStatus::Pending {
        return Ok(true);
    }

    match system.acknowledge_order(order.id(), acknowledged_at) {
        Ok(report) => {
            result.acknowledged_reports.push(report);
            Ok(true)
        }
        Err(OrderSystemError::OrderNotActive { .. }) => Ok(false),
        Err(err) => Err(err.into()),
    }
}

fn build_fill(
    order: &Order,
    fill_id: FillId,
    executed_at: DateTime<Utc>,
    price: Decimal,
) -> Result<Fill, MatchError> {
    Fill::new(
        fill_id,
        order.id(),
        order.symbol(),
        order.side(),
        order.position_effect(),
        executed_at,
        order.leaves_qty(),
        price,
        Decimal::ZERO,
    )
    .map_err(MatchError::from)
}

fn match_bar_limit(
    side: OrderSide,
    limit_price: Decimal,
    prices: &BarPrices,
    trigger: PriceTrigger,
) -> Option<Decimal> {
    match side {
        OrderSide::Buy => {
            if prices.open <= limit_price {
                Some(prices.open)
            } else if touches_down(prices.low, limit_price, trigger) {
                Some(limit_price)
            } else {
                None
            }
        }
        OrderSide::Sell => {
            if prices.open >= limit_price {
                Some(prices.open)
            } else if touches_up(prices.high, limit_price, trigger) {
                Some(limit_price)
            } else {
                None
            }
        }
    }
}

fn match_bar_stop(
    side: OrderSide,
    stop_price: Decimal,
    prices: &BarPrices,
    trigger: PriceTrigger,
) -> Option<Decimal> {
    match side {
        OrderSide::Buy => {
            if prices.open >= stop_price {
                Some(prices.open)
            } else if touches_up(prices.high, stop_price, trigger) {
                Some(stop_price)
            } else {
                None
            }
        }
        OrderSide::Sell => {
            if prices.open <= stop_price {
                Some(prices.open)
            } else if touches_down(prices.low, stop_price, trigger) {
                Some(stop_price)
            } else {
                None
            }
        }
    }
}

fn stop_triggers_at_open(side: OrderSide, stop_price: Decimal, open_price: Decimal) -> bool {
    match side {
        OrderSide::Buy => open_price >= stop_price,
        OrderSide::Sell => open_price <= stop_price,
    }
}

fn stop_triggers_intrabar(
    side: OrderSide,
    stop_price: Decimal,
    prices: &BarPrices,
    trigger: PriceTrigger,
) -> bool {
    match side {
        OrderSide::Buy => touches_up(prices.high, stop_price, trigger),
        OrderSide::Sell => touches_down(prices.low, stop_price, trigger),
    }
}

fn match_tick_limit(
    side: OrderSide,
    limit_price: Decimal,
    tick: &Tick,
    trigger: PriceTrigger,
) -> Option<Decimal> {
    match side {
        OrderSide::Buy => {
            if touches_down(tick.ask, limit_price, trigger) {
                Some(tick.ask)
            } else {
                None
            }
        }
        OrderSide::Sell => {
            if touches_up(tick.bid, limit_price, trigger) {
                Some(tick.bid)
            } else {
                None
            }
        }
    }
}

fn match_tick_stop(
    side: OrderSide,
    stop_price: Decimal,
    tick: &Tick,
    trigger: PriceTrigger,
) -> Option<Decimal> {
    match side {
        OrderSide::Buy => {
            if touches_up(tick.ask, stop_price, trigger) {
                Some(tick.ask)
            } else {
                None
            }
        }
        OrderSide::Sell => {
            if touches_down(tick.bid, stop_price, trigger) {
                Some(tick.bid)
            } else {
                None
            }
        }
    }
}

fn stop_triggers_on_tick(
    side: OrderSide,
    stop_price: Decimal,
    tick: &Tick,
    trigger: PriceTrigger,
) -> bool {
    match side {
        OrderSide::Buy => touches_up(tick.ask, stop_price, trigger),
        OrderSide::Sell => touches_down(tick.bid, stop_price, trigger),
    }
}

fn touches_up(observed: Decimal, threshold: Decimal, trigger: PriceTrigger) -> bool {
    match trigger {
        PriceTrigger::Touch => observed >= threshold,
        PriceTrigger::TradeThrough => observed > threshold,
    }
}

fn touches_down(observed: Decimal, threshold: Decimal, trigger: PriceTrigger) -> bool {
    match trigger {
        PriceTrigger::Touch => observed <= threshold,
        PriceTrigger::TradeThrough => observed < threshold,
    }
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};
    use rust_decimal::Decimal;

    use super::{
        BarMarketFillPrice, BarMatcher, BarMatcherConfig, PriceTrigger, Tick, TickMatcher,
    };
    use crate::Bar;
    use crate::order_sys::holding::LotReliefMethod;
    use crate::order_sys::order::{OrderSide, OrderStatus, OrderType, PositionEffect, TimeInForce};
    use crate::order_sys::system::{OrderSystem, SubmitOrderRequest};

    fn dec(value: i64, scale: u32) -> Decimal {
        Decimal::new(value, scale)
    }

    fn ts(day: u32, minute: u32) -> chrono::DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 5, day, 9, minute, 0).unwrap()
    }

    fn bar(day: u32, open: f32, high: f32, low: f32, close: f32) -> Bar {
        Bar {
            time: ts(day, 30),
            open,
            high,
            low,
            close,
            volume: 1.0,
        }
    }

    #[test]
    fn bar_matcher_fills_market_orders_on_next_bar_open_by_default() {
        let mut system = OrderSystem::new(LotReliefMethod::Fifo);
        let mut matcher = BarMatcher::new(BarMatcherConfig {
            market_fill_price: BarMarketFillPrice::Open,
            ..BarMatcherConfig::default()
        });

        let order = system
            .submit_order(SubmitOrderRequest::new(
                "AAPL",
                OrderSide::Buy,
                PositionEffect::Open,
                OrderType::Market,
                TimeInForce::Day,
                ts(24, 30),
                dec(2, 0),
                None,
                None,
            ))
            .unwrap();

        let same_bar_result = matcher
            .process_bar(&bar(24, 100.0, 101.0, 99.0, 100.5), &mut system)
            .unwrap();
        assert!(same_bar_result.acknowledged_reports.is_empty());
        assert!(same_bar_result.applied_fills.is_empty());
        assert_eq!(
            system.order(order.order.id()).unwrap().status(),
            OrderStatus::Pending
        );

        let next_bar_result = matcher
            .process_bar(&bar(25, 102.0, 103.0, 101.0, 102.5), &mut system)
            .unwrap();
        assert_eq!(next_bar_result.acknowledged_reports.len(), 1);
        assert_eq!(next_bar_result.applied_fills.len(), 1);
        assert_eq!(
            next_bar_result.applied_fills[0].order.average_fill_price(),
            Some(dec(102, 0))
        );
    }

    #[test]
    fn bar_matcher_acknowledges_resting_limits_before_they_fill() {
        let mut system = OrderSystem::new(LotReliefMethod::Fifo);
        let mut matcher = BarMatcher::default();

        let order = system
            .submit_order(SubmitOrderRequest::new(
                "AAPL",
                OrderSide::Buy,
                PositionEffect::Open,
                OrderType::Limit,
                TimeInForce::Day,
                ts(24, 30),
                dec(1, 0),
                Some(dec(100, 0)),
                None,
            ))
            .unwrap();

        let rest_result = matcher
            .process_bar(&bar(25, 105.0, 106.0, 101.0, 103.0), &mut system)
            .unwrap();
        assert_eq!(rest_result.acknowledged_reports.len(), 1);
        assert!(rest_result.applied_fills.is_empty());
        assert_eq!(
            system.order(order.order.id()).unwrap().status(),
            OrderStatus::Working
        );

        let fill_result = matcher
            .process_bar(&bar(26, 102.0, 104.0, 99.0, 100.0), &mut system)
            .unwrap();
        assert_eq!(fill_result.applied_fills.len(), 1);
        assert_eq!(
            fill_result.applied_fills[0].order.average_fill_price(),
            Some(dec(100, 0))
        );
    }

    #[test]
    fn bar_matcher_trade_through_requires_more_than_a_touch() {
        let mut system = OrderSystem::new(LotReliefMethod::Fifo);
        let mut matcher = BarMatcher::new(BarMatcherConfig {
            price_trigger: PriceTrigger::TradeThrough,
            ..BarMatcherConfig::default()
        });

        let order = system
            .submit_order(SubmitOrderRequest::new(
                "AAPL",
                OrderSide::Buy,
                PositionEffect::Open,
                OrderType::Limit,
                TimeInForce::Day,
                ts(24, 30),
                dec(1, 0),
                Some(dec(100, 0)),
                None,
            ))
            .unwrap();

        let touch_only = matcher
            .process_bar(&bar(25, 105.0, 106.0, 100.0, 104.0), &mut system)
            .unwrap();
        assert_eq!(touch_only.acknowledged_reports.len(), 1);
        assert!(touch_only.applied_fills.is_empty());
        assert_eq!(
            system.order(order.order.id()).unwrap().status(),
            OrderStatus::Working
        );

        let trade_through = matcher
            .process_bar(&bar(26, 105.0, 106.0, 99.0, 101.0), &mut system)
            .unwrap();
        assert_eq!(trade_through.applied_fills.len(), 1);
        assert_eq!(
            trade_through.applied_fills[0].order.average_fill_price(),
            Some(dec(100, 0))
        );
    }

    #[test]
    fn bar_matcher_conservatively_defers_intrabar_stop_limit_fills() {
        let mut system = OrderSystem::new(LotReliefMethod::Fifo);
        let mut matcher = BarMatcher::default();

        let order = system
            .submit_order(SubmitOrderRequest::new(
                "AAPL",
                OrderSide::Buy,
                PositionEffect::Open,
                OrderType::StopLimit,
                TimeInForce::Day,
                ts(24, 30),
                dec(1, 0),
                Some(dec(104, 0)),
                Some(dec(105, 0)),
            ))
            .unwrap();

        let trigger_only = matcher
            .process_bar(&bar(25, 100.0, 106.0, 103.0, 104.0), &mut system)
            .unwrap();
        assert_eq!(trigger_only.acknowledged_reports.len(), 1);
        assert!(trigger_only.applied_fills.is_empty());
        assert_eq!(
            system.order(order.order.id()).unwrap().status(),
            OrderStatus::Working
        );

        let fill_next_bar = matcher
            .process_bar(&bar(26, 104.0, 105.0, 102.0, 103.0), &mut system)
            .unwrap();
        assert_eq!(fill_next_bar.applied_fills.len(), 1);
        assert_eq!(
            fill_next_bar.applied_fills[0].order.average_fill_price(),
            Some(dec(104, 0))
        );
    }

    #[test]
    fn tick_matcher_can_trigger_and_fill_stop_limit_on_same_tick() {
        let mut system = OrderSystem::new(LotReliefMethod::Fifo);
        let mut matcher = TickMatcher::default();

        let order = system
            .submit_order(SubmitOrderRequest::new(
                "AAPL",
                OrderSide::Buy,
                PositionEffect::Open,
                OrderType::StopLimit,
                TimeInForce::Day,
                ts(24, 30),
                dec(1, 0),
                Some(dec(106, 0)),
                Some(dec(105, 0)),
            ))
            .unwrap();

        let same_tick = Tick::new(ts(24, 30), dec(104, 0), dec(105, 0), None, None).unwrap();
        let same_tick_result = matcher.process_tick(&same_tick, &mut system).unwrap();
        assert!(same_tick_result.applied_fills.is_empty());
        assert_eq!(
            system.order(order.order.id()).unwrap().status(),
            OrderStatus::Pending
        );

        let trigger_tick = Tick::new(ts(24, 31), dec(105, 0), dec(1055, 1), None, None).unwrap();
        let trigger_result = matcher.process_tick(&trigger_tick, &mut system).unwrap();
        assert_eq!(trigger_result.acknowledged_reports.len(), 1);
        assert_eq!(trigger_result.applied_fills.len(), 1);
        assert_eq!(
            trigger_result.applied_fills[0].order.average_fill_price(),
            Some(dec(1055, 1))
        );
    }
}
