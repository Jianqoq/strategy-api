//! Order intent and order-state transitions.
//!
//! The [`Order`] aggregate models what the strategy or client asked the system
//! to do. It tracks requested quantity, order type, time in force, fills
//! applied so far, and terminal states such as cancel and reject.

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use thiserror::Error;

use crate::order_sys::fill::Fill;
use crate::order_sys::lot::LotSide;
use crate::order_sys::{FillId, OrderId};

/// Direction of the order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OrderSide {
    /// Buy side order.
    Buy,
    /// Sell side order.
    Sell,
}

impl OrderSide {
    /// Maps an opening order side to the resulting lot side.
    pub fn to_open_lot_side(self) -> LotSide {
        match self {
            Self::Buy => LotSide::Long,
            Self::Sell => LotSide::Short,
        }
    }

    /// Returns which lot side this order would close.
    pub fn closing_lot_side(self) -> LotSide {
        match self {
            Self::Buy => LotSide::Short,
            Self::Sell => LotSide::Long,
        }
    }
}

/// Whether an order opens or closes exposure.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PositionEffect {
    /// The order adds new position exposure.
    Open,
    /// The order reduces or removes existing position exposure.
    Close,
}

/// Execution style requested by the order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OrderType {
    /// Immediate execution at the best available price.
    Market,
    /// Execution limited by a limit price.
    Limit,
    /// Triggered by a stop price and then behaves like a market order.
    Stop,
    /// Triggered by a stop price and then behaves like a limit order.
    StopLimit,
}

/// Time-in-force policy requested by the order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimeInForce {
    /// Valid for the current trading day or session.
    Day,
    /// Good until canceled.
    Gtc,
    /// Immediate-or-cancel.
    Ioc,
    /// Fill-or-kill.
    Fok,
}

/// Current lifecycle state of the order aggregate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OrderStatus {
    /// The order has been created locally but not yet acknowledged.
    Pending,
    /// The order is live and working in the market.
    Working,
    /// The order has received at least one fill and still has leaves quantity.
    PartiallyFilled,
    /// The order has been fully filled.
    Filled,
    /// The order was canceled before completion.
    Canceled,
    /// The order was rejected.
    Rejected,
}

/// Domain errors raised while validating or mutating an order.
#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum OrderError {
    /// The symbol string was empty or whitespace.
    #[error("order symbol cannot be empty")]
    EmptySymbol,
    /// Requested quantity must be strictly positive.
    #[error("order quantity must be positive, got {quantity}")]
    NonPositiveQuantity {
        /// Invalid quantity supplied by the caller.
        quantity: Decimal,
    },
    /// Prices must be strictly positive when present.
    #[error("order price must be positive, got {price}")]
    NonPositivePrice {
        /// Invalid price supplied by the caller.
        price: Decimal,
    },
    /// A limit order requires a limit price.
    #[error("limit order requires a limit price")]
    MissingLimitPrice,
    /// A stop order requires a stop price.
    #[error("stop order requires a stop price")]
    MissingStopPrice,
    /// A limit price was supplied to an order type that does not use one.
    #[error("limit price is not valid for this order type")]
    UnexpectedLimitPrice,
    /// A stop price was supplied to an order type that does not use one.
    #[error("stop price is not valid for this order type")]
    UnexpectedStopPrice,
    /// Order events must be applied in non-decreasing timestamp order.
    #[error(
        "order event timestamp {new_timestamp} is earlier than the previous order event {previous_timestamp}"
    )]
    EventOutOfOrder {
        /// Timestamp already stored on the order.
        previous_timestamp: DateTime<Utc>,
        /// Newly supplied timestamp.
        new_timestamp: DateTime<Utc>,
    },
    /// The fill referenced a different order.
    #[error("fill order mismatch: expected {}, got {}", .expected.value(), .actual.value())]
    FillOrderMismatch {
        /// Order expected by the aggregate.
        expected: OrderId,
        /// Order referenced by the fill.
        actual: OrderId,
    },
    /// The fill symbol did not match the order symbol.
    #[error("fill symbol mismatch: expected {expected}, got {actual}")]
    FillSymbolMismatch {
        /// Symbol stored on the order.
        expected: String,
        /// Symbol stored on the fill.
        actual: String,
    },
    /// The fill side did not match the order side.
    #[error("fill side mismatch: expected {expected:?}, got {actual:?}")]
    FillSideMismatch {
        /// Side stored on the order.
        expected: OrderSide,
        /// Side stored on the fill.
        actual: OrderSide,
    },
    /// The fill position effect did not match the order position effect.
    #[error("fill position effect mismatch: expected {expected:?}, got {actual:?}")]
    FillPositionEffectMismatch {
        /// Position effect stored on the order.
        expected: PositionEffect,
        /// Position effect stored on the fill.
        actual: PositionEffect,
    },
    /// The fill quantity exceeded the order's remaining leaves quantity.
    #[error("fill quantity {fill_qty} exceeds order leaves quantity {leaves_qty}")]
    FillQuantityExceedsLeaves {
        /// Quantity carried by the fill.
        fill_qty: Decimal,
        /// Remaining quantity on the order.
        leaves_qty: Decimal,
    },
    /// The same fill identifier was applied more than once.
    #[error("fill {} was already applied to this order", .fill_id.value())]
    DuplicateFillId {
        /// Duplicate fill identifier.
        fill_id: FillId,
    },
    /// The order is already terminal and cannot accept more lifecycle events.
    #[error("order {} is in terminal status {status:?}", .order_id.value())]
    TerminalStatus {
        /// Terminal order identifier.
        order_id: OrderId,
        /// Current terminal status.
        status: OrderStatus,
    },
    /// A rejection after any fills would destroy audit consistency.
    #[error("order {} cannot be rejected after fills; filled quantity is {filled_qty}", .order_id.value())]
    RejectAfterFill {
        /// Order being rejected.
        order_id: OrderId,
        /// Already filled quantity on the order.
        filled_qty: Decimal,
    },
}

/// Order aggregate and state machine.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Order {
    /// Stable order identifier.
    id: OrderId,
    /// Instrument symbol requested by the order.
    symbol: String,
    /// Buy or sell side.
    side: OrderSide,
    /// Whether the order opens or closes exposure.
    position_effect: PositionEffect,
    /// Execution style requested by the caller.
    order_type: OrderType,
    /// Time-in-force policy.
    time_in_force: TimeInForce,
    /// UTC timestamp when the order was submitted to the system.
    submitted_at: DateTime<Utc>,
    /// Timestamp of the most recent lifecycle event.
    last_event_at: DateTime<Utc>,
    /// Original requested quantity.
    requested_qty: Decimal,
    /// Optional limit price depending on the order type.
    limit_price: Option<Decimal>,
    /// Optional stop price depending on the order type.
    stop_price: Option<Decimal>,
    /// Current order status.
    status: OrderStatus,
    /// Cumulative filled quantity.
    filled_qty: Decimal,
    /// Weighted average fill price, if any fills have occurred.
    average_fill_price: Option<Decimal>,
    /// Cumulative fees charged by fills applied to the order.
    cumulative_fees: Decimal,
    /// Fill identifiers already applied to the order.
    fill_ids: Vec<FillId>,
    /// Cancellation timestamp when the order was canceled.
    canceled_at: Option<DateTime<Utc>>,
    /// Rejection timestamp when the order was rejected.
    rejected_at: Option<DateTime<Utc>>,
    /// Free-form reject reason.
    reject_reason: Option<String>,
}

impl Order {
    /// Creates a new order aggregate in [`OrderStatus::Pending`].
    ///
    /// The constructor validates quantity and price requirements implied by the
    /// [`OrderType`].
    pub fn new(
        id: OrderId,
        symbol: impl Into<String>,
        side: OrderSide,
        position_effect: PositionEffect,
        order_type: OrderType,
        time_in_force: TimeInForce,
        submitted_at: DateTime<Utc>,
        requested_qty: Decimal,
        limit_price: Option<Decimal>,
        stop_price: Option<Decimal>,
    ) -> Result<Self, OrderError> {
        let symbol = symbol.into();
        if symbol.trim().is_empty() {
            return Err(OrderError::EmptySymbol);
        }
        validate_positive_quantity(requested_qty)?;
        validate_price_requirements(order_type, limit_price, stop_price)?;

        Ok(Self {
            id,
            symbol,
            side,
            position_effect,
            order_type,
            time_in_force,
            submitted_at,
            last_event_at: submitted_at,
            requested_qty,
            limit_price,
            stop_price,
            status: OrderStatus::Pending,
            filled_qty: Decimal::ZERO,
            average_fill_price: None,
            cumulative_fees: Decimal::ZERO,
            fill_ids: Vec::new(),
            canceled_at: None,
            rejected_at: None,
            reject_reason: None,
        })
    }

    /// Returns the order identifier.
    pub fn id(&self) -> OrderId {
        self.id
    }

    /// Returns the order symbol.
    pub fn symbol(&self) -> &str {
        &self.symbol
    }

    /// Returns the order side.
    pub fn side(&self) -> OrderSide {
        self.side
    }

    /// Returns whether the order opens or closes exposure.
    pub fn position_effect(&self) -> PositionEffect {
        self.position_effect
    }

    /// Returns the order type.
    pub fn order_type(&self) -> OrderType {
        self.order_type
    }

    /// Returns the time-in-force policy.
    pub fn time_in_force(&self) -> TimeInForce {
        self.time_in_force
    }

    /// Returns the submission timestamp.
    pub fn submitted_at(&self) -> DateTime<Utc> {
        self.submitted_at
    }

    /// Returns the timestamp of the most recent lifecycle event.
    pub fn last_event_at(&self) -> DateTime<Utc> {
        self.last_event_at
    }

    /// Returns the originally requested quantity.
    pub fn requested_qty(&self) -> Decimal {
        self.requested_qty
    }

    /// Returns the limit price if the order type uses one.
    pub fn limit_price(&self) -> Option<Decimal> {
        self.limit_price
    }

    /// Returns the stop price if the order type uses one.
    pub fn stop_price(&self) -> Option<Decimal> {
        self.stop_price
    }

    /// Returns the current status.
    pub fn status(&self) -> OrderStatus {
        self.status
    }

    /// Returns cumulative filled quantity.
    pub fn filled_qty(&self) -> Decimal {
        self.filled_qty
    }

    /// Returns remaining quantity still open on the order.
    pub fn leaves_qty(&self) -> Decimal {
        self.requested_qty - self.filled_qty
    }

    /// Returns weighted average fill price if at least one fill exists.
    pub fn average_fill_price(&self) -> Option<Decimal> {
        self.average_fill_price
    }

    /// Returns cumulative fees charged by all fills.
    pub fn cumulative_fees(&self) -> Decimal {
        self.cumulative_fees
    }

    /// Returns identifiers of fills already applied to the order.
    pub fn fill_ids(&self) -> &[FillId] {
        &self.fill_ids
    }

    /// Returns cancellation timestamp when the order was canceled.
    pub fn canceled_at(&self) -> Option<DateTime<Utc>> {
        self.canceled_at
    }

    /// Returns rejection timestamp when the order was rejected.
    pub fn rejected_at(&self) -> Option<DateTime<Utc>> {
        self.rejected_at
    }

    /// Returns reject reason text when the order was rejected.
    pub fn reject_reason(&self) -> Option<&str> {
        self.reject_reason.as_deref()
    }

    /// Marks the order as acknowledged and working.
    ///
    /// This transition is only legal while the order is non-terminal and the
    /// event timestamp is not earlier than the previous event.
    pub fn acknowledge(&mut self, acknowledged_at: DateTime<Utc>) -> Result<(), OrderError> {
        self.ensure_non_terminal()?;
        ensure_event_order(self.last_event_at, acknowledged_at)?;

        if self.filled_qty.is_zero() {
            self.status = OrderStatus::Working;
        }
        self.last_event_at = acknowledged_at;
        Ok(())
    }

    /// Applies one immutable fill to the order aggregate.
    ///
    /// The method validates fill identity, symbol, side, position effect,
    /// duplicate replay, chronological ordering, and leaves quantity before
    /// mutating the aggregate.
    pub fn record_fill(&mut self, fill: &Fill) -> Result<(), OrderError> {
        self.ensure_non_terminal()?;
        if fill.order_id() != self.id {
            return Err(OrderError::FillOrderMismatch {
                expected: self.id,
                actual: fill.order_id(),
            });
        }
        if fill.symbol() != self.symbol {
            return Err(OrderError::FillSymbolMismatch {
                expected: self.symbol.clone(),
                actual: fill.symbol().to_owned(),
            });
        }
        if fill.order_side() != self.side {
            return Err(OrderError::FillSideMismatch {
                expected: self.side,
                actual: fill.order_side(),
            });
        }
        if fill.position_effect() != self.position_effect {
            return Err(OrderError::FillPositionEffectMismatch {
                expected: self.position_effect,
                actual: fill.position_effect(),
            });
        }
        if self.fill_ids.contains(&fill.id()) {
            return Err(OrderError::DuplicateFillId { fill_id: fill.id() });
        }
        ensure_event_order(self.last_event_at, fill.executed_at())?;
        if fill.qty() > self.leaves_qty() {
            return Err(OrderError::FillQuantityExceedsLeaves {
                fill_qty: fill.qty(),
                leaves_qty: self.leaves_qty(),
            });
        }

        let previous_qty = self.filled_qty;
        let new_filled_qty = previous_qty + fill.qty();
        let new_notional = self.average_fill_price.unwrap_or(Decimal::ZERO) * previous_qty
            + fill.price() * fill.qty();

        self.filled_qty = new_filled_qty;
        self.average_fill_price = Some(new_notional / new_filled_qty);
        self.cumulative_fees += fill.fees();
        self.fill_ids.push(fill.id());
        self.last_event_at = fill.executed_at();
        self.status = if self.leaves_qty().is_zero() {
            OrderStatus::Filled
        } else {
            OrderStatus::PartiallyFilled
        };

        Ok(())
    }

    /// Cancels the order.
    ///
    /// Cancellation is only legal while the order is non-terminal.
    pub fn cancel(&mut self, canceled_at: DateTime<Utc>) -> Result<(), OrderError> {
        self.ensure_non_terminal()?;
        ensure_event_order(self.last_event_at, canceled_at)?;

        self.status = OrderStatus::Canceled;
        self.canceled_at = Some(canceled_at);
        self.last_event_at = canceled_at;
        Ok(())
    }

    /// Rejects the order with a free-form reason.
    ///
    /// Rejections are blocked after any fills to avoid creating contradictory
    /// audit history.
    pub fn reject(
        &mut self,
        rejected_at: DateTime<Utc>,
        reason: impl Into<String>,
    ) -> Result<(), OrderError> {
        self.ensure_non_terminal()?;
        if !self.filled_qty.is_zero() {
            return Err(OrderError::RejectAfterFill {
                order_id: self.id,
                filled_qty: self.filled_qty,
            });
        }
        ensure_event_order(self.last_event_at, rejected_at)?;

        self.status = OrderStatus::Rejected;
        self.rejected_at = Some(rejected_at);
        self.reject_reason = Some(reason.into());
        self.last_event_at = rejected_at;
        Ok(())
    }

    /// Internal helper that rejects transitions out of terminal states.
    fn ensure_non_terminal(&self) -> Result<(), OrderError> {
        match self.status {
            OrderStatus::Filled | OrderStatus::Canceled | OrderStatus::Rejected => {
                Err(OrderError::TerminalStatus {
                    order_id: self.id,
                    status: self.status,
                })
            }
            OrderStatus::Pending | OrderStatus::Working | OrderStatus::PartiallyFilled => Ok(()),
        }
    }
}

/// Validates that quantity is strictly positive.
fn validate_positive_quantity(quantity: Decimal) -> Result<(), OrderError> {
    if quantity <= Decimal::ZERO {
        return Err(OrderError::NonPositiveQuantity { quantity });
    }

    Ok(())
}

/// Validates that price is strictly positive.
fn validate_positive_price(price: Decimal) -> Result<(), OrderError> {
    if price <= Decimal::ZERO {
        return Err(OrderError::NonPositivePrice { price });
    }

    Ok(())
}

/// Validates limit and stop price requirements implied by the order type.
fn validate_price_requirements(
    order_type: OrderType,
    limit_price: Option<Decimal>,
    stop_price: Option<Decimal>,
) -> Result<(), OrderError> {
    if let Some(price) = limit_price {
        validate_positive_price(price)?;
    }
    if let Some(price) = stop_price {
        validate_positive_price(price)?;
    }

    match order_type {
        OrderType::Market => {
            if limit_price.is_some() {
                return Err(OrderError::UnexpectedLimitPrice);
            }
            if stop_price.is_some() {
                return Err(OrderError::UnexpectedStopPrice);
            }
        }
        OrderType::Limit => {
            if limit_price.is_none() {
                return Err(OrderError::MissingLimitPrice);
            }
            if stop_price.is_some() {
                return Err(OrderError::UnexpectedStopPrice);
            }
        }
        OrderType::Stop => {
            if stop_price.is_none() {
                return Err(OrderError::MissingStopPrice);
            }
            if limit_price.is_some() {
                return Err(OrderError::UnexpectedLimitPrice);
            }
        }
        OrderType::StopLimit => {
            if limit_price.is_none() {
                return Err(OrderError::MissingLimitPrice);
            }
            if stop_price.is_none() {
                return Err(OrderError::MissingStopPrice);
            }
        }
    }

    Ok(())
}

/// Validates chronological ordering for order lifecycle events.
fn ensure_event_order(
    previous_timestamp: DateTime<Utc>,
    new_timestamp: DateTime<Utc>,
) -> Result<(), OrderError> {
    if new_timestamp < previous_timestamp {
        return Err(OrderError::EventOutOfOrder {
            previous_timestamp,
            new_timestamp,
        });
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};
    use rust_decimal::Decimal;

    use super::{
        Order, OrderError, OrderSide, OrderStatus, OrderType, PositionEffect, TimeInForce,
    };
    use crate::order_sys::fill::Fill;
    use crate::order_sys::{FillId, OrderId};

    fn dec(value: i64, scale: u32) -> Decimal {
        Decimal::new(value, scale)
    }

    fn ts(day: u32) -> chrono::DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 5, day, 9, 30, 0).unwrap()
    }

    #[test]
    fn order_tracks_partial_and_full_fill_lifecycle() {
        let mut order = Order::new(
            OrderId::new(1),
            "AAPL",
            OrderSide::Buy,
            PositionEffect::Open,
            OrderType::Limit,
            TimeInForce::Day,
            ts(24),
            dec(10, 0),
            Some(dec(100, 0)),
            None,
        )
        .unwrap();
        order.acknowledge(ts(24)).unwrap();

        let fill_one = Fill::new(
            FillId::new(10),
            OrderId::new(1),
            "AAPL",
            OrderSide::Buy,
            PositionEffect::Open,
            ts(25),
            dec(4, 0),
            dec(99, 0),
            dec(4, 1),
        )
        .unwrap();
        let fill_two = Fill::new(
            FillId::new(11),
            OrderId::new(1),
            "AAPL",
            OrderSide::Buy,
            PositionEffect::Open,
            ts(26),
            dec(6, 0),
            dec(101, 0),
            dec(6, 1),
        )
        .unwrap();

        order.record_fill(&fill_one).unwrap();
        assert_eq!(order.status(), OrderStatus::PartiallyFilled);
        assert_eq!(order.filled_qty(), dec(4, 0));
        assert_eq!(order.leaves_qty(), dec(6, 0));
        assert_eq!(order.average_fill_price(), Some(dec(99, 0)));
        assert_eq!(order.cumulative_fees(), dec(4, 1));

        order.record_fill(&fill_two).unwrap();
        assert_eq!(order.status(), OrderStatus::Filled);
        assert_eq!(order.filled_qty(), dec(10, 0));
        assert_eq!(order.leaves_qty(), Decimal::ZERO);
        assert_eq!(order.average_fill_price(), Some(dec(1002, 1)));
        assert_eq!(order.cumulative_fees(), dec(1, 0));
        assert_eq!(order.fill_ids(), &[FillId::new(10), FillId::new(11)]);
    }

    #[test]
    fn order_rejects_incompatible_fill() {
        let mut order = Order::new(
            OrderId::new(2),
            "AAPL",
            OrderSide::Buy,
            PositionEffect::Open,
            OrderType::Market,
            TimeInForce::Day,
            ts(24),
            dec(5, 0),
            None,
            None,
        )
        .unwrap();

        let wrong_fill = Fill::new(
            FillId::new(12),
            OrderId::new(999),
            "AAPL",
            OrderSide::Buy,
            PositionEffect::Open,
            ts(25),
            dec(1, 0),
            dec(100, 0),
            dec(0, 0),
        )
        .unwrap();

        let err = order.record_fill(&wrong_fill).unwrap_err();
        assert_eq!(
            err,
            OrderError::FillOrderMismatch {
                expected: OrderId::new(2),
                actual: OrderId::new(999),
            }
        );
    }

    #[test]
    fn order_reject_after_fill_is_blocked() {
        let mut order = Order::new(
            OrderId::new(3),
            "AAPL",
            OrderSide::Buy,
            PositionEffect::Open,
            OrderType::Market,
            TimeInForce::Day,
            ts(24),
            dec(2, 0),
            None,
            None,
        )
        .unwrap();

        let fill = Fill::new(
            FillId::new(13),
            OrderId::new(3),
            "AAPL",
            OrderSide::Buy,
            PositionEffect::Open,
            ts(25),
            dec(1, 0),
            dec(100, 0),
            dec(0, 0),
        )
        .unwrap();

        order.record_fill(&fill).unwrap();
        let err = order.reject(ts(26), "late reject").unwrap_err();
        assert_eq!(
            err,
            OrderError::RejectAfterFill {
                order_id: OrderId::new(3),
                filled_qty: dec(1, 0),
            }
        );
    }

    #[test]
    fn order_rejects_duplicate_fill_replay() {
        let mut order = Order::new(
            OrderId::new(4),
            "AAPL",
            OrderSide::Buy,
            PositionEffect::Open,
            OrderType::Market,
            TimeInForce::Day,
            ts(24),
            dec(3, 0),
            None,
            None,
        )
        .unwrap();

        let fill = Fill::new(
            FillId::new(14),
            OrderId::new(4),
            "AAPL",
            OrderSide::Buy,
            PositionEffect::Open,
            ts(25),
            dec(1, 0),
            dec(100, 0),
            dec(0, 0),
        )
        .unwrap();

        order.record_fill(&fill).unwrap();
        let err = order.record_fill(&fill).unwrap_err();
        assert_eq!(
            err,
            OrderError::DuplicateFillId {
                fill_id: FillId::new(14),
            }
        );
    }
}
