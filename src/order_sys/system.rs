//! High-level order-system facade.
//!
//! The lower-level types in [`crate::order_sys`] deliberately separate order
//! intent, execution facts, holdings, and execution reports. [`OrderSystem`]
//! stitches those pieces together into one API surface that can:
//!
//! - submit new orders and emit `PendingNew` execution reports,
//! - infer close-side orders from current holdings,
//! - reserve close quantity so overlapping close orders are rejected early,
//! - apply fills atomically to both the order state machine and symbol holding,
//! - keep a complete in-memory audit trail of fills and execution reports.
//!
//! This facade is not a matching engine. It expects fills to arrive from an
//! external simulator, exchange adapter, or broker connector.

use std::collections::{BTreeMap, HashSet};

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;

use crate::order_sys::execution_report::ExecutionReport;
use crate::order_sys::fill::Fill;
use crate::order_sys::holding::{Holding, HoldingClose, HoldingError, LotReliefMethod};
use crate::order_sys::lot::LotSide;
use crate::order_sys::order::{
    Order, OrderError, OrderSide, OrderStatus, OrderType, PositionEffect, TimeInForce,
};
use crate::order_sys::{ExecutionReportId, FillId, HoldingId, LotId, OrderId};

/// Input DTO for generic order submission.
///
/// The `lot_relief_method` field is only used for close orders. When present,
/// the override is remembered on the submitted order and applied later when a
/// close fill arrives for that order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SubmitOrderRequest {
    /// Instrument symbol, for example `AAPL`.
    pub symbol: String,
    /// Buy or sell side.
    pub side: OrderSide,
    /// Whether the order opens or closes exposure.
    pub position_effect: PositionEffect,
    /// Requested execution style.
    pub order_type: OrderType,
    /// Requested time-in-force.
    pub time_in_force: TimeInForce,
    /// UTC submission timestamp.
    pub submitted_at: DateTime<Utc>,
    /// Requested quantity.
    pub qty: Decimal,
    /// Optional limit price.
    pub limit_price: Option<Decimal>,
    /// Optional stop price.
    pub stop_price: Option<Decimal>,
    /// Optional one-off close matching policy.
    pub lot_relief_method: Option<LotReliefMethod>,
    /// Whether opposite exposure should be auto-reversed instead of rejected.
    pub allow_reversal: bool,
}

impl SubmitOrderRequest {
    /// Creates a generic order-submission request.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        symbol: impl Into<String>,
        side: OrderSide,
        position_effect: PositionEffect,
        order_type: OrderType,
        time_in_force: TimeInForce,
        submitted_at: DateTime<Utc>,
        qty: Decimal,
        limit_price: Option<Decimal>,
        stop_price: Option<Decimal>,
    ) -> Self {
        Self {
            symbol: symbol.into(),
            side,
            position_effect,
            order_type,
            time_in_force,
            submitted_at,
            qty,
            limit_price,
            stop_price,
            lot_relief_method: None,
            allow_reversal: false,
        }
    }

    /// Attaches a close-specific lot-relief override to the request.
    pub fn with_lot_relief_method(mut self, lot_relief_method: LotReliefMethod) -> Self {
        self.lot_relief_method = Some(lot_relief_method);
        self
    }

    /// Enables or disables automatic reversal for opposite exposure.
    pub fn with_allow_reversal(mut self, allow_reversal: bool) -> Self {
        self.allow_reversal = allow_reversal;
        self
    }
}

/// Input DTO for a close order whose side should be inferred from holdings.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SubmitCloseOrderRequest {
    /// Instrument symbol whose position should be reduced.
    pub symbol: String,
    /// UTC submission timestamp.
    pub submitted_at: DateTime<Utc>,
    /// Quantity to close.
    pub qty: Decimal,
    /// Requested execution style.
    pub order_type: OrderType,
    /// Requested time-in-force.
    pub time_in_force: TimeInForce,
    /// Optional limit price.
    pub limit_price: Option<Decimal>,
    /// Optional stop price.
    pub stop_price: Option<Decimal>,
    /// Optional one-off lot-relief override for this close order.
    pub lot_relief_method: Option<LotReliefMethod>,
}

impl SubmitCloseOrderRequest {
    /// Creates a close-order request with inferred side.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        symbol: impl Into<String>,
        submitted_at: DateTime<Utc>,
        qty: Decimal,
        order_type: OrderType,
        time_in_force: TimeInForce,
        limit_price: Option<Decimal>,
        stop_price: Option<Decimal>,
    ) -> Self {
        Self {
            symbol: symbol.into(),
            submitted_at,
            qty,
            order_type,
            time_in_force,
            limit_price,
            stop_price,
            lot_relief_method: None,
        }
    }

    /// Attaches a close-specific lot-relief override to the request.
    pub fn with_lot_relief_method(mut self, lot_relief_method: LotReliefMethod) -> Self {
        self.lot_relief_method = Some(lot_relief_method);
        self
    }
}

/// Input DTO for flattening the unreserved open quantity of one symbol.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FlattenPositionRequest {
    /// Instrument symbol to flatten.
    pub symbol: String,
    /// UTC submission timestamp.
    pub submitted_at: DateTime<Utc>,
    /// Requested execution style.
    pub order_type: OrderType,
    /// Requested time-in-force.
    pub time_in_force: TimeInForce,
    /// Optional limit price.
    pub limit_price: Option<Decimal>,
    /// Optional stop price.
    pub stop_price: Option<Decimal>,
    /// Optional one-off lot-relief override for the close order.
    pub lot_relief_method: Option<LotReliefMethod>,
}

impl FlattenPositionRequest {
    /// Creates a request that closes every currently unreserved unit.
    pub fn new(
        symbol: impl Into<String>,
        submitted_at: DateTime<Utc>,
        order_type: OrderType,
        time_in_force: TimeInForce,
    ) -> Self {
        Self {
            symbol: symbol.into(),
            submitted_at,
            order_type,
            time_in_force,
            limit_price: None,
            stop_price: None,
            lot_relief_method: None,
        }
    }

    /// Replaces the limit price on the request.
    pub fn with_limit_price(mut self, limit_price: Decimal) -> Self {
        self.limit_price = Some(limit_price);
        self
    }

    /// Replaces the stop price on the request.
    pub fn with_stop_price(mut self, stop_price: Decimal) -> Self {
        self.stop_price = Some(stop_price);
        self
    }

    /// Attaches a close-specific lot-relief override to the request.
    pub fn with_lot_relief_method(mut self, lot_relief_method: LotReliefMethod) -> Self {
        self.lot_relief_method = Some(lot_relief_method);
        self
    }
}

/// Return payload produced when an order is submitted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SubmittedOrder {
    /// Immutable snapshot of the submitted order after insertion.
    pub order: Order,
    /// `PendingNew` execution report emitted for the submission.
    pub report: ExecutionReport,
    /// Auto-generated close order that must complete before this order can open
    /// opposite-side exposure, when reversal was requested.
    pub auto_close_order: Option<Box<SubmittedOrder>>,
}

/// Return payload produced when a fill is applied through the facade.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppliedFill {
    /// Immutable snapshot of the order after the fill was applied.
    pub order: Order,
    /// `Trade` execution report emitted for the fill.
    pub report: ExecutionReport,
    /// Newly created lot identifier for an opening fill, if any.
    pub opened_lot_id: Option<LotId>,
    /// Holding-level close summary for a closing fill, if any.
    pub holding_close: Option<HoldingClose>,
}

/// Errors raised by the high-level order-system facade.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OrderSystemError {
    /// Unknown order identifier.
    UnknownOrderId {
        /// Order identifier supplied by the caller.
        order_id: OrderId,
    },
    /// The same fill identifier was applied to the system twice.
    DuplicateFillId {
        /// Duplicate fill identifier.
        fill_id: FillId,
    },
    /// There is no open position for the requested symbol.
    NoOpenPosition {
        /// Symbol requested by the caller.
        symbol: String,
    },
    /// An opening order tried to add exposure on the opposite side.
    OpenSideMismatch {
        /// Symbol being traded.
        symbol: String,
        /// Side requested by the order.
        requested: OrderSide,
        /// Side already open in the holding.
        holding_side: LotSide,
    },
    /// A live opening order already reserves the opposite opening side.
    LiveOpenOrderSideConflict {
        /// Symbol being traded.
        symbol: String,
        /// Side requested by the new order.
        requested: OrderSide,
        /// Side already reserved by another live open order.
        live_order_side: OrderSide,
    },
    /// A contingent reversal open order cannot accept fills before its
    /// prerequisite close order completes.
    OrderNotActive {
        /// Inactive order identifier.
        order_id: OrderId,
        /// Close order that must complete first.
        waiting_on_order_id: OrderId,
    },
    /// Automatic reversal is blocked while other live open orders still exist
    /// for the symbol.
    ReversalBlockedByLiveOpenOrders {
        /// Symbol requested by the caller.
        symbol: String,
    },
    /// A closing order side did not match the currently open position side.
    CloseSideMismatch {
        /// Symbol being traded.
        symbol: String,
        /// Side requested by the order.
        requested: OrderSide,
        /// Side currently open in the holding.
        holding_side: LotSide,
    },
    /// The requested close quantity exceeded unreserved closeable quantity.
    CloseQuantityExceedsAvailable {
        /// Symbol being traded.
        symbol: String,
        /// Quantity requested by the new close order.
        requested: Decimal,
        /// Quantity currently available after subtracting working close orders.
        available: Decimal,
    },
    /// The symbol has an open position, but all of it is already reserved.
    NoClosableQuantity {
        /// Symbol requested by the caller.
        symbol: String,
    },
    /// Wrapped order-level error.
    Order(OrderError),
    /// Wrapped holding-level error.
    Holding(HoldingError),
}

impl std::fmt::Display for OrderSystemError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownOrderId { order_id } => {
                write!(f, "order {} does not exist", order_id.value())
            }
            Self::DuplicateFillId { fill_id } => {
                write!(
                    f,
                    "fill {} was already applied to the order system",
                    fill_id.value()
                )
            }
            Self::NoOpenPosition { symbol } => write!(f, "symbol {symbol} has no open position"),
            Self::OpenSideMismatch {
                symbol,
                requested,
                holding_side,
            } => write!(
                f,
                "cannot open {:?} exposure for {symbol} while {:?} exposure is already open",
                requested, holding_side
            ),
            Self::LiveOpenOrderSideConflict {
                symbol,
                requested,
                live_order_side,
            } => write!(
                f,
                "cannot open {:?} exposure for {symbol} while a live open order already reserves {:?} exposure",
                requested, live_order_side
            ),
            Self::OrderNotActive {
                order_id,
                waiting_on_order_id,
            } => write!(
                f,
                "order {} is not active yet; it is waiting on order {} to complete",
                order_id.value(),
                waiting_on_order_id.value()
            ),
            Self::ReversalBlockedByLiveOpenOrders { symbol } => write!(
                f,
                "automatic reversal for {symbol} is blocked while other live open orders exist"
            ),
            Self::CloseSideMismatch {
                symbol,
                requested,
                holding_side,
            } => write!(
                f,
                "close order side {:?} does not match {:?} exposure in {symbol}",
                requested, holding_side
            ),
            Self::CloseQuantityExceedsAvailable {
                symbol,
                requested,
                available,
            } => write!(
                f,
                "close quantity {requested} exceeds currently available quantity {available} for {symbol}"
            ),
            Self::NoClosableQuantity { symbol } => {
                write!(
                    f,
                    "symbol {symbol} has no unreserved quantity left to close"
                )
            }
            Self::Order(err) => err.fmt(f),
            Self::Holding(err) => err.fmt(f),
        }
    }
}

impl std::error::Error for OrderSystemError {}

impl From<OrderError> for OrderSystemError {
    fn from(value: OrderError) -> Self {
        Self::Order(value)
    }
}

impl From<HoldingError> for OrderSystemError {
    fn from(value: HoldingError) -> Self {
        Self::Holding(value)
    }
}

/// Internal tracked order entry that remembers per-order close overrides.
#[derive(Clone, Debug, PartialEq, Eq)]
struct TrackedOrder {
    order: Order,
    close_lot_relief_method: Option<LotReliefMethod>,
    contingent_on_order_id: Option<OrderId>,
}

/// In-memory facade that wires order submission, fills, holdings, and reports.
#[derive(Clone, Debug)]
pub struct OrderSystem {
    /// Default lot-relief policy assigned to newly created holdings.
    default_lot_relief_method: LotReliefMethod,
    /// Sequence used for internally allocated order identifiers.
    next_order_id: u64,
    /// Sequence used for internally allocated holding identifiers.
    next_holding_id: u64,
    /// Sequence used for internally allocated lot identifiers.
    next_lot_id: u64,
    /// Sequence used for internally allocated execution-report identifiers.
    next_execution_report_id: u64,
    /// All tracked orders keyed by order identifier.
    orders: BTreeMap<OrderId, TrackedOrder>,
    /// Holdings keyed by symbol.
    holdings: BTreeMap<String, Holding>,
    /// Immutable fill facts keyed by fill identifier.
    fills: BTreeMap<FillId, Fill>,
    /// Append-only execution-report history.
    execution_reports: Vec<ExecutionReport>,
}

impl OrderSystem {
    /// Creates an empty order system.
    pub fn new(default_lot_relief_method: LotReliefMethod) -> Self {
        Self {
            default_lot_relief_method,
            next_order_id: 1,
            next_holding_id: 1,
            next_lot_id: 1,
            next_execution_report_id: 1,
            orders: BTreeMap::new(),
            holdings: BTreeMap::new(),
            fills: BTreeMap::new(),
            execution_reports: Vec::new(),
        }
    }

    /// Returns the default lot-relief policy for newly created holdings.
    pub fn default_lot_relief_method(&self) -> &LotReliefMethod {
        &self.default_lot_relief_method
    }

    /// Returns one tracked order by identifier.
    pub fn order(&self, order_id: OrderId) -> Option<&Order> {
        self.orders
            .get(&order_id)
            .map(|tracked_order| &tracked_order.order)
    }

    /// Returns an iterator over every tracked order.
    pub fn orders(&self) -> impl Iterator<Item = &Order> {
        self.orders
            .values()
            .map(|tracked_order| &tracked_order.order)
    }

    /// Returns one holding by symbol.
    pub fn holding(&self, symbol: &str) -> Option<&Holding> {
        self.holdings.get(symbol)
    }

    /// Returns an iterator over every holding.
    pub fn holdings(&self) -> impl Iterator<Item = &Holding> {
        self.holdings.values()
    }

    /// Returns one fill by identifier.
    pub fn fill(&self, fill_id: FillId) -> Option<&Fill> {
        self.fills.get(&fill_id)
    }

    /// Returns an iterator over every applied fill.
    pub fn fills(&self) -> impl Iterator<Item = &Fill> {
        self.fills.values()
    }

    /// Returns the append-only execution report history.
    pub fn execution_reports(&self) -> &[ExecutionReport] {
        &self.execution_reports
    }

    /// Returns closeable quantity after subtracting working close-order leaves.
    pub fn available_close_qty(&self, symbol: &str) -> Result<Decimal, OrderSystemError> {
        self.ensure_symbol_not_blank(symbol)?;

        let holding =
            self.holdings
                .get(symbol)
                .ok_or_else(|| OrderSystemError::NoOpenPosition {
                    symbol: symbol.to_owned(),
                })?;
        let open_qty = holding.open_qty();
        if open_qty.is_zero() {
            return Err(OrderSystemError::NoOpenPosition {
                symbol: symbol.to_owned(),
            });
        }

        let available = open_qty - self.working_close_qty(symbol);
        if available <= Decimal::ZERO {
            return Ok(Decimal::ZERO);
        }

        Ok(available)
    }

    /// Submits a generic order and emits a `PendingNew` execution report.
    ///
    /// Close-order requests are validated against current holdings and already
    /// working close orders so impossible over-close requests are rejected
    /// before they become live.
    pub fn submit_order(
        &mut self,
        request: SubmitOrderRequest,
    ) -> Result<SubmittedOrder, OrderSystemError> {
        if let Some(reversal_submission) = self.try_submit_reversal(&request)? {
            return Ok(reversal_submission);
        }

        self.validate_submission(&request)?;
        self.submit_single_order(request, None)
    }

    /// Submits a close order whose side is inferred from the current holding.
    pub fn submit_close_order(
        &mut self,
        request: SubmitCloseOrderRequest,
    ) -> Result<SubmittedOrder, OrderSystemError> {
        let side = self.infer_close_side(&request.symbol)?;
        self.submit_order(
            SubmitOrderRequest::new(
                request.symbol,
                side,
                PositionEffect::Close,
                request.order_type,
                request.time_in_force,
                request.submitted_at,
                request.qty,
                request.limit_price,
                request.stop_price,
            )
            .with_lot_relief_method_option(request.lot_relief_method),
        )
    }

    /// Submits a close order for every currently unreserved unit of a symbol.
    pub fn flatten_position(
        &mut self,
        request: FlattenPositionRequest,
    ) -> Result<SubmittedOrder, OrderSystemError> {
        let side = self.infer_close_side(&request.symbol)?;
        let qty = self.available_close_qty(&request.symbol)?;
        if qty.is_zero() {
            return Err(OrderSystemError::NoClosableQuantity {
                symbol: request.symbol,
            });
        }

        self.submit_order(
            SubmitOrderRequest::new(
                request.symbol,
                side,
                PositionEffect::Close,
                request.order_type,
                request.time_in_force,
                request.submitted_at,
                qty,
                request.limit_price,
                request.stop_price,
            )
            .with_lot_relief_method_option(request.lot_relief_method),
        )
    }

    /// Marks a submitted order as acknowledged and emits a `New` report.
    pub fn acknowledge_order(
        &mut self,
        order_id: OrderId,
        acknowledged_at: DateTime<Utc>,
    ) -> Result<ExecutionReport, OrderSystemError> {
        let report_id = self.allocate_execution_report_id();
        let order_snapshot = {
            let tracked_order = self
                .orders
                .get_mut(&order_id)
                .ok_or(OrderSystemError::UnknownOrderId { order_id })?;
            if let Some(waiting_on_order_id) = tracked_order.contingent_on_order_id {
                return Err(OrderSystemError::OrderNotActive {
                    order_id,
                    waiting_on_order_id,
                });
            }
            tracked_order.order.acknowledge(acknowledged_at)?;
            tracked_order.order.clone()
        };

        let report = ExecutionReport::acknowledged(report_id, &order_snapshot, acknowledged_at);
        self.execution_reports.push(report.clone());
        Ok(report)
    }

    /// Cancels a working order and emits a `Canceled` report.
    pub fn cancel_order(
        &mut self,
        order_id: OrderId,
        canceled_at: DateTime<Utc>,
    ) -> Result<ExecutionReport, OrderSystemError> {
        let report_id = self.allocate_execution_report_id();
        let order_snapshot = {
            let tracked_order = self
                .orders
                .get_mut(&order_id)
                .ok_or(OrderSystemError::UnknownOrderId { order_id })?;
            tracked_order.order.cancel(canceled_at)?;
            tracked_order.order.clone()
        };

        let report = ExecutionReport::canceled(report_id, &order_snapshot, canceled_at, None);
        self.execution_reports.push(report.clone());
        self.cancel_contingent_orders(order_id, canceled_at);
        Ok(report)
    }

    /// Rejects a submitted order and emits a `Rejected` report.
    pub fn reject_order(
        &mut self,
        order_id: OrderId,
        rejected_at: DateTime<Utc>,
        reason: impl Into<String>,
    ) -> Result<ExecutionReport, OrderSystemError> {
        let reason = reason.into();
        let report_id = self.allocate_execution_report_id();
        let order_snapshot = {
            let tracked_order = self
                .orders
                .get_mut(&order_id)
                .ok_or(OrderSystemError::UnknownOrderId { order_id })?;
            tracked_order.order.reject(rejected_at, reason.clone())?;
            tracked_order.order.clone()
        };

        let report = ExecutionReport::rejected(report_id, &order_snapshot, rejected_at, reason);
        self.execution_reports.push(report.clone());
        self.reject_contingent_orders(order_id, rejected_at, "parent reversal close order failed");
        Ok(report)
    }

    /// Applies one fill to the tracked order and the corresponding holding.
    ///
    /// The method first validates the fill against clones of the order and
    /// holding, then commits the updated snapshots together. This keeps the
    /// facade from mutating one aggregate successfully and failing halfway
    /// through another.
    pub fn apply_fill(&mut self, fill: Fill) -> Result<AppliedFill, OrderSystemError> {
        if self.fills.contains_key(&fill.id()) {
            return Err(OrderSystemError::DuplicateFillId { fill_id: fill.id() });
        }

        let tracked_order =
            self.orders
                .get(&fill.order_id())
                .cloned()
                .ok_or(OrderSystemError::UnknownOrderId {
                    order_id: fill.order_id(),
                })?;
        if let Some(waiting_on_order_id) = tracked_order.contingent_on_order_id {
            return Err(OrderSystemError::OrderNotActive {
                order_id: fill.order_id(),
                waiting_on_order_id,
            });
        }

        let mut order_snapshot = tracked_order.order.clone();
        order_snapshot.record_fill(&fill)?;

        let mut holding_close = None;
        let mut opened_lot_id = None;
        let mut holding_snapshot = self.holdings.get(fill.symbol()).cloned();
        let mut next_close_lot_relief_method = tracked_order.close_lot_relief_method.clone();

        match fill.position_effect() {
            PositionEffect::Open => {
                if holding_snapshot.is_none() {
                    holding_snapshot = Some(Holding::new(
                        self.allocate_holding_id(),
                        fill.symbol(),
                        self.default_lot_relief_method.clone(),
                    )?);
                }

                let lot_id = self.allocate_lot_id();
                holding_snapshot
                    .as_mut()
                    .expect("holding snapshot must exist for open fill")
                    .open_lot_from_fill(lot_id, &fill)?;
                opened_lot_id = Some(lot_id);
            }
            PositionEffect::Close => {
                let holding =
                    holding_snapshot
                        .as_mut()
                        .ok_or_else(|| OrderSystemError::NoOpenPosition {
                            symbol: fill.symbol().to_owned(),
                        })?;

                let close = if let Some(lot_relief_method) =
                    tracked_order.close_lot_relief_method.as_ref()
                {
                    holding.close_with_fill_using(&fill, lot_relief_method)?
                } else {
                    holding.close_with_fill(&fill)?
                };
                self.advance_close_lot_relief_method(
                    &order_snapshot,
                    holding,
                    &mut next_close_lot_relief_method,
                );
                holding_close = Some(close);
            }
        }

        let report =
            ExecutionReport::trade(self.allocate_execution_report_id(), &order_snapshot, &fill);

        self.orders
            .get_mut(&fill.order_id())
            .expect("tracked order must still exist at commit time")
            .order = order_snapshot.clone();
        self.orders
            .get_mut(&fill.order_id())
            .expect("tracked order must still exist at commit time")
            .close_lot_relief_method = next_close_lot_relief_method;
        self.holdings.insert(
            fill.symbol().to_owned(),
            holding_snapshot.expect("holding snapshot must exist after fill application"),
        );
        self.fills.insert(fill.id(), fill.clone());
        self.execution_reports.push(report.clone());
        if order_snapshot.status() == OrderStatus::Filled {
            self.activate_contingent_orders(fill.order_id());
        }

        Ok(AppliedFill {
            order: order_snapshot,
            report,
            opened_lot_id,
            holding_close,
        })
    }

    /// Submits one already-validated order and emits its `PendingNew` report.
    fn submit_single_order(
        &mut self,
        request: SubmitOrderRequest,
        contingent_on_order_id: Option<OrderId>,
    ) -> Result<SubmittedOrder, OrderSystemError> {
        let order_id = self.allocate_order_id();
        let order = Order::new(
            order_id,
            request.symbol,
            request.side,
            request.position_effect,
            request.order_type,
            request.time_in_force,
            request.submitted_at,
            request.qty,
            request.limit_price,
            request.stop_price,
        )?;
        let report = ExecutionReport::pending_new(self.allocate_execution_report_id(), &order);

        self.orders.insert(
            order_id,
            TrackedOrder {
                order: order.clone(),
                close_lot_relief_method: request.lot_relief_method,
                contingent_on_order_id,
            },
        );
        self.execution_reports.push(report.clone());

        Ok(SubmittedOrder {
            order,
            report,
            auto_close_order: None,
        })
    }

    /// Builds a two-leg reversal submission when the request opens exposure on
    /// the opposite side of the currently open holding.
    fn try_submit_reversal(
        &mut self,
        request: &SubmitOrderRequest,
    ) -> Result<Option<SubmittedOrder>, OrderSystemError> {
        if request.position_effect != PositionEffect::Open || !request.allow_reversal {
            return Ok(None);
        }
        self.ensure_symbol_not_blank(&request.symbol)?;

        let Some(holding) = self.holdings.get(request.symbol.as_str()) else {
            return Ok(None);
        };
        let Some(holding_side) = holding.side() else {
            return Ok(None);
        };
        if holding_side == request.side.to_open_lot_side() {
            return Ok(None);
        }
        if self.has_live_open_orders(request.symbol.as_str()) {
            return Err(OrderSystemError::ReversalBlockedByLiveOpenOrders {
                symbol: request.symbol.clone(),
            });
        }

        let open_qty = holding.open_qty();
        let available_close_qty = self.available_close_qty(&request.symbol)?;
        if available_close_qty != open_qty {
            return Err(OrderSystemError::CloseQuantityExceedsAvailable {
                symbol: request.symbol.clone(),
                requested: open_qty,
                available: available_close_qty,
            });
        }

        let close_request = SubmitOrderRequest::new(
            request.symbol.clone(),
            request.side,
            PositionEffect::Close,
            request.order_type,
            request.time_in_force,
            request.submitted_at,
            open_qty,
            request.limit_price,
            request.stop_price,
        );
        self.validate_close_submission(&close_request)?;
        let close_submission = self.submit_single_order(close_request, None)?;

        let open_request = SubmitOrderRequest::new(
            request.symbol.clone(),
            request.side,
            PositionEffect::Open,
            request.order_type,
            request.time_in_force,
            request.submitted_at,
            request.qty,
            request.limit_price,
            request.stop_price,
        );
        let mut open_submission =
            self.submit_single_order(open_request, Some(close_submission.order.id()))?;
        open_submission.auto_close_order = Some(Box::new(close_submission));
        Ok(Some(open_submission))
    }

    /// Returns the total leaves quantity of all live close orders for one symbol.
    fn working_close_qty(&self, symbol: &str) -> Decimal {
        self.orders
            .values()
            .filter(|tracked_order| {
                tracked_order.order.symbol() == symbol
                    && tracked_order.order.position_effect() == PositionEffect::Close
                    && matches!(
                        tracked_order.order.status(),
                        OrderStatus::Pending | OrderStatus::Working | OrderStatus::PartiallyFilled
                    )
            })
            .fold(Decimal::ZERO, |sum, tracked_order| {
                sum + tracked_order.order.leaves_qty()
            })
    }

    /// Returns `true` when any live open order already exists for the symbol.
    fn has_live_open_orders(&self, symbol: &str) -> bool {
        self.orders.values().any(|tracked_order| {
            tracked_order.order.symbol() == symbol
                && tracked_order.order.position_effect() == PositionEffect::Open
                && matches!(
                    tracked_order.order.status(),
                    OrderStatus::Pending | OrderStatus::Working | OrderStatus::PartiallyFilled
                )
        })
    }

    /// Validates a submission request against current holdings and reservations.
    fn validate_submission(&self, request: &SubmitOrderRequest) -> Result<(), OrderSystemError> {
        self.ensure_symbol_not_blank(&request.symbol)?;

        match request.position_effect {
            PositionEffect::Open => self.validate_open_submission(request),
            PositionEffect::Close => self.validate_close_submission(request),
        }
    }

    /// Validates that a new opening order does not cross current open exposure.
    fn validate_open_submission(
        &self,
        request: &SubmitOrderRequest,
    ) -> Result<(), OrderSystemError> {
        if let Some(live_order_side) =
            self.live_conflicting_open_order_side(&request.symbol, request.side)
        {
            return Err(OrderSystemError::LiveOpenOrderSideConflict {
                symbol: request.symbol.clone(),
                requested: request.side,
                live_order_side,
            });
        }

        if let Some(holding) = self.holdings.get(request.symbol.as_str())
            && let Some(holding_side) = holding.side()
            && holding_side != request.side.to_open_lot_side()
        {
            return Err(OrderSystemError::OpenSideMismatch {
                symbol: request.symbol.clone(),
                requested: request.side,
                holding_side,
            });
        }

        Ok(())
    }

    /// Validates that a new close order matches current open exposure and
    /// available-to-close quantity.
    fn validate_close_submission(
        &self,
        request: &SubmitOrderRequest,
    ) -> Result<(), OrderSystemError> {
        let holding = self.holdings.get(request.symbol.as_str()).ok_or_else(|| {
            OrderSystemError::NoOpenPosition {
                symbol: request.symbol.clone(),
            }
        })?;
        let holding_side = holding
            .side()
            .ok_or_else(|| OrderSystemError::NoOpenPosition {
                symbol: request.symbol.clone(),
            })?;

        if request.side.closing_lot_side() != holding_side {
            return Err(OrderSystemError::CloseSideMismatch {
                symbol: request.symbol.clone(),
                requested: request.side,
                holding_side,
            });
        }

        self.validate_close_lot_relief_method(
            holding,
            request.qty,
            request.lot_relief_method.as_ref(),
        )?;

        let available = self.available_close_qty(&request.symbol)?;
        if request.qty > available {
            return Err(OrderSystemError::CloseQuantityExceedsAvailable {
                symbol: request.symbol.clone(),
                requested: request.qty,
                available,
            });
        }

        Ok(())
    }

    /// Validates close-side lot-relief overrides that can be rejected before
    /// any fills arrive.
    fn validate_close_lot_relief_method(
        &self,
        holding: &Holding,
        requested_qty: Decimal,
        lot_relief_method: Option<&LotReliefMethod>,
    ) -> Result<(), OrderSystemError> {
        let Some(LotReliefMethod::SpecificLot(selected_lot_ids)) = lot_relief_method else {
            return Ok(());
        };

        let mut seen_lot_ids = HashSet::with_capacity(selected_lot_ids.len());
        let mut available_qty = Decimal::ZERO;

        for lot_id in selected_lot_ids {
            if !seen_lot_ids.insert(*lot_id) {
                return Err(HoldingError::DuplicateLotIdSelection { lot_id: *lot_id }.into());
            }

            let lot = holding
                .lots()
                .iter()
                .find(|lot| lot.id() == *lot_id)
                .ok_or(HoldingError::UnknownLotId { lot_id: *lot_id })?;
            if !lot.is_open() {
                return Err(HoldingError::LotNotOpen { lot_id: *lot_id }.into());
            }

            let available_on_lot =
                lot.remaining_qty() - self.live_specific_lot_reserved_qty(holding, *lot_id);
            if available_on_lot > Decimal::ZERO {
                available_qty += available_on_lot;
            }
        }

        if available_qty < requested_qty {
            return Err(HoldingError::SpecificLotQuantityInsufficient {
                requested: requested_qty,
                available: available_qty,
            }
            .into());
        }

        Ok(())
    }

    /// Returns quantity already reserved on one lot by other live `SpecificLot`
    /// close orders.
    fn live_specific_lot_reserved_qty(&self, holding: &Holding, lot_id: LotId) -> Decimal {
        self.orders
            .values()
            .filter(|tracked_order| {
                tracked_order.order.symbol() == holding.symbol()
                    && tracked_order.order.position_effect() == PositionEffect::Close
                    && matches!(
                        tracked_order.order.status(),
                        OrderStatus::Pending | OrderStatus::Working | OrderStatus::PartiallyFilled
                    )
            })
            .fold(Decimal::ZERO, |reserved_sum, tracked_order| {
                reserved_sum
                    + self
                        .specific_lot_reservations_for_live_order(holding, tracked_order)
                        .into_iter()
                        .find_map(|(reserved_lot_id, reserved_qty)| {
                            (reserved_lot_id == lot_id).then_some(reserved_qty)
                        })
                        .unwrap_or(Decimal::ZERO)
            })
    }

    /// Returns one conflicting live open-order side, if any exists.
    fn live_conflicting_open_order_side(
        &self,
        symbol: &str,
        requested_side: OrderSide,
    ) -> Option<OrderSide> {
        self.orders
            .values()
            .filter(|tracked_order| {
                tracked_order.order.symbol() == symbol
                    && tracked_order.order.position_effect() == PositionEffect::Open
                    && matches!(
                        tracked_order.order.status(),
                        OrderStatus::Pending | OrderStatus::Working | OrderStatus::PartiallyFilled
                    )
            })
            .map(|tracked_order| tracked_order.order.side())
            .find(|live_order_side| *live_order_side != requested_side)
    }

    /// Activates any contingent orders waiting on the completed parent order.
    fn activate_contingent_orders(&mut self, parent_order_id: OrderId) {
        for tracked_order in self.orders.values_mut() {
            if tracked_order.contingent_on_order_id == Some(parent_order_id) {
                tracked_order.contingent_on_order_id = None;
            }
        }
    }

    /// Cascades cancellation from a parent reversal close order to its staged
    /// contingent open orders.
    fn cancel_contingent_orders(&mut self, parent_order_id: OrderId, canceled_at: DateTime<Utc>) {
        let dependent_order_ids: Vec<OrderId> = self
            .orders
            .iter()
            .filter_map(|(order_id, tracked_order)| {
                (tracked_order.contingent_on_order_id == Some(parent_order_id)).then_some(*order_id)
            })
            .collect();

        for dependent_order_id in dependent_order_ids {
            let order_snapshot = {
                let tracked_order = self
                    .orders
                    .get_mut(&dependent_order_id)
                    .expect("dependent order must exist while cascading cancel");
                if tracked_order.order.status() == OrderStatus::Canceled
                    || tracked_order.order.status() == OrderStatus::Rejected
                    || tracked_order.order.status() == OrderStatus::Filled
                {
                    continue;
                }
                tracked_order
                    .order
                    .cancel(canceled_at)
                    .expect("contingent order must cancel cleanly during parent cancel");
                tracked_order.contingent_on_order_id = None;
                tracked_order.order.clone()
            };
            let report = ExecutionReport::canceled(
                self.allocate_execution_report_id(),
                &order_snapshot,
                canceled_at,
                Some("canceled because reversal close order was canceled".to_owned()),
            );
            self.execution_reports.push(report);
        }
    }

    /// Cascades rejection from a parent reversal close order to its staged
    /// contingent open orders.
    fn reject_contingent_orders(
        &mut self,
        parent_order_id: OrderId,
        rejected_at: DateTime<Utc>,
        reason: &str,
    ) {
        let dependent_order_ids: Vec<OrderId> = self
            .orders
            .iter()
            .filter_map(|(order_id, tracked_order)| {
                (tracked_order.contingent_on_order_id == Some(parent_order_id)).then_some(*order_id)
            })
            .collect();

        for dependent_order_id in dependent_order_ids {
            let order_snapshot = {
                let tracked_order = self
                    .orders
                    .get_mut(&dependent_order_id)
                    .expect("dependent order must exist while cascading reject");
                if tracked_order.order.status() == OrderStatus::Canceled
                    || tracked_order.order.status() == OrderStatus::Rejected
                    || tracked_order.order.status() == OrderStatus::Filled
                {
                    continue;
                }
                tracked_order
                    .order
                    .reject(rejected_at, reason.to_owned())
                    .expect("contingent order must reject cleanly during parent reject");
                tracked_order.contingent_on_order_id = None;
                tracked_order.order.clone()
            };
            let report = ExecutionReport::rejected(
                self.allocate_execution_report_id(),
                &order_snapshot,
                rejected_at,
                reason.to_owned(),
            );
            self.execution_reports.push(report);
        }
    }

    /// Derives the current per-lot reservation plan of one live `SpecificLot`
    /// close order from the holding snapshot and the order's remaining leaves.
    fn specific_lot_reservations_for_live_order(
        &self,
        holding: &Holding,
        tracked_order: &TrackedOrder,
    ) -> Vec<(LotId, Decimal)> {
        let Some(LotReliefMethod::SpecificLot(selected_lot_ids)) =
            tracked_order.close_lot_relief_method.as_ref()
        else {
            return Vec::new();
        };

        let mut remaining_qty = tracked_order.order.leaves_qty();
        let mut reservations = Vec::new();

        for lot_id in selected_lot_ids {
            if remaining_qty <= Decimal::ZERO {
                break;
            }

            let Some(lot) = holding
                .lots()
                .iter()
                .find(|lot| lot.id() == *lot_id && lot.is_open())
            else {
                continue;
            };

            let reserved_qty = if lot.remaining_qty() < remaining_qty {
                lot.remaining_qty()
            } else {
                remaining_qty
            };
            if reserved_qty > Decimal::ZERO {
                reservations.push((*lot_id, reserved_qty));
                remaining_qty -= reserved_qty;
            }
        }

        reservations
    }

    /// Advances order-scoped lot-relief state after one close fill.
    ///
    /// `SpecificLot` needs special handling because the original lot list is
    /// attached to the order rather than to one individual fill. Once a selected
    /// lot is fully consumed by an earlier partial fill, later fills on the same
    /// order must skip it rather than failing with `LotNotOpen`.
    fn advance_close_lot_relief_method(
        &self,
        order: &Order,
        holding: &Holding,
        lot_relief_method: &mut Option<LotReliefMethod>,
    ) {
        if order.leaves_qty().is_zero() {
            *lot_relief_method = None;
            return;
        }

        let Some(LotReliefMethod::SpecificLot(selected_lot_ids)) = lot_relief_method.as_mut()
        else {
            return;
        };

        selected_lot_ids.retain(|lot_id| {
            holding
                .lots()
                .iter()
                .any(|lot| lot.id() == *lot_id && lot.is_open())
        });
    }

    /// Infers the order side needed to close the current open holding.
    fn infer_close_side(&self, symbol: &str) -> Result<OrderSide, OrderSystemError> {
        self.ensure_symbol_not_blank(symbol)?;

        let holding =
            self.holdings
                .get(symbol)
                .ok_or_else(|| OrderSystemError::NoOpenPosition {
                    symbol: symbol.to_owned(),
                })?;

        match holding.side() {
            Some(LotSide::Long) => Ok(OrderSide::Sell),
            Some(LotSide::Short) => Ok(OrderSide::Buy),
            None => Err(OrderSystemError::NoOpenPosition {
                symbol: symbol.to_owned(),
            }),
        }
    }

    /// Rejects blank symbols before more specific validation runs.
    fn ensure_symbol_not_blank(&self, symbol: &str) -> Result<(), OrderSystemError> {
        if symbol.trim().is_empty() {
            return Err(OrderError::EmptySymbol.into());
        }

        Ok(())
    }

    /// Allocates the next order identifier.
    fn allocate_order_id(&mut self) -> OrderId {
        let id = OrderId::new(self.next_order_id);
        self.next_order_id += 1;
        id
    }

    /// Allocates the next holding identifier.
    fn allocate_holding_id(&mut self) -> HoldingId {
        let id = HoldingId::new(self.next_holding_id);
        self.next_holding_id += 1;
        id
    }

    /// Allocates the next lot identifier.
    fn allocate_lot_id(&mut self) -> LotId {
        let id = LotId::new(self.next_lot_id);
        self.next_lot_id += 1;
        id
    }

    /// Allocates the next execution-report identifier.
    fn allocate_execution_report_id(&mut self) -> ExecutionReportId {
        let id = ExecutionReportId::new(self.next_execution_report_id);
        self.next_execution_report_id += 1;
        id
    }
}

trait WithLotReliefMethodOption {
    fn with_lot_relief_method_option(self, lot_relief_method: Option<LotReliefMethod>) -> Self;
}

impl WithLotReliefMethodOption for SubmitOrderRequest {
    fn with_lot_relief_method_option(mut self, lot_relief_method: Option<LotReliefMethod>) -> Self {
        self.lot_relief_method = lot_relief_method;
        self
    }
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};
    use rust_decimal::Decimal;

    use super::{
        AppliedFill, FlattenPositionRequest, OrderSystem, OrderSystemError,
        SubmitCloseOrderRequest, SubmitOrderRequest,
    };
    use crate::order_sys::execution_report::ExecutionReportType;
    use crate::order_sys::fill::Fill;
    use crate::order_sys::holding::LotReliefMethod;
    use crate::order_sys::lot::LotSide;
    use crate::order_sys::order::{OrderSide, OrderStatus, OrderType, PositionEffect, TimeInForce};
    use crate::order_sys::{FillId, LotId, OrderId};

    fn dec(value: i64, scale: u32) -> Decimal {
        Decimal::new(value, scale)
    }

    fn ts(day: u32) -> chrono::DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 5, day, 9, 30, 0).unwrap()
    }

    fn open_fill(order_id: u64, fill_id: u64, day: u32, qty: i64, price: i64) -> Fill {
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

    fn close_fill(order_id: u64, fill_id: u64, day: u32, qty: i64, price: i64) -> Fill {
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

    fn short_open_fill(order_id: u64, fill_id: u64, day: u32, qty: i64, price: i64) -> Fill {
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

    fn submit_and_fill_long(
        system: &mut OrderSystem,
        day: u32,
        qty: i64,
        price: i64,
    ) -> AppliedFill {
        let submission = system
            .submit_order(SubmitOrderRequest::new(
                "AAPL",
                OrderSide::Buy,
                PositionEffect::Open,
                OrderType::Market,
                TimeInForce::Day,
                ts(day),
                dec(qty, 0),
                None,
                None,
            ))
            .unwrap();

        system
            .apply_fill(open_fill(
                submission.order.id().value(),
                1000 + submission.order.id().value(),
                day + 1,
                qty,
                price,
            ))
            .unwrap()
    }

    #[test]
    fn submit_order_creates_pending_order_and_report() {
        let mut system = OrderSystem::new(LotReliefMethod::Fifo);

        let submission = system
            .submit_order(SubmitOrderRequest::new(
                "AAPL",
                OrderSide::Buy,
                PositionEffect::Open,
                OrderType::Market,
                TimeInForce::Day,
                ts(24),
                dec(5, 0),
                None,
                None,
            ))
            .unwrap();

        assert_eq!(submission.order.id(), OrderId::new(1));
        assert_eq!(submission.order.status(), OrderStatus::Pending);
        assert_eq!(
            submission.report.report_type(),
            ExecutionReportType::PendingNew
        );
        assert_eq!(system.execution_reports().len(), 1);
        assert_eq!(system.order(OrderId::new(1)).unwrap().symbol(), "AAPL");
        assert!(system.holding("AAPL").is_none());
    }

    #[test]
    fn apply_open_fill_creates_holding_and_trade_report() {
        let mut system = OrderSystem::new(LotReliefMethod::Fifo);
        let submission = system
            .submit_order(SubmitOrderRequest::new(
                "AAPL",
                OrderSide::Buy,
                PositionEffect::Open,
                OrderType::Market,
                TimeInForce::Day,
                ts(24),
                dec(5, 0),
                None,
                None,
            ))
            .unwrap();

        system
            .acknowledge_order(submission.order.id(), ts(24))
            .unwrap();
        let applied = system.apply_fill(open_fill(1, 10, 25, 5, 100)).unwrap();

        assert_eq!(applied.order.status(), OrderStatus::Filled);
        assert_eq!(applied.report.report_type(), ExecutionReportType::Trade);
        assert_eq!(applied.opened_lot_id, Some(LotId::new(1)));
        assert!(applied.holding_close.is_none());
        assert_eq!(system.holding("AAPL").unwrap().open_qty(), dec(5, 0));
        assert_eq!(system.holding("AAPL").unwrap().side(), Some(LotSide::Long));
        assert!(system.fill(FillId::new(10)).is_some());
        assert_eq!(system.execution_reports().len(), 3);
    }

    #[test]
    fn submit_close_order_infers_side_and_reserves_quantity() {
        let mut system = OrderSystem::new(LotReliefMethod::Fifo);
        submit_and_fill_long(&mut system, 24, 5, 100);

        let close = system
            .submit_close_order(SubmitCloseOrderRequest::new(
                "AAPL",
                ts(26),
                dec(3, 0),
                OrderType::Market,
                TimeInForce::Day,
                None,
                None,
            ))
            .unwrap();

        assert_eq!(close.order.side(), OrderSide::Sell);
        assert_eq!(close.order.position_effect(), PositionEffect::Close);
        assert_eq!(system.available_close_qty("AAPL").unwrap(), dec(2, 0));

        let err = system
            .submit_close_order(SubmitCloseOrderRequest::new(
                "AAPL",
                ts(26),
                dec(3, 0),
                OrderType::Market,
                TimeInForce::Day,
                None,
                None,
            ))
            .unwrap_err();
        assert_eq!(
            err,
            OrderSystemError::CloseQuantityExceedsAvailable {
                symbol: "AAPL".to_owned(),
                requested: dec(3, 0),
                available: dec(2, 0),
            }
        );
    }

    #[test]
    fn flatten_position_uses_remaining_unreserved_quantity() {
        let mut system = OrderSystem::new(LotReliefMethod::Fifo);
        submit_and_fill_long(&mut system, 24, 5, 100);

        system
            .submit_close_order(SubmitCloseOrderRequest::new(
                "AAPL",
                ts(26),
                dec(2, 0),
                OrderType::Market,
                TimeInForce::Day,
                None,
                None,
            ))
            .unwrap();

        let flatten = system
            .flatten_position(FlattenPositionRequest::new(
                "AAPL",
                ts(27),
                OrderType::Market,
                TimeInForce::Day,
            ))
            .unwrap();

        assert_eq!(flatten.order.side(), OrderSide::Sell);
        assert_eq!(flatten.order.requested_qty(), dec(3, 0));
    }

    #[test]
    fn apply_close_fill_uses_order_specific_lot_relief_override() {
        let mut system = OrderSystem::new(LotReliefMethod::Fifo);
        submit_and_fill_long(&mut system, 24, 3, 100);
        submit_and_fill_long(&mut system, 26, 3, 105);

        let close = system
            .submit_close_order(
                SubmitCloseOrderRequest::new(
                    "AAPL",
                    ts(28),
                    dec(4, 0),
                    OrderType::Market,
                    TimeInForce::Day,
                    None,
                    None,
                )
                .with_lot_relief_method(LotReliefMethod::Lifo),
            )
            .unwrap();

        let applied = system
            .apply_fill(close_fill(close.order.id().value(), 20, 29, 4, 110))
            .unwrap();
        let close_result = applied
            .holding_close
            .expect("close fill must return summary");

        assert_eq!(close_result.matches.len(), 2);
        assert_eq!(close_result.matches[0].lot_id, LotId::new(2));
        assert_eq!(close_result.matches[0].close.qty, dec(3, 0));
        assert_eq!(close_result.matches[1].lot_id, LotId::new(1));
        assert_eq!(close_result.matches[1].close.qty, dec(1, 0));
        assert_eq!(system.holding("AAPL").unwrap().open_qty(), dec(2, 0));
    }

    #[test]
    fn submit_order_rejects_opposite_open_side_against_existing_position() {
        let mut system = OrderSystem::new(LotReliefMethod::Fifo);
        submit_and_fill_long(&mut system, 24, 2, 100);

        let err = system
            .submit_order(SubmitOrderRequest::new(
                "AAPL",
                OrderSide::Sell,
                PositionEffect::Open,
                OrderType::Market,
                TimeInForce::Day,
                ts(26),
                dec(1, 0),
                None,
                None,
            ))
            .unwrap_err();

        assert_eq!(
            err,
            OrderSystemError::OpenSideMismatch {
                symbol: "AAPL".to_owned(),
                requested: OrderSide::Sell,
                holding_side: LotSide::Long,
            }
        );
    }

    #[test]
    fn cancel_close_order_releases_reserved_quantity() {
        let mut system = OrderSystem::new(LotReliefMethod::Fifo);
        submit_and_fill_long(&mut system, 24, 5, 100);

        let close = system
            .submit_close_order(SubmitCloseOrderRequest::new(
                "AAPL",
                ts(26),
                dec(5, 0),
                OrderType::Market,
                TimeInForce::Day,
                None,
                None,
            ))
            .unwrap();

        let err = system
            .submit_close_order(SubmitCloseOrderRequest::new(
                "AAPL",
                ts(26),
                dec(1, 0),
                OrderType::Market,
                TimeInForce::Day,
                None,
                None,
            ))
            .unwrap_err();
        assert_eq!(
            err,
            OrderSystemError::CloseQuantityExceedsAvailable {
                symbol: "AAPL".to_owned(),
                requested: dec(1, 0),
                available: Decimal::ZERO,
            }
        );

        system.cancel_order(close.order.id(), ts(27)).unwrap();

        assert!(
            system
                .submit_close_order(SubmitCloseOrderRequest::new(
                    "AAPL",
                    ts(28),
                    dec(1, 0),
                    OrderType::Market,
                    TimeInForce::Day,
                    None,
                    None,
                ))
                .is_ok()
        );
    }

    #[test]
    fn allow_reversal_stages_open_order_until_auto_close_fills() {
        let mut system = OrderSystem::new(LotReliefMethod::Fifo);
        submit_and_fill_long(&mut system, 24, 2, 100);

        let reversal = system
            .submit_order(
                SubmitOrderRequest::new(
                    "AAPL",
                    OrderSide::Sell,
                    PositionEffect::Open,
                    OrderType::Market,
                    TimeInForce::Day,
                    ts(26),
                    dec(1, 0),
                    None,
                    None,
                )
                .with_allow_reversal(true),
            )
            .unwrap();

        let auto_close = reversal
            .auto_close_order
            .as_ref()
            .expect("reversal must create an auto-close order");
        assert_eq!(auto_close.order.position_effect(), PositionEffect::Close);
        assert_eq!(auto_close.order.requested_qty(), dec(2, 0));
        assert_eq!(reversal.order.position_effect(), PositionEffect::Open);
        assert_eq!(reversal.order.requested_qty(), dec(1, 0));

        let early_fill_err = system
            .apply_fill(short_open_fill(reversal.order.id().value(), 200, 27, 1, 99))
            .unwrap_err();
        assert_eq!(
            early_fill_err,
            OrderSystemError::OrderNotActive {
                order_id: reversal.order.id(),
                waiting_on_order_id: auto_close.order.id(),
            }
        );

        system
            .apply_fill(close_fill(auto_close.order.id().value(), 201, 27, 2, 99))
            .unwrap();
        system
            .apply_fill(short_open_fill(reversal.order.id().value(), 202, 28, 1, 98))
            .unwrap();

        assert_eq!(system.holding("AAPL").unwrap().side(), Some(LotSide::Short));
        assert_eq!(system.holding("AAPL").unwrap().open_qty(), dec(1, 0));
    }

    #[test]
    fn canceling_auto_close_cascades_to_contingent_open_order() {
        let mut system = OrderSystem::new(LotReliefMethod::Fifo);
        submit_and_fill_long(&mut system, 24, 2, 100);

        let reversal = system
            .submit_order(
                SubmitOrderRequest::new(
                    "AAPL",
                    OrderSide::Sell,
                    PositionEffect::Open,
                    OrderType::Market,
                    TimeInForce::Day,
                    ts(26),
                    dec(1, 0),
                    None,
                    None,
                )
                .with_allow_reversal(true),
            )
            .unwrap();

        let auto_close = reversal
            .auto_close_order
            .as_ref()
            .expect("reversal must create an auto-close order");
        system.cancel_order(auto_close.order.id(), ts(27)).unwrap();

        assert_eq!(
            system.order(auto_close.order.id()).unwrap().status(),
            OrderStatus::Canceled
        );
        assert_eq!(
            system.order(reversal.order.id()).unwrap().status(),
            OrderStatus::Canceled
        );
    }

    #[test]
    fn specific_lot_close_order_can_fill_across_multiple_fills() {
        let mut system = OrderSystem::new(LotReliefMethod::Fifo);
        submit_and_fill_long(&mut system, 24, 1, 100);
        submit_and_fill_long(&mut system, 25, 1, 105);

        let close = system
            .submit_close_order(
                SubmitCloseOrderRequest::new(
                    "AAPL",
                    ts(26),
                    dec(2, 0),
                    OrderType::Market,
                    TimeInForce::Day,
                    None,
                    None,
                )
                .with_lot_relief_method(LotReliefMethod::SpecificLot(vec![
                    LotId::new(1),
                    LotId::new(2),
                ])),
            )
            .unwrap();

        system
            .apply_fill(close_fill(close.order.id().value(), 30, 27, 1, 110))
            .unwrap();
        let second_fill = system
            .apply_fill(close_fill(close.order.id().value(), 31, 28, 1, 111))
            .unwrap();

        let close_result = second_fill
            .holding_close
            .expect("second partial fill must produce a close summary");
        assert_eq!(close_result.matches.len(), 1);
        assert_eq!(close_result.matches[0].lot_id, LotId::new(2));
        assert_eq!(close_result.matches[0].close.qty, dec(1, 0));
        assert!(system.holding("AAPL").unwrap().is_flat());
    }

    #[test]
    fn submit_order_rejects_opposite_open_side_while_live_open_order_exists() {
        let mut system = OrderSystem::new(LotReliefMethod::Fifo);

        system
            .submit_order(SubmitOrderRequest::new(
                "AAPL",
                OrderSide::Buy,
                PositionEffect::Open,
                OrderType::Market,
                TimeInForce::Day,
                ts(24),
                dec(1, 0),
                None,
                None,
            ))
            .unwrap();

        let second_open = system
            .submit_order(SubmitOrderRequest::new(
                "AAPL",
                OrderSide::Sell,
                PositionEffect::Open,
                OrderType::Market,
                TimeInForce::Day,
                ts(24),
                dec(1, 0),
                None,
                None,
            ))
            .unwrap_err();

        assert_eq!(
            second_open,
            OrderSystemError::LiveOpenOrderSideConflict {
                symbol: "AAPL".to_owned(),
                requested: OrderSide::Sell,
                live_order_side: OrderSide::Buy,
            }
        );
    }

    #[test]
    fn submit_close_order_rejects_invalid_specific_lot_selection_up_front() {
        let mut system = OrderSystem::new(LotReliefMethod::Fifo);
        submit_and_fill_long(&mut system, 24, 1, 100);
        submit_and_fill_long(&mut system, 25, 1, 105);

        let err = system
            .submit_close_order(
                SubmitCloseOrderRequest::new(
                    "AAPL",
                    ts(26),
                    dec(2, 0),
                    OrderType::Market,
                    TimeInForce::Day,
                    None,
                    None,
                )
                .with_lot_relief_method(LotReliefMethod::SpecificLot(vec![LotId::new(1)])),
            )
            .unwrap_err();

        assert_eq!(
            err,
            OrderSystemError::Holding(
                crate::order_sys::holding::HoldingError::SpecificLotQuantityInsufficient {
                    requested: dec(2, 0),
                    available: dec(1, 0),
                }
            )
        );
    }

    #[test]
    fn submit_close_order_rejects_specific_lot_overreservation_from_live_orders() {
        let mut system = OrderSystem::new(LotReliefMethod::Fifo);
        submit_and_fill_long(&mut system, 24, 5, 100);
        submit_and_fill_long(&mut system, 25, 5, 105);

        system
            .submit_close_order(
                SubmitCloseOrderRequest::new(
                    "AAPL",
                    ts(26),
                    dec(4, 0),
                    OrderType::Market,
                    TimeInForce::Day,
                    None,
                    None,
                )
                .with_lot_relief_method(LotReliefMethod::SpecificLot(vec![LotId::new(1)])),
            )
            .unwrap();

        let err = system
            .submit_close_order(
                SubmitCloseOrderRequest::new(
                    "AAPL",
                    ts(27),
                    dec(2, 0),
                    OrderType::Market,
                    TimeInForce::Day,
                    None,
                    None,
                )
                .with_lot_relief_method(LotReliefMethod::SpecificLot(vec![LotId::new(1)])),
            )
            .unwrap_err();

        assert_eq!(
            err,
            OrderSystemError::Holding(
                crate::order_sys::holding::HoldingError::SpecificLotQuantityInsufficient {
                    requested: dec(2, 0),
                    available: dec(1, 0),
                }
            )
        );
    }
}
