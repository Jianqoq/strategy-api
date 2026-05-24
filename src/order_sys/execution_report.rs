//! Order-state snapshots for downstream consumers.
//!
//! An [`ExecutionReport`] is a denormalized view of an [`Order`](crate::order_sys::order::Order)
//! at a specific event boundary. It is useful for event sourcing, logging,
//! host callbacks, and integration surfaces that expect FIX-like execution
//! reports rather than direct access to the full order aggregate.

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;

use crate::order_sys::fill::Fill;
use crate::order_sys::order::{Order, OrderStatus};
use crate::order_sys::{ExecutionReportId, FillId, OrderId};

/// High-level meaning of an execution report snapshot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExecutionReportType {
    /// The order was created locally but not yet acknowledged.
    PendingNew,
    /// The order was acknowledged and is now working.
    New,
    /// The order received a fill.
    Trade,
    /// The order was canceled.
    Canceled,
    /// The order was rejected.
    Rejected,
}

/// Snapshot of order state at one event boundary.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExecutionReport {
    /// Unique identifier of the report itself.
    id: ExecutionReportId,
    /// Order the report belongs to.
    order_id: OrderId,
    /// Fill that triggered the report, if any.
    fill_id: Option<FillId>,
    /// High-level report category.
    report_type: ExecutionReportType,
    /// Order status after applying the event.
    status: OrderStatus,
    /// UTC timestamp when the report event occurred.
    occurred_at: DateTime<Utc>,
    /// Quantity executed by the triggering event.
    last_qty: Decimal,
    /// Price executed by the triggering event, if any.
    last_price: Option<Decimal>,
    /// Fees charged by the triggering event.
    last_fees: Decimal,
    /// Cumulative filled quantity on the order.
    cumulative_qty: Decimal,
    /// Remaining quantity still open on the order.
    leaves_qty: Decimal,
    /// Weighted average fill price on the order.
    average_price: Option<Decimal>,
    /// Cumulative fees charged to the order.
    cumulative_fees: Decimal,
    /// Optional free-form text, for example a reject reason.
    text: Option<String>,
}

impl ExecutionReport {
    /// Builds a `PendingNew` report from an order snapshot.
    pub fn pending_new(id: ExecutionReportId, order: &Order) -> Self {
        Self::from_snapshot(
            id,
            order,
            None,
            ExecutionReportType::PendingNew,
            order.submitted_at(),
            Decimal::ZERO,
            None,
            Decimal::ZERO,
            None,
        )
    }

    /// Builds a `New` report from an acknowledged order snapshot.
    pub fn acknowledged(id: ExecutionReportId, order: &Order, occurred_at: DateTime<Utc>) -> Self {
        Self::from_snapshot(
            id,
            order,
            None,
            ExecutionReportType::New,
            occurred_at,
            Decimal::ZERO,
            None,
            Decimal::ZERO,
            None,
        )
    }

    /// Builds a `Trade` report from an order snapshot and the triggering fill.
    pub fn trade(id: ExecutionReportId, order: &Order, fill: &Fill) -> Self {
        Self::from_snapshot(
            id,
            order,
            Some(fill.id()),
            ExecutionReportType::Trade,
            fill.executed_at(),
            fill.qty(),
            Some(fill.price()),
            fill.fees(),
            None,
        )
    }

    /// Builds a `Canceled` report from an order snapshot.
    pub fn canceled(
        id: ExecutionReportId,
        order: &Order,
        occurred_at: DateTime<Utc>,
        text: Option<String>,
    ) -> Self {
        Self::from_snapshot(
            id,
            order,
            None,
            ExecutionReportType::Canceled,
            occurred_at,
            Decimal::ZERO,
            None,
            Decimal::ZERO,
            text,
        )
    }

    /// Builds a `Rejected` report from an order snapshot.
    pub fn rejected(
        id: ExecutionReportId,
        order: &Order,
        occurred_at: DateTime<Utc>,
        text: impl Into<String>,
    ) -> Self {
        Self::from_snapshot(
            id,
            order,
            None,
            ExecutionReportType::Rejected,
            occurred_at,
            Decimal::ZERO,
            None,
            Decimal::ZERO,
            Some(text.into()),
        )
    }

    /// Returns the report identifier.
    pub fn id(&self) -> ExecutionReportId {
        self.id
    }

    /// Returns the parent order identifier.
    pub fn order_id(&self) -> OrderId {
        self.order_id
    }

    /// Returns the triggering fill identifier, if one exists.
    pub fn fill_id(&self) -> Option<FillId> {
        self.fill_id
    }

    /// Returns the high-level report category.
    pub fn report_type(&self) -> ExecutionReportType {
        self.report_type
    }

    /// Returns the order status captured by the snapshot.
    pub fn status(&self) -> OrderStatus {
        self.status
    }

    /// Returns the event timestamp.
    pub fn occurred_at(&self) -> DateTime<Utc> {
        self.occurred_at
    }

    /// Returns the last event quantity.
    pub fn last_qty(&self) -> Decimal {
        self.last_qty
    }

    /// Returns the last event price, if any.
    pub fn last_price(&self) -> Option<Decimal> {
        self.last_price
    }

    /// Returns the last event fees.
    pub fn last_fees(&self) -> Decimal {
        self.last_fees
    }

    /// Returns cumulative filled quantity.
    pub fn cumulative_qty(&self) -> Decimal {
        self.cumulative_qty
    }

    /// Returns remaining open quantity.
    pub fn leaves_qty(&self) -> Decimal {
        self.leaves_qty
    }

    /// Returns weighted average fill price, if any fills exist.
    pub fn average_price(&self) -> Option<Decimal> {
        self.average_price
    }

    /// Returns cumulative fees recorded on the order.
    pub fn cumulative_fees(&self) -> Decimal {
        self.cumulative_fees
    }

    /// Returns optional free-form text.
    pub fn text(&self) -> Option<&str> {
        self.text.as_deref()
    }

    /// Internal helper that copies all order-level snapshot data into a report.
    ///
    /// This method intentionally centralizes report construction so all report
    /// types keep the same denormalized field semantics.
    fn from_snapshot(
        id: ExecutionReportId,
        order: &Order,
        fill_id: Option<FillId>,
        report_type: ExecutionReportType,
        occurred_at: DateTime<Utc>,
        last_qty: Decimal,
        last_price: Option<Decimal>,
        last_fees: Decimal,
        text: Option<String>,
    ) -> Self {
        Self {
            id,
            order_id: order.id(),
            fill_id,
            report_type,
            status: order.status(),
            occurred_at,
            last_qty,
            last_price,
            last_fees,
            cumulative_qty: order.filled_qty(),
            leaves_qty: order.leaves_qty(),
            average_price: order.average_fill_price(),
            cumulative_fees: order.cumulative_fees(),
            text,
        }
    }
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};
    use rust_decimal::Decimal;

    use super::{ExecutionReport, ExecutionReportType};
    use crate::order_sys::fill::Fill;
    use crate::order_sys::order::{
        Order, OrderSide, OrderStatus, OrderType, PositionEffect, TimeInForce,
    };
    use crate::order_sys::{ExecutionReportId, FillId, OrderId};

    fn dec(value: i64, scale: u32) -> Decimal {
        Decimal::new(value, scale)
    }

    fn ts(day: u32) -> chrono::DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 5, day, 9, 30, 0).unwrap()
    }

    #[test]
    fn execution_report_snapshots_trade_state() {
        let mut order = Order::new(
            OrderId::new(1),
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
        order.acknowledge(ts(24)).unwrap();

        let fill = Fill::new(
            FillId::new(10),
            OrderId::new(1),
            "AAPL",
            OrderSide::Buy,
            PositionEffect::Open,
            ts(25),
            dec(2, 0),
            dec(101, 0),
            dec(2, 1),
        )
        .unwrap();
        order.record_fill(&fill).unwrap();

        let report = ExecutionReport::trade(ExecutionReportId::new(100), &order, &fill);

        assert_eq!(report.report_type(), ExecutionReportType::Trade);
        assert_eq!(report.order_id(), OrderId::new(1));
        assert_eq!(report.fill_id(), Some(FillId::new(10)));
        assert_eq!(report.status(), OrderStatus::PartiallyFilled);
        assert_eq!(report.last_qty(), dec(2, 0));
        assert_eq!(report.last_price(), Some(dec(101, 0)));
        assert_eq!(report.cumulative_qty(), dec(2, 0));
        assert_eq!(report.leaves_qty(), dec(3, 0));
        assert_eq!(report.cumulative_fees(), dec(2, 1));
    }
}
