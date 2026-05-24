use chrono::{DateTime, Utc};
use rust_decimal::Decimal;

use crate::order_sys::fill::Fill;
use crate::order_sys::lot::LotSide;
use crate::order_sys::{FillId, OrderId};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OrderSide {
    Buy,
    Sell,
}

impl OrderSide {
    pub fn to_open_lot_side(self) -> LotSide {
        match self {
            Self::Buy => LotSide::Long,
            Self::Sell => LotSide::Short,
        }
    }

    pub fn closing_lot_side(self) -> LotSide {
        match self {
            Self::Buy => LotSide::Short,
            Self::Sell => LotSide::Long,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PositionEffect {
    Open,
    Close,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OrderType {
    Market,
    Limit,
    Stop,
    StopLimit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimeInForce {
    Day,
    Gtc,
    Ioc,
    Fok,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OrderStatus {
    Pending,
    Working,
    PartiallyFilled,
    Filled,
    Canceled,
    Rejected,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OrderError {
    EmptySymbol,
    NonPositiveQuantity {
        quantity: Decimal,
    },
    NonPositivePrice {
        price: Decimal,
    },
    MissingLimitPrice,
    MissingStopPrice,
    UnexpectedLimitPrice,
    UnexpectedStopPrice,
    EventOutOfOrder {
        previous_timestamp: DateTime<Utc>,
        new_timestamp: DateTime<Utc>,
    },
    FillOrderMismatch {
        expected: OrderId,
        actual: OrderId,
    },
    FillSymbolMismatch {
        expected: String,
        actual: String,
    },
    FillSideMismatch {
        expected: OrderSide,
        actual: OrderSide,
    },
    FillPositionEffectMismatch {
        expected: PositionEffect,
        actual: PositionEffect,
    },
    FillQuantityExceedsLeaves {
        fill_qty: Decimal,
        leaves_qty: Decimal,
    },
    DuplicateFillId {
        fill_id: FillId,
    },
    TerminalStatus {
        order_id: OrderId,
        status: OrderStatus,
    },
    RejectAfterFill {
        order_id: OrderId,
        filled_qty: Decimal,
    },
}

impl std::fmt::Display for OrderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptySymbol => write!(f, "order symbol cannot be empty"),
            Self::NonPositiveQuantity { quantity } => {
                write!(f, "order quantity must be positive, got {quantity}")
            }
            Self::NonPositivePrice { price } => {
                write!(f, "order price must be positive, got {price}")
            }
            Self::MissingLimitPrice => write!(f, "limit order requires a limit price"),
            Self::MissingStopPrice => write!(f, "stop order requires a stop price"),
            Self::UnexpectedLimitPrice => write!(f, "limit price is not valid for this order type"),
            Self::UnexpectedStopPrice => write!(f, "stop price is not valid for this order type"),
            Self::EventOutOfOrder {
                previous_timestamp,
                new_timestamp,
            } => write!(
                f,
                "order event timestamp {new_timestamp} is earlier than the previous order event {previous_timestamp}"
            ),
            Self::FillOrderMismatch { expected, actual } => write!(
                f,
                "fill order mismatch: expected {}, got {}",
                expected.value(),
                actual.value()
            ),
            Self::FillSymbolMismatch { expected, actual } => {
                write!(f, "fill symbol mismatch: expected {expected}, got {actual}")
            }
            Self::FillSideMismatch { expected, actual } => {
                write!(
                    f,
                    "fill side mismatch: expected {:?}, got {:?}",
                    expected, actual
                )
            }
            Self::FillPositionEffectMismatch { expected, actual } => write!(
                f,
                "fill position effect mismatch: expected {:?}, got {:?}",
                expected, actual
            ),
            Self::FillQuantityExceedsLeaves {
                fill_qty,
                leaves_qty,
            } => write!(
                f,
                "fill quantity {fill_qty} exceeds order leaves quantity {leaves_qty}"
            ),
            Self::DuplicateFillId { fill_id } => {
                write!(
                    f,
                    "fill {} was already applied to this order",
                    fill_id.value()
                )
            }
            Self::TerminalStatus { order_id, status } => write!(
                f,
                "order {} is in terminal status {:?}",
                order_id.value(),
                status
            ),
            Self::RejectAfterFill {
                order_id,
                filled_qty,
            } => write!(
                f,
                "order {} cannot be rejected after fills; filled quantity is {filled_qty}",
                order_id.value()
            ),
        }
    }
}

impl std::error::Error for OrderError {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Order {
    id: OrderId,
    symbol: String,
    side: OrderSide,
    position_effect: PositionEffect,
    order_type: OrderType,
    time_in_force: TimeInForce,
    submitted_at: DateTime<Utc>,
    last_event_at: DateTime<Utc>,
    requested_qty: Decimal,
    limit_price: Option<Decimal>,
    stop_price: Option<Decimal>,
    status: OrderStatus,
    filled_qty: Decimal,
    average_fill_price: Option<Decimal>,
    cumulative_fees: Decimal,
    fill_ids: Vec<FillId>,
    canceled_at: Option<DateTime<Utc>>,
    rejected_at: Option<DateTime<Utc>>,
    reject_reason: Option<String>,
}

impl Order {
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

    pub fn id(&self) -> OrderId {
        self.id
    }

    pub fn symbol(&self) -> &str {
        &self.symbol
    }

    pub fn side(&self) -> OrderSide {
        self.side
    }

    pub fn position_effect(&self) -> PositionEffect {
        self.position_effect
    }

    pub fn order_type(&self) -> OrderType {
        self.order_type
    }

    pub fn time_in_force(&self) -> TimeInForce {
        self.time_in_force
    }

    pub fn submitted_at(&self) -> DateTime<Utc> {
        self.submitted_at
    }

    pub fn last_event_at(&self) -> DateTime<Utc> {
        self.last_event_at
    }

    pub fn requested_qty(&self) -> Decimal {
        self.requested_qty
    }

    pub fn limit_price(&self) -> Option<Decimal> {
        self.limit_price
    }

    pub fn stop_price(&self) -> Option<Decimal> {
        self.stop_price
    }

    pub fn status(&self) -> OrderStatus {
        self.status
    }

    pub fn filled_qty(&self) -> Decimal {
        self.filled_qty
    }

    pub fn leaves_qty(&self) -> Decimal {
        self.requested_qty - self.filled_qty
    }

    pub fn average_fill_price(&self) -> Option<Decimal> {
        self.average_fill_price
    }

    pub fn cumulative_fees(&self) -> Decimal {
        self.cumulative_fees
    }

    pub fn fill_ids(&self) -> &[FillId] {
        &self.fill_ids
    }

    pub fn canceled_at(&self) -> Option<DateTime<Utc>> {
        self.canceled_at
    }

    pub fn rejected_at(&self) -> Option<DateTime<Utc>> {
        self.rejected_at
    }

    pub fn reject_reason(&self) -> Option<&str> {
        self.reject_reason.as_deref()
    }

    pub fn acknowledge(&mut self, acknowledged_at: DateTime<Utc>) -> Result<(), OrderError> {
        self.ensure_non_terminal()?;
        ensure_event_order(self.last_event_at, acknowledged_at)?;

        if self.filled_qty.is_zero() {
            self.status = OrderStatus::Working;
        }
        self.last_event_at = acknowledged_at;
        Ok(())
    }

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

    pub fn cancel(&mut self, canceled_at: DateTime<Utc>) -> Result<(), OrderError> {
        self.ensure_non_terminal()?;
        ensure_event_order(self.last_event_at, canceled_at)?;

        self.status = OrderStatus::Canceled;
        self.canceled_at = Some(canceled_at);
        self.last_event_at = canceled_at;
        Ok(())
    }

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

fn validate_positive_quantity(quantity: Decimal) -> Result<(), OrderError> {
    if quantity <= Decimal::ZERO {
        return Err(OrderError::NonPositiveQuantity { quantity });
    }

    Ok(())
}

fn validate_positive_price(price: Decimal) -> Result<(), OrderError> {
    if price <= Decimal::ZERO {
        return Err(OrderError::NonPositivePrice { price });
    }

    Ok(())
}

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
