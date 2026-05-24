use chrono::{DateTime, Utc};
use rust_decimal::Decimal;

use crate::order_sys::order::{OrderSide, PositionEffect};
use crate::order_sys::{FillId, OrderId};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FillLiquidity {
    Maker,
    Taker,
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FillError {
    EmptySymbol,
    NonPositiveQuantity { quantity: Decimal },
    NonPositivePrice { price: Decimal },
    NegativeFees { fees: Decimal },
}

impl std::fmt::Display for FillError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptySymbol => write!(f, "fill symbol cannot be empty"),
            Self::NonPositiveQuantity { quantity } => {
                write!(f, "fill quantity must be positive, got {quantity}")
            }
            Self::NonPositivePrice { price } => {
                write!(f, "fill price must be positive, got {price}")
            }
            Self::NegativeFees { fees } => {
                write!(f, "fill fees cannot be negative, got {fees}")
            }
        }
    }
}

impl std::error::Error for FillError {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fill {
    id: FillId,
    order_id: OrderId,
    symbol: String,
    order_side: OrderSide,
    position_effect: PositionEffect,
    executed_at: DateTime<Utc>,
    qty: Decimal,
    price: Decimal,
    fees: Decimal,
    liquidity: FillLiquidity,
    venue_execution_id: Option<String>,
    venue_order_id: Option<String>,
}

impl Fill {
    pub fn new(
        id: FillId,
        order_id: OrderId,
        symbol: impl Into<String>,
        order_side: OrderSide,
        position_effect: PositionEffect,
        executed_at: DateTime<Utc>,
        qty: Decimal,
        price: Decimal,
        fees: Decimal,
    ) -> Result<Self, FillError> {
        Self::new_with_metadata(
            id,
            order_id,
            symbol,
            order_side,
            position_effect,
            executed_at,
            qty,
            price,
            fees,
            FillLiquidity::Unknown,
            None,
            None,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn new_with_metadata(
        id: FillId,
        order_id: OrderId,
        symbol: impl Into<String>,
        order_side: OrderSide,
        position_effect: PositionEffect,
        executed_at: DateTime<Utc>,
        qty: Decimal,
        price: Decimal,
        fees: Decimal,
        liquidity: FillLiquidity,
        venue_execution_id: Option<String>,
        venue_order_id: Option<String>,
    ) -> Result<Self, FillError> {
        let symbol = symbol.into();
        if symbol.trim().is_empty() {
            return Err(FillError::EmptySymbol);
        }
        if qty <= Decimal::ZERO {
            return Err(FillError::NonPositiveQuantity { quantity: qty });
        }
        if price <= Decimal::ZERO {
            return Err(FillError::NonPositivePrice { price });
        }
        if fees < Decimal::ZERO {
            return Err(FillError::NegativeFees { fees });
        }

        Ok(Self {
            id,
            order_id,
            symbol,
            order_side,
            position_effect,
            executed_at,
            qty,
            price,
            fees,
            liquidity,
            venue_execution_id,
            venue_order_id,
        })
    }

    pub fn id(&self) -> FillId {
        self.id
    }

    pub fn order_id(&self) -> OrderId {
        self.order_id
    }

    pub fn symbol(&self) -> &str {
        &self.symbol
    }

    pub fn order_side(&self) -> OrderSide {
        self.order_side
    }

    pub fn position_effect(&self) -> PositionEffect {
        self.position_effect
    }

    pub fn executed_at(&self) -> DateTime<Utc> {
        self.executed_at
    }

    pub fn qty(&self) -> Decimal {
        self.qty
    }

    pub fn price(&self) -> Decimal {
        self.price
    }

    pub fn fees(&self) -> Decimal {
        self.fees
    }

    pub fn gross_notional(&self) -> Decimal {
        self.qty * self.price
    }

    pub fn liquidity(&self) -> FillLiquidity {
        self.liquidity
    }

    pub fn venue_execution_id(&self) -> Option<&str> {
        self.venue_execution_id.as_deref()
    }

    pub fn venue_order_id(&self) -> Option<&str> {
        self.venue_order_id.as_deref()
    }
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};
    use rust_decimal::Decimal;

    use super::{Fill, FillError, FillLiquidity};
    use crate::order_sys::order::{OrderSide, PositionEffect};
    use crate::order_sys::{FillId, OrderId};

    fn dec(value: i64, scale: u32) -> Decimal {
        Decimal::new(value, scale)
    }

    fn ts(day: u32) -> chrono::DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 5, day, 9, 30, 0).unwrap()
    }

    #[test]
    fn fill_keeps_execution_metadata() {
        let fill = Fill::new_with_metadata(
            FillId::new(1),
            OrderId::new(10),
            "AAPL",
            OrderSide::Buy,
            PositionEffect::Open,
            ts(24),
            dec(5, 0),
            dec(100, 0),
            dec(5, 1),
            FillLiquidity::Taker,
            Some("exec-1".to_owned()),
            Some("venue-order-1".to_owned()),
        )
        .unwrap();

        assert_eq!(fill.gross_notional(), dec(500, 0));
        assert_eq!(fill.liquidity(), FillLiquidity::Taker);
        assert_eq!(fill.venue_execution_id(), Some("exec-1"));
        assert_eq!(fill.venue_order_id(), Some("venue-order-1"));
    }

    #[test]
    fn fill_rejects_non_positive_quantity() {
        let err = Fill::new(
            FillId::new(2),
            OrderId::new(20),
            "AAPL",
            OrderSide::Buy,
            PositionEffect::Open,
            ts(24),
            Decimal::ZERO,
            dec(100, 0),
            dec(0, 0),
        )
        .unwrap_err();

        assert_eq!(
            err,
            FillError::NonPositiveQuantity {
                quantity: Decimal::ZERO,
            }
        );
    }
}
