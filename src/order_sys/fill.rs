//! Immutable execution facts.
//!
//! A [`Fill`] represents what actually happened in the market or simulator.
//! Unlike [`crate::order_sys::order::Order`], which models intent and state
//! transitions, a fill is an immutable record of one executed slice.

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use thiserror::Error;

use crate::order_sys::order::{OrderSide, PositionEffect};
use crate::order_sys::{FillId, OrderId};

/// Whether the fill provided or removed liquidity.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FillLiquidity {
    /// The fill added liquidity to the venue book.
    Maker,
    /// The fill removed liquidity from the venue book.
    Taker,
    /// Liquidity side is unknown or was not reported.
    Unknown,
}

/// Domain errors raised while constructing a fill.
#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum FillError {
    /// The symbol string was empty or whitespace.
    #[error("fill symbol cannot be empty")]
    EmptySymbol,
    /// The executed quantity must be strictly positive.
    #[error("fill quantity must be positive, got {quantity}")]
    NonPositiveQuantity {
        /// The invalid quantity supplied by the caller.
        quantity: Decimal,
    },
    /// The execution price must be strictly positive.
    #[error("fill price must be positive, got {price}")]
    NonPositivePrice {
        /// The invalid price supplied by the caller.
        price: Decimal,
    },
    /// Fees must not be negative.
    #[error("fill fees cannot be negative, got {fees}")]
    NegativeFees {
        /// The invalid fee amount supplied by the caller.
        fees: Decimal,
    },
}

/// Immutable record of one execution slice.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fill {
    /// Unique identifier of the fill.
    id: FillId,
    /// Order that this fill belongs to.
    order_id: OrderId,
    /// Instrument symbol, for example `AAPL`.
    symbol: String,
    /// Side of the parent order.
    order_side: OrderSide,
    /// Whether this fill opens or closes position exposure.
    position_effect: PositionEffect,
    /// UTC execution timestamp.
    executed_at: DateTime<Utc>,
    /// Executed quantity.
    qty: Decimal,
    /// Execution price.
    price: Decimal,
    /// Fees charged for this execution slice.
    fees: Decimal,
    /// Whether the fill provided or removed liquidity.
    liquidity: FillLiquidity,
    /// Venue-native execution identifier, if one exists.
    venue_execution_id: Option<String>,
    /// Venue-native order identifier, if one exists.
    venue_order_id: Option<String>,
}

impl Fill {
    /// Creates a fill with default metadata.
    ///
    /// This constructor is convenient when venue identifiers and liquidity side
    /// are not available.
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

    /// Creates a fill with full optional venue metadata.
    ///
    /// The constructor validates the symbol, quantity, price, and fee values so
    /// downstream consumers can treat a `Fill` as a fully trusted immutable fact.
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

    /// Returns the fill identifier.
    pub fn id(&self) -> FillId {
        self.id
    }

    /// Returns the parent order identifier.
    pub fn order_id(&self) -> OrderId {
        self.order_id
    }

    /// Returns the instrument symbol.
    pub fn symbol(&self) -> &str {
        &self.symbol
    }

    /// Returns the side of the parent order.
    pub fn order_side(&self) -> OrderSide {
        self.order_side
    }

    /// Returns whether the fill opens or closes exposure.
    pub fn position_effect(&self) -> PositionEffect {
        self.position_effect
    }

    /// Returns the execution timestamp.
    pub fn executed_at(&self) -> DateTime<Utc> {
        self.executed_at
    }

    /// Returns the executed quantity.
    pub fn qty(&self) -> Decimal {
        self.qty
    }

    /// Returns the execution price.
    pub fn price(&self) -> Decimal {
        self.price
    }

    /// Returns the execution fees.
    pub fn fees(&self) -> Decimal {
        self.fees
    }

    /// Returns gross executed notional, equal to `qty * price`.
    pub fn gross_notional(&self) -> Decimal {
        self.qty * self.price
    }

    /// Returns the liquidity side if one was recorded.
    pub fn liquidity(&self) -> FillLiquidity {
        self.liquidity
    }

    /// Returns the venue execution identifier, if any.
    pub fn venue_execution_id(&self) -> Option<&str> {
        self.venue_execution_id.as_deref()
    }

    /// Returns the venue order identifier, if any.
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
