use rust_decimal::Decimal;

use crate::order_sys::fill::Fill;
use crate::order_sys::lot::{Lot, LotClose, LotError, LotSide};
use crate::order_sys::order::{OrderSide, PositionEffect};
use crate::order_sys::{FillId, HoldingId, LotId, OrderId};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LotReliefMethod {
    Fifo,
    Lifo,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HoldingError {
    EmptySymbol,
    SymbolMismatch {
        expected: String,
        actual: String,
    },
    MixedLotSide {
        expected: LotSide,
        actual: LotSide,
    },
    PositionEffectMismatch {
        expected: PositionEffect,
        actual: PositionEffect,
    },
    CloseSideMismatch {
        holding_side: LotSide,
        fill_side: OrderSide,
    },
    NoOpenLots {
        symbol: String,
    },
    CloseExceedsOpenQuantity {
        requested: Decimal,
        available: Decimal,
    },
    DuplicateFillId {
        fill_id: FillId,
    },
    Lot(LotError),
}

impl std::fmt::Display for HoldingError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptySymbol => write!(f, "holding symbol cannot be empty"),
            Self::SymbolMismatch { expected, actual } => {
                write!(
                    f,
                    "holding symbol mismatch: expected {expected}, got {actual}"
                )
            }
            Self::MixedLotSide { expected, actual } => write!(
                f,
                "holding mixes lot sides: expected {:?}, got {:?}",
                expected, actual
            ),
            Self::PositionEffectMismatch { expected, actual } => write!(
                f,
                "holding expected position effect {:?}, got {:?}",
                expected, actual
            ),
            Self::CloseSideMismatch {
                holding_side,
                fill_side,
            } => write!(
                f,
                "fill side {:?} does not close {:?} lots",
                fill_side, holding_side
            ),
            Self::NoOpenLots { symbol } => {
                write!(f, "holding for {symbol} has no open lots")
            }
            Self::CloseExceedsOpenQuantity {
                requested,
                available,
            } => write!(
                f,
                "requested close quantity {requested} exceeds open quantity {available}"
            ),
            Self::DuplicateFillId { fill_id } => {
                write!(
                    f,
                    "fill {} was already applied to this holding",
                    fill_id.value()
                )
            }
            Self::Lot(err) => err.fmt(f),
        }
    }
}

impl std::error::Error for HoldingError {}

impl From<LotError> for HoldingError {
    fn from(value: LotError) -> Self {
        Self::Lot(value)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HoldingCloseMatch {
    pub lot_id: LotId,
    pub lot_side: LotSide,
    pub close: LotClose,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HoldingClose {
    pub symbol: String,
    pub close_order_id: OrderId,
    pub close_fill_id: FillId,
    pub closed_qty: Decimal,
    pub close_price: Decimal,
    pub close_fees: Decimal,
    pub matches: Vec<HoldingCloseMatch>,
    pub total_realized_gross_pnl: Decimal,
    pub total_realized_net_pnl: Decimal,
    pub remaining_open_qty: Decimal,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Holding {
    id: HoldingId,
    symbol: String,
    lot_relief_method: LotReliefMethod,
    lots: Vec<Lot>,
}

impl Holding {
    pub fn new(
        id: HoldingId,
        symbol: impl Into<String>,
        lot_relief_method: LotReliefMethod,
    ) -> Result<Self, HoldingError> {
        let symbol = symbol.into();
        if symbol.trim().is_empty() {
            return Err(HoldingError::EmptySymbol);
        }

        Ok(Self {
            id,
            symbol,
            lot_relief_method,
            lots: Vec::new(),
        })
    }

    pub fn id(&self) -> HoldingId {
        self.id
    }

    pub fn symbol(&self) -> &str {
        &self.symbol
    }

    pub fn lot_relief_method(&self) -> LotReliefMethod {
        self.lot_relief_method
    }

    pub fn lots(&self) -> &[Lot] {
        &self.lots
    }

    pub fn side(&self) -> Option<LotSide> {
        self.lots.iter().find(|lot| lot.is_open()).map(Lot::side)
    }

    pub fn open_qty(&self) -> Decimal {
        self.lots
            .iter()
            .fold(Decimal::ZERO, |sum, lot| sum + lot.remaining_qty())
    }

    pub fn closed_qty(&self) -> Decimal {
        self.lots
            .iter()
            .fold(Decimal::ZERO, |sum, lot| sum + lot.closed_qty())
    }

    pub fn open_lot_count(&self) -> usize {
        self.lots.iter().filter(|lot| lot.is_open()).count()
    }

    pub fn is_flat(&self) -> bool {
        self.open_qty().is_zero()
    }

    pub fn realized_gross_pnl(&self) -> Decimal {
        self.lots
            .iter()
            .fold(Decimal::ZERO, |sum, lot| sum + lot.realized_gross_pnl())
    }

    pub fn realized_net_pnl(&self) -> Decimal {
        self.lots
            .iter()
            .fold(Decimal::ZERO, |sum, lot| sum + lot.realized_net_pnl())
    }

    pub fn open_lot(&mut self, lot: Lot) -> Result<&Lot, HoldingError> {
        if let Some(existing_side) = self.side()
            && existing_side != lot.side()
        {
            return Err(HoldingError::MixedLotSide {
                expected: existing_side,
                actual: lot.side(),
            });
        }

        self.lots.push(lot);
        Ok(self
            .lots
            .last()
            .expect("holding must contain the lot that was just pushed"))
    }

    pub fn open_lot_from_fill(&mut self, lot_id: LotId, fill: &Fill) -> Result<&Lot, HoldingError> {
        self.ensure_symbol_matches(fill.symbol())?;
        self.ensure_fill_not_applied(fill.id())?;
        if fill.position_effect() != PositionEffect::Open {
            return Err(HoldingError::PositionEffectMismatch {
                expected: PositionEffect::Open,
                actual: fill.position_effect(),
            });
        }

        let lot_side = fill.order_side().to_open_lot_side();
        let lot = Lot::open(
            lot_id,
            lot_side,
            fill.order_id(),
            fill.id(),
            fill.executed_at(),
            fill.qty(),
            fill.price(),
            fill.fees(),
        )?;
        self.open_lot(lot)
    }

    pub fn close_with_fill(&mut self, fill: &Fill) -> Result<HoldingClose, HoldingError> {
        self.ensure_symbol_matches(fill.symbol())?;
        self.ensure_fill_not_applied(fill.id())?;
        if fill.position_effect() != PositionEffect::Close {
            return Err(HoldingError::PositionEffectMismatch {
                expected: PositionEffect::Close,
                actual: fill.position_effect(),
            });
        }

        let holding_side = self.side().ok_or_else(|| HoldingError::NoOpenLots {
            symbol: self.symbol.clone(),
        })?;
        if fill.order_side().closing_lot_side() != holding_side {
            return Err(HoldingError::CloseSideMismatch {
                holding_side,
                fill_side: fill.order_side(),
            });
        }

        let open_qty = self.open_qty();
        if fill.qty() > open_qty {
            return Err(HoldingError::CloseExceedsOpenQuantity {
                requested: fill.qty(),
                available: open_qty,
            });
        }

        let mut remaining_qty = fill.qty();
        let mut remaining_fees = fill.fees();
        let mut matches = Vec::new();

        for lot_index in self.close_order_indices() {
            if remaining_qty.is_zero() {
                break;
            }

            let lot = &mut self.lots[lot_index];
            let close_qty = remaining_qty.min(lot.remaining_qty());
            let allocated_close_fees = if close_qty == remaining_qty {
                remaining_fees
            } else {
                fill.fees() * close_qty / fill.qty()
            };

            let close = lot
                .close(
                    fill.order_id(),
                    fill.id(),
                    fill.executed_at(),
                    close_qty,
                    fill.price(),
                    allocated_close_fees,
                )?
                .clone();

            matches.push(HoldingCloseMatch {
                lot_id: lot.id(),
                lot_side: lot.side(),
                close,
            });

            remaining_qty -= close_qty;
            remaining_fees -= allocated_close_fees;
        }

        debug_assert!(
            remaining_qty.is_zero(),
            "close allocation left unmatched quantity"
        );
        debug_assert!(
            remaining_fees.is_zero(),
            "close allocation left unmatched fees"
        );

        let total_realized_gross_pnl = matches.iter().fold(Decimal::ZERO, |sum, matched| {
            sum + matched.close.realized_gross_pnl
        });
        let total_realized_net_pnl = matches.iter().fold(Decimal::ZERO, |sum, matched| {
            sum + matched.close.realized_net_pnl
        });

        Ok(HoldingClose {
            symbol: self.symbol.clone(),
            close_order_id: fill.order_id(),
            close_fill_id: fill.id(),
            closed_qty: fill.qty(),
            close_price: fill.price(),
            close_fees: fill.fees(),
            matches,
            total_realized_gross_pnl,
            total_realized_net_pnl,
            remaining_open_qty: self.open_qty(),
        })
    }

    fn ensure_symbol_matches(&self, symbol: &str) -> Result<(), HoldingError> {
        if self.symbol != symbol {
            return Err(HoldingError::SymbolMismatch {
                expected: self.symbol.clone(),
                actual: symbol.to_owned(),
            });
        }

        Ok(())
    }

    fn ensure_fill_not_applied(&self, fill_id: FillId) -> Result<(), HoldingError> {
        if self.lots.iter().any(|lot| {
            lot.open_fill_id() == fill_id
                || lot
                    .close_events()
                    .iter()
                    .any(|close_event| close_event.close_fill_id == fill_id)
        }) {
            return Err(HoldingError::DuplicateFillId { fill_id });
        }

        Ok(())
    }

    fn close_order_indices(&self) -> Vec<usize> {
        let open_indices = self
            .lots
            .iter()
            .enumerate()
            .filter_map(|(index, lot)| lot.is_open().then_some(index));

        match self.lot_relief_method {
            LotReliefMethod::Fifo => open_indices.collect(),
            LotReliefMethod::Lifo => open_indices.rev().collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};
    use rust_decimal::Decimal;

    use super::{Holding, HoldingError, LotReliefMethod};
    use crate::order_sys::fill::Fill;
    use crate::order_sys::lot::LotSide;
    use crate::order_sys::order::{OrderSide, PositionEffect};
    use crate::order_sys::{FillId, HoldingId, LotId, OrderId};

    fn dec(value: i64, scale: u32) -> Decimal {
        Decimal::new(value, scale)
    }

    fn ts(day: u32) -> chrono::DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 5, day, 9, 30, 0).unwrap()
    }

    #[test]
    fn fifo_close_matches_oldest_lots_first() {
        let mut holding = Holding::new(HoldingId::new(1), "AAPL", LotReliefMethod::Fifo).unwrap();

        let open_one = Fill::new(
            FillId::new(1),
            OrderId::new(100),
            "AAPL",
            OrderSide::Buy,
            PositionEffect::Open,
            ts(24),
            dec(5, 0),
            dec(100, 0),
            dec(1, 0),
        )
        .unwrap();
        let open_two = Fill::new(
            FillId::new(2),
            OrderId::new(101),
            "AAPL",
            OrderSide::Buy,
            PositionEffect::Open,
            ts(25),
            dec(5, 0),
            dec(105, 0),
            dec(1, 0),
        )
        .unwrap();
        let close = Fill::new(
            FillId::new(3),
            OrderId::new(102),
            "AAPL",
            OrderSide::Sell,
            PositionEffect::Close,
            ts(26),
            dec(6, 0),
            dec(110, 0),
            dec(12, 1),
        )
        .unwrap();

        holding
            .open_lot_from_fill(LotId::new(10), &open_one)
            .unwrap();
        holding
            .open_lot_from_fill(LotId::new(11), &open_two)
            .unwrap();

        let result = holding.close_with_fill(&close).unwrap();

        assert_eq!(holding.side(), Some(LotSide::Long));
        assert_eq!(result.matches.len(), 2);
        assert_eq!(result.matches[0].lot_id, LotId::new(10));
        assert_eq!(result.matches[0].close.qty, dec(5, 0));
        assert_eq!(result.matches[0].close.allocated_open_fees, dec(1, 0));
        assert_eq!(result.matches[0].close.close_fees, dec(1, 0));
        assert_eq!(result.matches[1].lot_id, LotId::new(11));
        assert_eq!(result.matches[1].close.qty, dec(1, 0));
        assert_eq!(result.matches[1].close.close_fees, dec(2, 1));
        assert_eq!(result.total_realized_gross_pnl, dec(55, 0));
        assert_eq!(result.total_realized_net_pnl, dec(526, 1));
        assert_eq!(result.remaining_open_qty, dec(4, 0));
        assert_eq!(holding.open_qty(), dec(4, 0));
        assert_eq!(holding.realized_net_pnl(), dec(526, 1));
    }

    #[test]
    fn lifo_close_matches_newest_lots_first() {
        let mut holding = Holding::new(HoldingId::new(2), "AAPL", LotReliefMethod::Lifo).unwrap();

        let open_one = Fill::new(
            FillId::new(4),
            OrderId::new(200),
            "AAPL",
            OrderSide::Buy,
            PositionEffect::Open,
            ts(24),
            dec(5, 0),
            dec(100, 0),
            dec(1, 0),
        )
        .unwrap();
        let open_two = Fill::new(
            FillId::new(5),
            OrderId::new(201),
            "AAPL",
            OrderSide::Buy,
            PositionEffect::Open,
            ts(25),
            dec(5, 0),
            dec(105, 0),
            dec(1, 0),
        )
        .unwrap();
        let close = Fill::new(
            FillId::new(6),
            OrderId::new(202),
            "AAPL",
            OrderSide::Sell,
            PositionEffect::Close,
            ts(26),
            dec(6, 0),
            dec(110, 0),
            dec(12, 1),
        )
        .unwrap();

        holding
            .open_lot_from_fill(LotId::new(20), &open_one)
            .unwrap();
        holding
            .open_lot_from_fill(LotId::new(21), &open_two)
            .unwrap();

        let result = holding.close_with_fill(&close).unwrap();

        assert_eq!(result.matches.len(), 2);
        assert_eq!(result.matches[0].lot_id, LotId::new(21));
        assert_eq!(result.matches[0].close.qty, dec(5, 0));
        assert_eq!(result.matches[1].lot_id, LotId::new(20));
        assert_eq!(result.matches[1].close.qty, dec(1, 0));
        assert_eq!(result.total_realized_gross_pnl, dec(35, 0));
        assert_eq!(result.total_realized_net_pnl, dec(326, 1));
    }

    #[test]
    fn holding_rejects_mixed_sides_and_close_oversize() {
        let mut holding = Holding::new(HoldingId::new(3), "AAPL", LotReliefMethod::Fifo).unwrap();

        let long_fill = Fill::new(
            FillId::new(7),
            OrderId::new(300),
            "AAPL",
            OrderSide::Buy,
            PositionEffect::Open,
            ts(24),
            dec(2, 0),
            dec(100, 0),
            dec(0, 0),
        )
        .unwrap();
        let short_fill = Fill::new(
            FillId::new(8),
            OrderId::new(301),
            "AAPL",
            OrderSide::Sell,
            PositionEffect::Open,
            ts(25),
            dec(1, 0),
            dec(101, 0),
            dec(0, 0),
        )
        .unwrap();
        let oversize_close = Fill::new(
            FillId::new(9),
            OrderId::new(302),
            "AAPL",
            OrderSide::Sell,
            PositionEffect::Close,
            ts(26),
            dec(3, 0),
            dec(102, 0),
            dec(0, 0),
        )
        .unwrap();

        holding
            .open_lot_from_fill(LotId::new(30), &long_fill)
            .unwrap();

        let mixed_side = holding
            .open_lot_from_fill(LotId::new(31), &short_fill)
            .unwrap_err();
        assert_eq!(
            mixed_side,
            HoldingError::MixedLotSide {
                expected: LotSide::Long,
                actual: LotSide::Short,
            }
        );

        let oversize = holding.close_with_fill(&oversize_close).unwrap_err();
        assert_eq!(
            oversize,
            HoldingError::CloseExceedsOpenQuantity {
                requested: dec(3, 0),
                available: dec(2, 0),
            }
        );
    }

    #[test]
    fn holding_can_open_opposite_side_after_flattening() {
        let mut holding = Holding::new(HoldingId::new(4), "AAPL", LotReliefMethod::Fifo).unwrap();

        let open_long = Fill::new(
            FillId::new(10),
            OrderId::new(400),
            "AAPL",
            OrderSide::Buy,
            PositionEffect::Open,
            ts(24),
            dec(2, 0),
            dec(100, 0),
            dec(0, 0),
        )
        .unwrap();
        let close_long = Fill::new(
            FillId::new(11),
            OrderId::new(401),
            "AAPL",
            OrderSide::Sell,
            PositionEffect::Close,
            ts(25),
            dec(2, 0),
            dec(101, 0),
            dec(0, 0),
        )
        .unwrap();
        let open_short = Fill::new(
            FillId::new(12),
            OrderId::new(402),
            "AAPL",
            OrderSide::Sell,
            PositionEffect::Open,
            ts(26),
            dec(1, 0),
            dec(99, 0),
            dec(0, 0),
        )
        .unwrap();

        holding
            .open_lot_from_fill(LotId::new(40), &open_long)
            .unwrap();
        holding.close_with_fill(&close_long).unwrap();

        assert!(holding.is_flat());
        assert_eq!(holding.side(), None);
        assert!(
            holding
                .open_lot_from_fill(LotId::new(41), &open_short)
                .is_ok()
        );
        assert_eq!(holding.side(), Some(LotSide::Short));
    }

    #[test]
    fn holding_close_on_flat_returns_no_open_lots() {
        let mut holding = Holding::new(HoldingId::new(5), "AAPL", LotReliefMethod::Fifo).unwrap();

        let open_long = Fill::new(
            FillId::new(13),
            OrderId::new(500),
            "AAPL",
            OrderSide::Buy,
            PositionEffect::Open,
            ts(24),
            dec(1, 0),
            dec(100, 0),
            dec(0, 0),
        )
        .unwrap();
        let close_long = Fill::new(
            FillId::new(14),
            OrderId::new(501),
            "AAPL",
            OrderSide::Sell,
            PositionEffect::Close,
            ts(25),
            dec(1, 0),
            dec(101, 0),
            dec(0, 0),
        )
        .unwrap();
        let extra_close = Fill::new(
            FillId::new(15),
            OrderId::new(502),
            "AAPL",
            OrderSide::Sell,
            PositionEffect::Close,
            ts(26),
            dec(1, 0),
            dec(102, 0),
            dec(0, 0),
        )
        .unwrap();

        holding
            .open_lot_from_fill(LotId::new(50), &open_long)
            .unwrap();
        holding.close_with_fill(&close_long).unwrap();

        let err = holding.close_with_fill(&extra_close).unwrap_err();
        assert_eq!(
            err,
            HoldingError::NoOpenLots {
                symbol: "AAPL".to_owned(),
            }
        );
    }

    #[test]
    fn holding_rejects_duplicate_open_fill_replay() {
        let mut holding = Holding::new(HoldingId::new(6), "AAPL", LotReliefMethod::Fifo).unwrap();

        let open_fill = Fill::new(
            FillId::new(16),
            OrderId::new(600),
            "AAPL",
            OrderSide::Buy,
            PositionEffect::Open,
            ts(24),
            dec(1, 0),
            dec(100, 0),
            dec(0, 0),
        )
        .unwrap();

        holding
            .open_lot_from_fill(LotId::new(60), &open_fill)
            .unwrap();

        let err = holding
            .open_lot_from_fill(LotId::new(61), &open_fill)
            .unwrap_err();
        assert_eq!(
            err,
            HoldingError::DuplicateFillId {
                fill_id: FillId::new(16),
            }
        );
    }

    #[test]
    fn holding_rejects_duplicate_close_fill_replay() {
        let mut holding = Holding::new(HoldingId::new(7), "AAPL", LotReliefMethod::Fifo).unwrap();

        let open_fill = Fill::new(
            FillId::new(17),
            OrderId::new(700),
            "AAPL",
            OrderSide::Buy,
            PositionEffect::Open,
            ts(24),
            dec(2, 0),
            dec(100, 0),
            dec(0, 0),
        )
        .unwrap();
        let close_fill = Fill::new(
            FillId::new(18),
            OrderId::new(701),
            "AAPL",
            OrderSide::Sell,
            PositionEffect::Close,
            ts(25),
            dec(1, 0),
            dec(101, 0),
            dec(0, 0),
        )
        .unwrap();

        holding
            .open_lot_from_fill(LotId::new(70), &open_fill)
            .unwrap();
        holding.close_with_fill(&close_fill).unwrap();

        let err = holding.close_with_fill(&close_fill).unwrap_err();
        assert_eq!(
            err,
            HoldingError::DuplicateFillId {
                fill_id: FillId::new(18),
            }
        );
    }
}
