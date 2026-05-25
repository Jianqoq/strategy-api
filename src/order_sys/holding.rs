//! Per-symbol position aggregate and lot-relief engine.
//!
//! A [`Holding`] owns every open and closed [`Lot`](crate::order_sys::lot::Lot)
//! for one symbol. It is responsible for:
//!
//! - preventing mixed long/short exposure inside the same aggregate,
//! - translating fills into opened lots,
//! - selecting which lots to close when a closing fill arrives,
//! - preserving an auditable close trail across many lots.
//!
//! The lot-relief selection is modeled as a two-step process:
//!
//! 1. Build a close plan that decides which lot quantities should be consumed.
//! 2. Apply that plan to the lots and record realized audit events.
//!
//! This separation keeps the matching logic testable and allows advanced
//! methods such as `SpecificLot` and `AverageCost` without duplicating close
//! execution logic.

use std::cmp::Ordering;
use std::collections::HashSet;

use rust_decimal::Decimal;
use thiserror::Error;

use crate::order_sys::fill::Fill;
use crate::order_sys::lot::{Lot, LotClose, LotError, LotSide};
use crate::order_sys::order::{OrderSide, PositionEffect};
use crate::order_sys::{FillId, HoldingId, LotId, OrderId};

/// Policy that decides which open lots should be relieved first.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LotReliefMethod {
    /// Oldest open lots first.
    Fifo,
    /// Newest open lots first.
    Lifo,
    /// Highest fee-adjusted cost basis first.
    Hifo,
    /// Lots with the worst estimated unit net PnL first.
    MaxLoss,
    /// Lots with the best estimated unit net PnL first.
    MaxGain,
    /// Follow the caller-provided lot order exactly.
    ///
    /// The list contains lot identifiers, not quantities. The engine consumes
    /// each selected lot in list order until the requested close quantity is
    /// satisfied or the selection runs out.
    SpecificLot(Vec<LotId>),
    /// Pro-rate the close quantity across all currently open lots.
    ///
    /// This implementation preserves lot-level auditability by allocating the
    /// close across existing lots rather than collapsing them into a single
    /// pooled position.
    AverageCost,
}

/// Errors raised while mutating a holding or building a close plan.
#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum HoldingError {
    /// The symbol string was empty or whitespace.
    #[error("holding symbol cannot be empty")]
    EmptySymbol,
    /// A fill referenced a different symbol from the holding.
    #[error("holding symbol mismatch: expected {expected}, got {actual}")]
    SymbolMismatch {
        /// Symbol stored on the holding.
        expected: String,
        /// Symbol carried by the incoming fill.
        actual: String,
    },
    /// The caller attempted to mix long and short lots in one holding.
    #[error("holding mixes lot sides: expected {expected:?}, got {actual:?}")]
    MixedLotSide {
        /// Side already present in open lots.
        expected: LotSide,
        /// Side requested by the new lot.
        actual: LotSide,
    },
    /// The caller used a fill with the wrong position effect for the operation.
    #[error("holding expected position effect {expected:?}, got {actual:?}")]
    PositionEffectMismatch {
        /// Position effect expected by the holding operation.
        expected: PositionEffect,
        /// Position effect carried by the fill.
        actual: PositionEffect,
    },
    /// The fill side cannot close the currently open lot side.
    #[error("fill side {fill_side:?} does not close {holding_side:?} lots")]
    CloseSideMismatch {
        /// Side of the currently open exposure.
        holding_side: LotSide,
        /// Side of the closing fill.
        fill_side: OrderSide,
    },
    /// A close was requested while the holding had no open lots.
    #[error("holding for {symbol} has no open lots")]
    NoOpenLots {
        /// Symbol of the holding.
        symbol: String,
    },
    /// Requested close quantity exceeded total open quantity.
    #[error("requested close quantity {requested} exceeds open quantity {available}")]
    CloseExceedsOpenQuantity {
        /// Quantity requested by the close fill.
        requested: Decimal,
        /// Quantity currently open across all lots.
        available: Decimal,
    },
    /// The same fill was replayed into the holding twice.
    #[error("fill {} was already applied to this holding", .fill_id.value())]
    DuplicateFillId {
        /// Identifier of the duplicate fill.
        fill_id: FillId,
    },
    /// A new lot reused an identifier that already exists in the holding.
    #[error("lot {} already exists in this holding", .lot_id.value())]
    DuplicateLotId {
        /// Duplicate lot identifier.
        lot_id: LotId,
    },
    /// A `SpecificLot` selection referenced a lot that does not exist here.
    #[error("lot {} does not exist in this holding", .lot_id.value())]
    UnknownLotId {
        /// Unknown lot identifier.
        lot_id: LotId,
    },
    /// A `SpecificLot` selection referenced a lot that is already closed.
    #[error("lot {} is not open", .lot_id.value())]
    LotNotOpen {
        /// Lot identifier that is not currently open.
        lot_id: LotId,
    },
    /// A `SpecificLot` selection listed the same lot more than once.
    #[error("lot {} was selected more than once in SpecificLot", .lot_id.value())]
    DuplicateLotIdSelection {
        /// Duplicate lot identifier.
        lot_id: LotId,
    },
    /// The selected lots in `SpecificLot` did not cover the requested close.
    #[error(
        "specific lot selection covers {available}, below requested close quantity {requested}"
    )]
    SpecificLotQuantityInsufficient {
        /// Quantity requested by the close fill.
        requested: Decimal,
        /// Aggregate quantity covered by the provided lot list.
        available: Decimal,
    },
    /// Wrapped error bubbled up from an individual lot.
    #[error(transparent)]
    Lot(#[from] LotError),
}

/// One realized lot match produced by a holding-level close operation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HoldingCloseMatch {
    /// Lot relieved by this match.
    pub lot_id: LotId,
    /// Side of the relieved lot.
    pub lot_side: LotSide,
    /// Concrete lot-level close event produced by the match.
    pub close: LotClose,
}

/// Full result of applying one close fill to a holding.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HoldingClose {
    /// Symbol of the holding.
    pub symbol: String,
    /// Closing order identifier.
    pub close_order_id: OrderId,
    /// Closing fill identifier.
    pub close_fill_id: FillId,
    /// Total quantity closed by the fill.
    pub closed_qty: Decimal,
    /// Execution price used for the entire close fill.
    pub close_price: Decimal,
    /// Total fees on the close fill before per-lot allocation.
    pub close_fees: Decimal,
    /// Per-lot matches created by this close.
    pub matches: Vec<HoldingCloseMatch>,
    /// Sum of gross realized PnL across every match.
    pub total_realized_gross_pnl: Decimal,
    /// Sum of net realized PnL across every match.
    pub total_realized_net_pnl: Decimal,
    /// Remaining open quantity after the close completed.
    pub remaining_open_qty: Decimal,
}

/// Point-in-time PnL snapshot for one holding at a given mark price.
///
/// `unrealized_net_pnl` subtracts only the still-unrealized portion of
/// open-side fees already paid when the lots were opened. It does not attempt
/// to predict future exit fees because those depend on how the position will be
/// closed later.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HoldingPnl {
    /// Symbol of the holding.
    pub symbol: String,
    /// Current open side of the holding, if any.
    pub side: Option<LotSide>,
    /// Remaining open quantity across all lots.
    pub open_qty: Decimal,
    /// Mark price used for the unrealized snapshot.
    pub mark_price: Decimal,
    /// Sum of realized gross PnL across all lots.
    pub realized_gross_pnl: Decimal,
    /// Sum of realized net PnL across all lots.
    pub realized_net_pnl: Decimal,
    /// Gross PnL of all still-open quantity at the supplied mark.
    pub unrealized_gross_pnl: Decimal,
    /// Portion of already-paid open fees not yet recognized through closes.
    pub unrealized_open_fees: Decimal,
    /// Unrealized PnL net of remaining open fees, but before any unknown future exit fees.
    pub unrealized_net_pnl: Decimal,
}

/// Per-symbol aggregate that owns all lots.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Holding {
    /// Stable identifier of the holding.
    id: HoldingId,
    /// Instrument symbol represented by the holding.
    symbol: String,
    /// Default lot-relief method used by [`Self::close_with_fill`].
    lot_relief_method: LotReliefMethod,
    /// Complete set of lots, including closed historical lots.
    lots: Vec<Lot>,
}

/// Internal plan entry used to separate lot selection from lot mutation.
#[derive(Clone, Debug, PartialEq, Eq)]
struct ClosePlanEntry {
    /// Index of the target lot inside `Holding::lots`.
    lot_index: usize,
    /// Quantity that should be relieved from that lot.
    qty: Decimal,
}

impl Holding {
    /// Creates a new empty holding for one symbol.
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

    /// Returns the holding identifier.
    pub fn id(&self) -> HoldingId {
        self.id
    }

    /// Returns the symbol represented by the holding.
    pub fn symbol(&self) -> &str {
        &self.symbol
    }

    /// Returns the default lot-relief method.
    pub fn lot_relief_method(&self) -> &LotReliefMethod {
        &self.lot_relief_method
    }

    /// Replaces the default lot-relief method used by [`Self::close_with_fill`].
    pub fn set_lot_relief_method(&mut self, lot_relief_method: LotReliefMethod) {
        self.lot_relief_method = lot_relief_method;
    }

    /// Returns all lots, including already closed historical lots.
    pub fn lots(&self) -> &[Lot] {
        &self.lots
    }

    /// Returns the side of the currently open exposure, if any.
    pub fn side(&self) -> Option<LotSide> {
        self.lots.iter().find(|lot| lot.is_open()).map(Lot::side)
    }

    /// Returns the total open quantity across all open lots.
    pub fn open_qty(&self) -> Decimal {
        self.lots
            .iter()
            .fold(Decimal::ZERO, |sum, lot| sum + lot.remaining_qty())
    }

    /// Returns the total historically closed quantity across all lots.
    pub fn closed_qty(&self) -> Decimal {
        self.lots
            .iter()
            .fold(Decimal::ZERO, |sum, lot| sum + lot.closed_qty())
    }

    /// Returns the number of lots that still have open quantity.
    pub fn open_lot_count(&self) -> usize {
        self.lots.iter().filter(|lot| lot.is_open()).count()
    }

    /// Returns `true` when the holding has no open quantity.
    pub fn is_flat(&self) -> bool {
        self.open_qty().is_zero()
    }

    /// Returns cumulative realized gross PnL across all lots.
    pub fn realized_gross_pnl(&self) -> Decimal {
        self.lots
            .iter()
            .fold(Decimal::ZERO, |sum, lot| sum + lot.realized_gross_pnl())
    }

    /// Returns cumulative realized net PnL across all lots.
    pub fn realized_net_pnl(&self) -> Decimal {
        self.lots
            .iter()
            .fold(Decimal::ZERO, |sum, lot| sum + lot.realized_net_pnl())
    }

    /// Returns mark-to-market gross PnL across all still-open lots.
    pub fn unrealized_gross_pnl(&self, mark_price: Decimal) -> Decimal {
        self.lots
            .iter()
            .filter(|lot| lot.is_open())
            .fold(Decimal::ZERO, |sum, lot| {
                let lot_gross_pnl = match lot.side() {
                    LotSide::Long => (mark_price - lot.open_price()) * lot.remaining_qty(),
                    LotSide::Short => (lot.open_price() - mark_price) * lot.remaining_qty(),
                };
                sum + lot_gross_pnl
            })
    }

    /// Returns still-unrealized open fees across all currently open lots.
    pub fn unrealized_open_fees(&self) -> Decimal {
        self.lots
            .iter()
            .filter(|lot| lot.is_open())
            .fold(Decimal::ZERO, |sum, lot| {
                sum + self.remaining_open_fees(lot)
            })
    }

    /// Returns mark-to-market PnL net of the remaining open-side fees.
    ///
    /// Future close fees are intentionally excluded because they are not known
    /// until the eventual exit execution arrives.
    pub fn unrealized_net_pnl(&self, mark_price: Decimal) -> Decimal {
        self.unrealized_gross_pnl(mark_price) - self.unrealized_open_fees()
    }

    /// Returns a point-in-time PnL snapshot for the holding.
    pub fn pnl(&self, mark_price: Decimal) -> HoldingPnl {
        let realized_gross_pnl = self.realized_gross_pnl();
        let realized_net_pnl = self.realized_net_pnl();
        let unrealized_gross_pnl = self.unrealized_gross_pnl(mark_price);
        let unrealized_open_fees = self.unrealized_open_fees();
        let unrealized_net_pnl = unrealized_gross_pnl - unrealized_open_fees;

        HoldingPnl {
            symbol: self.symbol.clone(),
            side: self.side(),
            open_qty: self.open_qty(),
            mark_price,
            realized_gross_pnl,
            realized_net_pnl,
            unrealized_gross_pnl,
            unrealized_open_fees,
            unrealized_net_pnl,
        }
    }

    /// Inserts a pre-built lot into the holding.
    ///
    /// The holding forbids mixed long and short exposure. Closed historical lots
    /// do not determine side; only currently open lots do.
    pub fn open_lot(&mut self, lot: Lot) -> Result<&Lot, HoldingError> {
        if self
            .lots
            .iter()
            .any(|existing_lot| existing_lot.id() == lot.id())
        {
            return Err(HoldingError::DuplicateLotId { lot_id: lot.id() });
        }

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

    /// Converts one opening fill into a new lot and appends it to the holding.
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

    /// Closes quantity using the holding's default lot-relief method.
    pub fn close_with_fill(&mut self, fill: &Fill) -> Result<HoldingClose, HoldingError> {
        let close_plan = self.build_close_plan(fill, &self.lot_relief_method)?;
        self.execute_close_plan(fill, close_plan)
    }

    /// Closes quantity using a method supplied for this call only.
    ///
    /// This is especially useful for one-off `SpecificLot` or `AverageCost`
    /// operations without changing the holding's default policy.
    pub fn close_with_fill_using(
        &mut self,
        fill: &Fill,
        lot_relief_method: &LotReliefMethod,
    ) -> Result<HoldingClose, HoldingError> {
        let close_plan = self.build_close_plan(fill, lot_relief_method)?;
        self.execute_close_plan(fill, close_plan)
    }

    /// Applies a previously computed close plan to the underlying lots.
    ///
    /// The plan contains only lot indices and quantities. This method handles
    /// close-fee allocation, lot mutation, and the final holding-level
    /// aggregation of realized PnL.
    fn execute_close_plan(
        &mut self,
        fill: &Fill,
        close_plan: Vec<ClosePlanEntry>,
    ) -> Result<HoldingClose, HoldingError> {
        let mut remaining_fees = fill.fees();
        let mut matches = Vec::with_capacity(close_plan.len());

        for (plan_index, entry) in close_plan.iter().enumerate() {
            let allocated_close_fees = if plan_index + 1 == close_plan.len() {
                remaining_fees
            } else {
                fill.fees() * entry.qty / fill.qty()
            };

            let lot = &mut self.lots[entry.lot_index];
            let close = lot
                .close(
                    fill.order_id(),
                    fill.id(),
                    fill.executed_at(),
                    entry.qty,
                    fill.price(),
                    allocated_close_fees,
                )?
                .clone();

            matches.push(HoldingCloseMatch {
                lot_id: lot.id(),
                lot_side: lot.side(),
                close,
            });
            remaining_fees -= allocated_close_fees;
        }

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

    /// Builds a close plan without mutating any lots.
    ///
    /// This method validates fill identity and close preconditions first, then
    /// delegates to the selected lot-relief strategy.
    fn build_close_plan(
        &self,
        fill: &Fill,
        lot_relief_method: &LotReliefMethod,
    ) -> Result<Vec<ClosePlanEntry>, HoldingError> {
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

        let open_indices = self.open_lot_indices();
        match lot_relief_method {
            LotReliefMethod::Fifo => Ok(self.build_sequential_plan(open_indices, fill.qty())),
            LotReliefMethod::Lifo => {
                let mut ordered_indices = open_indices;
                ordered_indices.reverse();
                Ok(self.build_sequential_plan(ordered_indices, fill.qty()))
            }
            LotReliefMethod::Hifo => {
                let mut ordered_indices = open_indices;
                ordered_indices.sort_by(|left, right| self.compare_hifo(*left, *right));
                Ok(self.build_sequential_plan(ordered_indices, fill.qty()))
            }
            LotReliefMethod::MaxLoss => {
                let mut ordered_indices = open_indices;
                ordered_indices
                    .sort_by(|left, right| self.compare_estimated_pnl(*left, *right, fill.price()));
                Ok(self.build_sequential_plan(ordered_indices, fill.qty()))
            }
            LotReliefMethod::MaxGain => {
                let mut ordered_indices = open_indices;
                ordered_indices
                    .sort_by(|left, right| self.compare_estimated_pnl(*right, *left, fill.price()));
                Ok(self.build_sequential_plan(ordered_indices, fill.qty()))
            }
            LotReliefMethod::SpecificLot(lot_ids) => {
                self.build_specific_lot_plan(lot_ids, fill.qty())
            }
            LotReliefMethod::AverageCost => {
                Ok(self.build_average_cost_plan(open_indices, fill.qty()))
            }
        }
    }

    /// Validates that an incoming symbol belongs to this holding.
    fn ensure_symbol_matches(&self, symbol: &str) -> Result<(), HoldingError> {
        if self.symbol != symbol {
            return Err(HoldingError::SymbolMismatch {
                expected: self.symbol.clone(),
                actual: symbol.to_owned(),
            });
        }

        Ok(())
    }

    /// Rejects duplicate open or close fill replays.
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

    /// Returns indices of lots that still have open quantity.
    fn open_lot_indices(&self) -> Vec<usize> {
        self.lots
            .iter()
            .enumerate()
            .filter_map(|(index, lot)| lot.is_open().then_some(index))
            .collect()
    }

    /// Builds a simple sequential close plan from a pre-ordered lot list.
    ///
    /// This helper is used by FIFO, LIFO, HIFO, MaxLoss, and MaxGain after they
    /// decide the lot ordering.
    fn build_sequential_plan(
        &self,
        ordered_indices: Vec<usize>,
        requested_close_qty: Decimal,
    ) -> Vec<ClosePlanEntry> {
        let mut remaining_qty = requested_close_qty;
        let mut close_plan = Vec::new();

        for lot_index in ordered_indices {
            if remaining_qty.is_zero() {
                break;
            }

            let close_qty = remaining_qty.min(self.lots[lot_index].remaining_qty());
            if close_qty.is_zero() {
                continue;
            }

            close_plan.push(ClosePlanEntry {
                lot_index,
                qty: close_qty,
            });
            remaining_qty -= close_qty;
        }

        debug_assert!(
            remaining_qty.is_zero(),
            "ordered close plan left unmatched quantity"
        );

        close_plan
    }

    /// Builds an `AverageCost` plan by allocating close quantity pro rata.
    ///
    /// The implementation preserves auditability by still mutating concrete lots.
    /// It does not collapse lots into a single pooled average-cost position.
    fn build_average_cost_plan(
        &self,
        ordered_indices: Vec<usize>,
        requested_close_qty: Decimal,
    ) -> Vec<ClosePlanEntry> {
        let mut remaining_close_qty = requested_close_qty;
        let mut remaining_open_qty = ordered_indices.iter().fold(Decimal::ZERO, |sum, index| {
            sum + self.lots[*index].remaining_qty()
        });
        let mut close_plan = Vec::new();

        for (position, lot_index) in ordered_indices.iter().enumerate() {
            if remaining_close_qty.is_zero() {
                break;
            }

            let lot_qty = self.lots[*lot_index].remaining_qty();
            let close_qty = if position + 1 == ordered_indices.len() {
                remaining_close_qty
            } else {
                remaining_close_qty * lot_qty / remaining_open_qty
            };

            if !close_qty.is_zero() {
                close_plan.push(ClosePlanEntry {
                    lot_index: *lot_index,
                    qty: close_qty,
                });
                remaining_close_qty -= close_qty;
            }
            remaining_open_qty -= lot_qty;
        }

        debug_assert!(
            remaining_close_qty.is_zero(),
            "average cost plan left unmatched quantity"
        );

        close_plan
    }

    /// Builds a plan that follows an explicit lot-id selection from the caller.
    fn build_specific_lot_plan(
        &self,
        lot_ids: &[LotId],
        requested_close_qty: Decimal,
    ) -> Result<Vec<ClosePlanEntry>, HoldingError> {
        let mut seen_lot_ids = HashSet::new();
        let mut remaining_qty = requested_close_qty;
        let mut selected_available = Decimal::ZERO;
        let mut close_plan = Vec::new();

        for lot_id in lot_ids {
            if !seen_lot_ids.insert(*lot_id) {
                return Err(HoldingError::DuplicateLotIdSelection { lot_id: *lot_id });
            }

            let lot_index = self
                .lots
                .iter()
                .position(|lot| lot.id() == *lot_id)
                .ok_or(HoldingError::UnknownLotId { lot_id: *lot_id })?;
            let lot = &self.lots[lot_index];
            if !lot.is_open() {
                return Err(HoldingError::LotNotOpen { lot_id: *lot_id });
            }

            selected_available += lot.remaining_qty();
            if remaining_qty.is_zero() {
                continue;
            }

            let close_qty = remaining_qty.min(lot.remaining_qty());
            if close_qty.is_zero() {
                continue;
            }

            close_plan.push(ClosePlanEntry {
                lot_index,
                qty: close_qty,
            });
            remaining_qty -= close_qty;
        }

        if !remaining_qty.is_zero() {
            return Err(HoldingError::SpecificLotQuantityInsufficient {
                requested: requested_close_qty,
                available: selected_available,
            });
        }

        Ok(close_plan)
    }

    /// Comparator used by `Hifo`.
    ///
    /// The method sorts by fee-adjusted open basis descending, then uses open
    /// timestamp and lot id as deterministic tie-breakers.
    fn compare_hifo(&self, left_index: usize, right_index: usize) -> Ordering {
        self.fee_adjusted_open_basis(&self.lots[right_index])
            .cmp(&self.fee_adjusted_open_basis(&self.lots[left_index]))
            .then_with(|| self.lot_order_tiebreak(left_index, right_index))
    }

    /// Comparator used by `MaxLoss` and `MaxGain`.
    ///
    /// The estimated unit net PnL incorporates close price and remaining
    /// unrecognized open fees. Sorting ascending yields `MaxLoss`; reversing the
    /// argument order yields `MaxGain`.
    fn compare_estimated_pnl(
        &self,
        left_index: usize,
        right_index: usize,
        close_price: Decimal,
    ) -> Ordering {
        self.estimated_unit_net_pnl(&self.lots[left_index], close_price)
            .cmp(&self.estimated_unit_net_pnl(&self.lots[right_index], close_price))
            .then_with(|| self.lot_order_tiebreak(left_index, right_index))
    }

    /// Stable tie-breaker used after strategy-specific comparisons.
    fn lot_order_tiebreak(&self, left_index: usize, right_index: usize) -> Ordering {
        self.lots[left_index]
            .opened_at()
            .cmp(&self.lots[right_index].opened_at())
            .then_with(|| self.lots[left_index].id().cmp(&self.lots[right_index].id()))
    }

    /// Returns fee-adjusted open basis per unit for one lot.
    fn fee_adjusted_open_basis(&self, lot: &Lot) -> Decimal {
        lot.open_price() + self.remaining_open_fees(lot) / lot.remaining_qty()
    }

    /// Estimates unit net PnL at a hypothetical close price.
    ///
    /// This helper is used only for ordering lots during selection. Actual
    /// realized PnL is still computed by [`Lot::close`](crate::order_sys::lot::Lot::close).
    fn estimated_unit_net_pnl(&self, lot: &Lot, close_price: Decimal) -> Decimal {
        let gross_unit_pnl = match lot.side() {
            LotSide::Long => close_price - lot.open_price(),
            LotSide::Short => lot.open_price() - close_price,
        };
        gross_unit_pnl - self.remaining_open_fees(lot) / lot.remaining_qty()
    }

    /// Returns open-side fees that have not yet been realized.
    fn remaining_open_fees(&self, lot: &Lot) -> Decimal {
        lot.open_fees() - lot.realized_open_fees()
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

    fn long_open_fill(fill_id: u64, order_id: u64, day: u32, qty: i64, price: i64) -> Fill {
        Fill::new(
            FillId::new(fill_id),
            OrderId::new(order_id),
            "AAPL",
            OrderSide::Buy,
            PositionEffect::Open,
            ts(day),
            dec(qty, 0),
            dec(price, 0),
            dec(0, 0),
        )
        .unwrap()
    }

    fn short_open_fill(fill_id: u64, order_id: u64, day: u32, qty: i64, price: i64) -> Fill {
        Fill::new(
            FillId::new(fill_id),
            OrderId::new(order_id),
            "AAPL",
            OrderSide::Sell,
            PositionEffect::Open,
            ts(day),
            dec(qty, 0),
            dec(price, 0),
            dec(0, 0),
        )
        .unwrap()
    }

    fn long_close_fill(fill_id: u64, order_id: u64, day: u32, qty: i64, price: i64) -> Fill {
        Fill::new(
            FillId::new(fill_id),
            OrderId::new(order_id),
            "AAPL",
            OrderSide::Sell,
            PositionEffect::Close,
            ts(day),
            dec(qty, 0),
            dec(price, 0),
            dec(0, 0),
        )
        .unwrap()
    }

    fn short_close_fill(fill_id: u64, order_id: u64, day: u32, qty: i64, price: i64) -> Fill {
        Fill::new(
            FillId::new(fill_id),
            OrderId::new(order_id),
            "AAPL",
            OrderSide::Buy,
            PositionEffect::Close,
            ts(day),
            dec(qty, 0),
            dec(price, 0),
            dec(0, 0),
        )
        .unwrap()
    }

    fn long_open_fill_with_fees(
        fill_id: u64,
        order_id: u64,
        day: u32,
        qty: i64,
        price: i64,
        fees: Decimal,
    ) -> Fill {
        Fill::new(
            FillId::new(fill_id),
            OrderId::new(order_id),
            "AAPL",
            OrderSide::Buy,
            PositionEffect::Open,
            ts(day),
            dec(qty, 0),
            dec(price, 0),
            fees,
        )
        .unwrap()
    }

    fn short_open_fill_with_fees(
        fill_id: u64,
        order_id: u64,
        day: u32,
        qty: i64,
        price: i64,
        fees: Decimal,
    ) -> Fill {
        Fill::new(
            FillId::new(fill_id),
            OrderId::new(order_id),
            "AAPL",
            OrderSide::Sell,
            PositionEffect::Open,
            ts(day),
            dec(qty, 0),
            dec(price, 0),
            fees,
        )
        .unwrap()
    }

    fn long_close_fill_with_fees(
        fill_id: u64,
        order_id: u64,
        day: u32,
        qty: i64,
        price: i64,
        fees: Decimal,
    ) -> Fill {
        Fill::new(
            FillId::new(fill_id),
            OrderId::new(order_id),
            "AAPL",
            OrderSide::Sell,
            PositionEffect::Close,
            ts(day),
            dec(qty, 0),
            dec(price, 0),
            fees,
        )
        .unwrap()
    }

    #[test]
    fn holding_pnl_snapshot_combines_realized_and_unrealized_pnl() {
        let mut holding = Holding::new(HoldingId::new(1), "AAPL", LotReliefMethod::Fifo).unwrap();
        let open = long_open_fill_with_fees(1, 100, 24, 10, 100, dec(2, 0));
        let close = long_close_fill_with_fees(2, 101, 25, 4, 110, dec(1, 0));

        holding.open_lot_from_fill(LotId::new(10), &open).unwrap();
        holding.close_with_fill(&close).unwrap();

        let pnl = holding.pnl(dec(105, 0));
        assert_eq!(pnl.symbol, "AAPL");
        assert_eq!(pnl.side, Some(LotSide::Long));
        assert_eq!(pnl.open_qty, dec(6, 0));
        assert_eq!(pnl.mark_price, dec(105, 0));
        assert_eq!(pnl.realized_gross_pnl, dec(40, 0));
        assert_eq!(pnl.realized_net_pnl, dec(382, 1));
        assert_eq!(pnl.unrealized_gross_pnl, dec(30, 0));
        assert_eq!(pnl.unrealized_open_fees, dec(12, 1));
        assert_eq!(pnl.unrealized_net_pnl, dec(288, 1));
        assert_eq!(holding.unrealized_gross_pnl(dec(105, 0)), dec(30, 0));
        assert_eq!(holding.unrealized_net_pnl(dec(105, 0)), dec(288, 1));
    }

    #[test]
    fn short_holding_unrealized_pnl_marks_to_market_correctly() {
        let mut holding = Holding::new(HoldingId::new(1), "AAPL", LotReliefMethod::Fifo).unwrap();
        let open = short_open_fill_with_fees(1, 100, 24, 5, 100, dec(15, 1));

        holding.open_lot_from_fill(LotId::new(10), &open).unwrap();

        let pnl = holding.pnl(dec(90, 0));
        assert_eq!(pnl.side, Some(LotSide::Short));
        assert_eq!(pnl.open_qty, dec(5, 0));
        assert_eq!(pnl.realized_net_pnl, Decimal::ZERO);
        assert_eq!(pnl.unrealized_gross_pnl, dec(50, 0));
        assert_eq!(pnl.unrealized_open_fees, dec(15, 1));
        assert_eq!(pnl.unrealized_net_pnl, dec(485, 1));
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
    fn hifo_close_matches_highest_cost_lots_first() {
        let mut holding = Holding::new(HoldingId::new(8), "AAPL", LotReliefMethod::Hifo).unwrap();

        holding
            .open_lot_from_fill(LotId::new(80), &long_open_fill(80, 800, 24, 1, 100))
            .unwrap();
        holding
            .open_lot_from_fill(LotId::new(81), &long_open_fill(81, 801, 25, 1, 110))
            .unwrap();
        holding
            .open_lot_from_fill(LotId::new(82), &long_open_fill(82, 802, 26, 1, 105))
            .unwrap();

        let result = holding
            .close_with_fill(&long_close_fill(83, 803, 27, 2, 120))
            .unwrap();

        assert_eq!(result.matches.len(), 2);
        assert_eq!(result.matches[0].lot_id, LotId::new(81));
        assert_eq!(result.matches[1].lot_id, LotId::new(82));
        assert_eq!(result.total_realized_gross_pnl, dec(25, 0));
    }

    #[test]
    fn max_loss_on_short_matches_worst_losing_lots_first() {
        let mut holding =
            Holding::new(HoldingId::new(9), "AAPL", LotReliefMethod::MaxLoss).unwrap();

        holding
            .open_lot_from_fill(LotId::new(90), &short_open_fill(90, 900, 24, 1, 100))
            .unwrap();
        holding
            .open_lot_from_fill(LotId::new(91), &short_open_fill(91, 901, 25, 1, 110))
            .unwrap();

        let result = holding
            .close_with_fill(&short_close_fill(92, 902, 26, 1, 105))
            .unwrap();

        assert_eq!(result.matches.len(), 1);
        assert_eq!(result.matches[0].lot_id, LotId::new(90));
        assert_eq!(result.total_realized_gross_pnl, dec(-5, 0));
    }

    #[test]
    fn max_gain_on_short_matches_best_winning_lots_first() {
        let mut holding =
            Holding::new(HoldingId::new(10), "AAPL", LotReliefMethod::MaxGain).unwrap();

        holding
            .open_lot_from_fill(LotId::new(100), &short_open_fill(100, 1000, 24, 1, 100))
            .unwrap();
        holding
            .open_lot_from_fill(LotId::new(101), &short_open_fill(101, 1001, 25, 1, 110))
            .unwrap();

        let result = holding
            .close_with_fill(&short_close_fill(102, 1002, 26, 1, 105))
            .unwrap();

        assert_eq!(result.matches.len(), 1);
        assert_eq!(result.matches[0].lot_id, LotId::new(101));
        assert_eq!(result.total_realized_gross_pnl, dec(5, 0));
    }

    #[test]
    fn specific_lot_matches_explicit_ids_in_requested_order() {
        let mut holding = Holding::new(HoldingId::new(11), "AAPL", LotReliefMethod::Fifo).unwrap();

        holding
            .open_lot_from_fill(LotId::new(110), &long_open_fill(110, 1100, 24, 3, 100))
            .unwrap();
        holding
            .open_lot_from_fill(LotId::new(111), &long_open_fill(111, 1101, 25, 3, 105))
            .unwrap();
        holding
            .open_lot_from_fill(LotId::new(112), &long_open_fill(112, 1102, 26, 3, 110))
            .unwrap();

        let close = long_close_fill(113, 1103, 27, 5, 120);
        let result = holding
            .close_with_fill_using(
                &close,
                &LotReliefMethod::SpecificLot(vec![LotId::new(111), LotId::new(110)]),
            )
            .unwrap();

        assert_eq!(result.matches.len(), 2);
        assert_eq!(result.matches[0].lot_id, LotId::new(111));
        assert_eq!(result.matches[0].close.qty, dec(3, 0));
        assert_eq!(result.matches[1].lot_id, LotId::new(110));
        assert_eq!(result.matches[1].close.qty, dec(2, 0));
    }

    #[test]
    fn specific_lot_rejects_insufficient_selected_quantity() {
        let mut holding = Holding::new(HoldingId::new(12), "AAPL", LotReliefMethod::Fifo).unwrap();

        holding
            .open_lot_from_fill(LotId::new(120), &long_open_fill(120, 1200, 24, 3, 100))
            .unwrap();
        holding
            .open_lot_from_fill(LotId::new(121), &long_open_fill(121, 1201, 25, 3, 105))
            .unwrap();

        let err = holding
            .close_with_fill_using(
                &long_close_fill(122, 1202, 26, 5, 120),
                &LotReliefMethod::SpecificLot(vec![LotId::new(121)]),
            )
            .unwrap_err();

        assert_eq!(
            err,
            HoldingError::SpecificLotQuantityInsufficient {
                requested: dec(5, 0),
                available: dec(3, 0),
            }
        );
    }

    #[test]
    fn average_cost_allocates_pro_rata_across_open_lots() {
        let mut holding =
            Holding::new(HoldingId::new(13), "AAPL", LotReliefMethod::AverageCost).unwrap();

        holding
            .open_lot_from_fill(LotId::new(130), &long_open_fill(130, 1300, 24, 4, 100))
            .unwrap();
        holding
            .open_lot_from_fill(LotId::new(131), &long_open_fill(131, 1301, 25, 6, 110))
            .unwrap();

        let result = holding
            .close_with_fill(&long_close_fill(132, 1302, 26, 5, 120))
            .unwrap();

        assert_eq!(result.matches.len(), 2);
        assert_eq!(result.matches[0].lot_id, LotId::new(130));
        assert_eq!(result.matches[0].close.qty, dec(2, 0));
        assert_eq!(result.matches[1].lot_id, LotId::new(131));
        assert_eq!(result.matches[1].close.qty, dec(3, 0));
        assert_eq!(result.total_realized_gross_pnl, dec(70, 0));
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

    #[test]
    fn holding_rejects_duplicate_lot_id_reuse() {
        let mut holding = Holding::new(HoldingId::new(14), "AAPL", LotReliefMethod::Fifo).unwrap();

        let open_one = long_open_fill(140, 1400, 24, 1, 100);
        let open_two = long_open_fill(141, 1401, 25, 1, 101);

        holding
            .open_lot_from_fill(LotId::new(140), &open_one)
            .unwrap();

        let err = holding
            .open_lot_from_fill(LotId::new(140), &open_two)
            .unwrap_err();
        assert_eq!(
            err,
            HoldingError::DuplicateLotId {
                lot_id: LotId::new(140),
            }
        );
    }

    #[test]
    fn single_close_fill_can_relieve_multiple_lots() {
        let mut holding = Holding::new(HoldingId::new(15), "AAPL", LotReliefMethod::Fifo).unwrap();

        holding
            .open_lot_from_fill(LotId::new(150), &long_open_fill(150, 1500, 24, 1, 100))
            .unwrap();
        holding
            .open_lot_from_fill(LotId::new(151), &long_open_fill(151, 1501, 25, 1, 101))
            .unwrap();

        let result = holding
            .close_with_fill(&long_close_fill(152, 1502, 26, 2, 110))
            .unwrap();

        assert_eq!(result.matches.len(), 2);
        assert_eq!(result.matches[0].close.close_fill_id, FillId::new(152));
        assert_eq!(result.matches[1].close.close_fill_id, FillId::new(152));
    }
}
