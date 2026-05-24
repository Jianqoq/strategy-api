use chrono::{DateTime, Utc};
use rust_decimal::Decimal;

use crate::order_sys::{FillId, LotId, OrderId};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LotSide {
    Long,
    Short,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LotStatus {
    Open,
    Closed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LotError {
    NonPositiveQuantity {
        quantity: Decimal,
    },
    NonPositivePrice {
        price: Decimal,
    },
    NegativeFees {
        fees: Decimal,
    },
    CloseExceedsRemaining {
        requested: Decimal,
        remaining: Decimal,
    },
    EventOutOfOrder {
        previous_timestamp: DateTime<Utc>,
        new_timestamp: DateTime<Utc>,
    },
    LotAlreadyClosed {
        lot_id: LotId,
    },
}

impl std::fmt::Display for LotError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NonPositiveQuantity { quantity } => {
                write!(f, "quantity must be positive, got {quantity}")
            }
            Self::NonPositivePrice { price } => {
                write!(f, "price must be positive, got {price}")
            }
            Self::NegativeFees { fees } => {
                write!(f, "fees cannot be negative, got {fees}")
            }
            Self::CloseExceedsRemaining {
                requested,
                remaining,
            } => write!(
                f,
                "close quantity {requested} exceeds remaining quantity {remaining}"
            ),
            Self::EventOutOfOrder {
                previous_timestamp,
                new_timestamp,
            } => write!(
                f,
                "event timestamp {new_timestamp} is earlier than the previous audit timestamp {previous_timestamp}"
            ),
            Self::LotAlreadyClosed { lot_id } => {
                write!(f, "lot {} is already closed", lot_id.value())
            }
        }
    }
}

impl std::error::Error for LotError {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LotClose {
    pub sequence: u32,
    pub close_order_id: OrderId,
    pub close_fill_id: FillId,
    pub closed_at: DateTime<Utc>,
    pub qty: Decimal,
    pub price: Decimal,
    pub gross_notional: Decimal,
    pub close_fees: Decimal,
    pub allocated_open_fees: Decimal,
    pub total_fees: Decimal,
    pub realized_gross_pnl: Decimal,
    pub realized_net_pnl: Decimal,
    pub remaining_qty_after: Decimal,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Lot {
    id: LotId,
    side: LotSide,
    opened_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    closed_at: Option<DateTime<Utc>>,
    open_order_id: OrderId,
    open_fill_id: FillId,
    original_qty: Decimal,
    remaining_qty: Decimal,
    open_price: Decimal,
    open_fees: Decimal,
    open_gross_notional: Decimal,
    close_events: Vec<LotClose>,
    realized_gross_pnl: Decimal,
    realized_open_fees: Decimal,
    realized_close_fees: Decimal,
    realized_net_pnl: Decimal,
}

impl Lot {
    pub fn open(
        id: LotId,
        side: LotSide,
        open_order_id: OrderId,
        open_fill_id: FillId,
        opened_at: DateTime<Utc>,
        original_qty: Decimal,
        open_price: Decimal,
        open_fees: Decimal,
    ) -> Result<Self, LotError> {
        validate_positive_quantity(original_qty)?;
        validate_positive_price(open_price)?;
        validate_non_negative_fees(open_fees)?;

        Ok(Self {
            id,
            side,
            opened_at,
            updated_at: opened_at,
            closed_at: None,
            open_order_id,
            open_fill_id,
            original_qty,
            remaining_qty: original_qty,
            open_price,
            open_fees,
            open_gross_notional: original_qty * open_price,
            close_events: Vec::new(),
            realized_gross_pnl: Decimal::ZERO,
            realized_open_fees: Decimal::ZERO,
            realized_close_fees: Decimal::ZERO,
            realized_net_pnl: Decimal::ZERO,
        })
    }

    pub fn close(
        &mut self,
        close_order_id: OrderId,
        close_fill_id: FillId,
        closed_at: DateTime<Utc>,
        qty: Decimal,
        price: Decimal,
        close_fees: Decimal,
    ) -> Result<&LotClose, LotError> {
        if self.is_closed() {
            return Err(LotError::LotAlreadyClosed { lot_id: self.id });
        }

        validate_positive_quantity(qty)?;
        validate_positive_price(price)?;
        validate_non_negative_fees(close_fees)?;

        if closed_at < self.updated_at {
            return Err(LotError::EventOutOfOrder {
                previous_timestamp: self.updated_at,
                new_timestamp: closed_at,
            });
        }

        if qty > self.remaining_qty {
            return Err(LotError::CloseExceedsRemaining {
                requested: qty,
                remaining: self.remaining_qty,
            });
        }

        let allocated_open_fees = if qty == self.remaining_qty {
            self.open_fees - self.realized_open_fees
        } else {
            self.open_fees * qty / self.original_qty
        };
        let total_fees = allocated_open_fees + close_fees;
        let realized_gross_pnl = match self.side {
            LotSide::Long => (price - self.open_price) * qty,
            LotSide::Short => (self.open_price - price) * qty,
        };
        let realized_net_pnl = realized_gross_pnl - total_fees;
        let remaining_qty_after = self.remaining_qty - qty;

        let close_event = LotClose {
            sequence: u32::try_from(self.close_events.len() + 1)
                .expect("lot close event sequence overflowed u32"),
            close_order_id,
            close_fill_id,
            closed_at,
            qty,
            price,
            gross_notional: qty * price,
            close_fees,
            allocated_open_fees,
            total_fees,
            realized_gross_pnl,
            realized_net_pnl,
            remaining_qty_after,
        };

        self.remaining_qty = remaining_qty_after;
        self.updated_at = closed_at;
        self.realized_gross_pnl += realized_gross_pnl;
        self.realized_open_fees += allocated_open_fees;
        self.realized_close_fees += close_fees;
        self.realized_net_pnl += realized_net_pnl;

        if self.remaining_qty.is_zero() {
            self.closed_at = Some(closed_at);
        }

        self.close_events.push(close_event);
        Ok(self
            .close_events
            .last()
            .expect("close event was pushed before returning"))
    }

    pub fn id(&self) -> LotId {
        self.id
    }

    pub fn side(&self) -> LotSide {
        self.side
    }

    pub fn status(&self) -> LotStatus {
        if self.is_closed() {
            LotStatus::Closed
        } else {
            LotStatus::Open
        }
    }

    pub fn is_open(&self) -> bool {
        !self.is_closed()
    }

    pub fn is_closed(&self) -> bool {
        self.remaining_qty.is_zero()
    }

    pub fn opened_at(&self) -> DateTime<Utc> {
        self.opened_at
    }

    pub fn updated_at(&self) -> DateTime<Utc> {
        self.updated_at
    }

    pub fn closed_at(&self) -> Option<DateTime<Utc>> {
        self.closed_at
    }

    pub fn open_order_id(&self) -> OrderId {
        self.open_order_id
    }

    pub fn open_fill_id(&self) -> FillId {
        self.open_fill_id
    }

    pub fn original_qty(&self) -> Decimal {
        self.original_qty
    }

    pub fn closed_qty(&self) -> Decimal {
        self.original_qty - self.remaining_qty
    }

    pub fn remaining_qty(&self) -> Decimal {
        self.remaining_qty
    }

    pub fn open_price(&self) -> Decimal {
        self.open_price
    }

    pub fn open_fees(&self) -> Decimal {
        self.open_fees
    }

    pub fn open_gross_notional(&self) -> Decimal {
        self.open_gross_notional
    }

    pub fn close_events(&self) -> &[LotClose] {
        &self.close_events
    }

    pub fn realized_gross_pnl(&self) -> Decimal {
        self.realized_gross_pnl
    }

    pub fn realized_open_fees(&self) -> Decimal {
        self.realized_open_fees
    }

    pub fn realized_close_fees(&self) -> Decimal {
        self.realized_close_fees
    }

    pub fn realized_total_fees(&self) -> Decimal {
        self.realized_open_fees + self.realized_close_fees
    }

    pub fn realized_net_pnl(&self) -> Decimal {
        self.realized_net_pnl
    }
}

fn validate_positive_quantity(quantity: Decimal) -> Result<(), LotError> {
    if quantity <= Decimal::ZERO {
        return Err(LotError::NonPositiveQuantity { quantity });
    }

    Ok(())
}

fn validate_positive_price(price: Decimal) -> Result<(), LotError> {
    if price <= Decimal::ZERO {
        return Err(LotError::NonPositivePrice { price });
    }

    Ok(())
}

fn validate_non_negative_fees(fees: Decimal) -> Result<(), LotError> {
    if fees < Decimal::ZERO {
        return Err(LotError::NegativeFees { fees });
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use chrono::{DateTime, TimeZone, Utc};
    use rust_decimal::Decimal;

    use super::{Lot, LotError, LotSide, LotStatus};
    use crate::order_sys::{FillId, LotId, OrderId};

    fn dec(value: i64, scale: u32) -> Decimal {
        Decimal::new(value, scale)
    }

    fn ts(day: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 5, day, 9, 30, 0).unwrap()
    }

    #[test]
    fn open_lot_keeps_open_audit_snapshot() {
        let lot = Lot::open(
            LotId::new(1),
            LotSide::Long,
            OrderId::new(10),
            FillId::new(100),
            ts(24),
            dec(10, 0),
            dec(100, 0),
            dec(2, 0),
        )
        .unwrap();

        assert_eq!(lot.id(), LotId::new(1));
        assert_eq!(lot.side(), LotSide::Long);
        assert_eq!(lot.status(), LotStatus::Open);
        assert_eq!(lot.opened_at(), ts(24));
        assert_eq!(lot.updated_at(), ts(24));
        assert_eq!(lot.open_order_id(), OrderId::new(10));
        assert_eq!(lot.open_fill_id(), FillId::new(100));
        assert_eq!(lot.original_qty(), dec(10, 0));
        assert_eq!(lot.remaining_qty(), dec(10, 0));
        assert_eq!(lot.open_price(), dec(100, 0));
        assert_eq!(lot.open_fees(), dec(2, 0));
        assert_eq!(lot.open_gross_notional(), dec(1000, 0));
        assert!(lot.close_events().is_empty());
    }

    #[test]
    fn partial_and_full_close_keep_complete_audit_trail() {
        let mut lot = Lot::open(
            LotId::new(2),
            LotSide::Long,
            OrderId::new(20),
            FillId::new(200),
            ts(24),
            dec(10, 0),
            dec(100, 0),
            dec(2, 0),
        )
        .unwrap();

        let first_close = lot
            .close(
                OrderId::new(21),
                FillId::new(201),
                ts(25),
                dec(4, 0),
                dec(110, 0),
                dec(1, 0),
            )
            .unwrap()
            .clone();

        assert_eq!(first_close.sequence, 1);
        assert_eq!(first_close.gross_notional, dec(440, 0));
        assert_eq!(first_close.allocated_open_fees, dec(8, 1));
        assert_eq!(first_close.close_fees, dec(1, 0));
        assert_eq!(first_close.total_fees, dec(18, 1));
        assert_eq!(first_close.realized_gross_pnl, dec(40, 0));
        assert_eq!(first_close.realized_net_pnl, dec(382, 1));
        assert_eq!(first_close.remaining_qty_after, dec(6, 0));

        let second_close = lot
            .close(
                OrderId::new(22),
                FillId::new(202),
                ts(26),
                dec(6, 0),
                dec(90, 0),
                dec(15, 1),
            )
            .unwrap()
            .clone();

        assert_eq!(second_close.sequence, 2);
        assert_eq!(second_close.allocated_open_fees, dec(12, 1));
        assert_eq!(second_close.total_fees, dec(27, 1));
        assert_eq!(second_close.realized_gross_pnl, dec(-60, 0));
        assert_eq!(second_close.realized_net_pnl, dec(-627, 1));
        assert_eq!(second_close.remaining_qty_after, Decimal::ZERO);

        assert_eq!(lot.status(), LotStatus::Closed);
        assert_eq!(lot.closed_at(), Some(ts(26)));
        assert_eq!(lot.closed_qty(), dec(10, 0));
        assert_eq!(lot.remaining_qty(), Decimal::ZERO);
        assert_eq!(lot.realized_gross_pnl(), dec(-20, 0));
        assert_eq!(lot.realized_open_fees(), dec(2, 0));
        assert_eq!(lot.realized_close_fees(), dec(25, 1));
        assert_eq!(lot.realized_total_fees(), dec(45, 1));
        assert_eq!(lot.realized_net_pnl(), dec(-245, 1));
        assert_eq!(lot.close_events().len(), 2);
    }

    #[test]
    fn short_lot_realizes_pnl_when_buying_back_lower() {
        let mut lot = Lot::open(
            LotId::new(3),
            LotSide::Short,
            OrderId::new(30),
            FillId::new(300),
            ts(24),
            dec(5, 0),
            dec(100, 0),
            dec(5, 1),
        )
        .unwrap();

        let close = lot
            .close(
                OrderId::new(31),
                FillId::new(301),
                ts(25),
                dec(5, 0),
                dec(92, 0),
                dec(25, 2),
            )
            .unwrap()
            .clone();

        assert_eq!(close.realized_gross_pnl, dec(40, 0));
        assert_eq!(close.total_fees, dec(75, 2));
        assert_eq!(close.realized_net_pnl, dec(3925, 2));
        assert_eq!(lot.realized_net_pnl(), dec(3925, 2));
    }

    #[test]
    fn close_rejects_invalid_quantity_and_out_of_order_timestamps() {
        let mut lot = Lot::open(
            LotId::new(4),
            LotSide::Long,
            OrderId::new(40),
            FillId::new(400),
            ts(24),
            dec(3, 0),
            dec(50, 0),
            dec(0, 0),
        )
        .unwrap();

        let too_large = lot
            .close(
                OrderId::new(41),
                FillId::new(401),
                ts(25),
                dec(4, 0),
                dec(55, 0),
                dec(0, 0),
            )
            .unwrap_err();
        assert_eq!(
            too_large,
            LotError::CloseExceedsRemaining {
                requested: dec(4, 0),
                remaining: dec(3, 0),
            }
        );

        let out_of_order = lot
            .close(
                OrderId::new(42),
                FillId::new(402),
                ts(23),
                dec(1, 0),
                dec(55, 0),
                dec(0, 0),
            )
            .unwrap_err();
        assert_eq!(
            out_of_order,
            LotError::EventOutOfOrder {
                previous_timestamp: ts(24),
                new_timestamp: ts(23),
            }
        );
    }
}
