use chrono::{DateTime, Utc};
use rust_decimal::Decimal;

use crate::order_sys::fill::Fill;
use crate::order_sys::order::{Order, OrderStatus};
use crate::order_sys::{ExecutionReportId, FillId, OrderId};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExecutionReportType {
    PendingNew,
    New,
    Trade,
    Canceled,
    Rejected,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExecutionReport {
    id: ExecutionReportId,
    order_id: OrderId,
    fill_id: Option<FillId>,
    report_type: ExecutionReportType,
    status: OrderStatus,
    occurred_at: DateTime<Utc>,
    last_qty: Decimal,
    last_price: Option<Decimal>,
    last_fees: Decimal,
    cumulative_qty: Decimal,
    leaves_qty: Decimal,
    average_price: Option<Decimal>,
    cumulative_fees: Decimal,
    text: Option<String>,
}

impl ExecutionReport {
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

    pub fn id(&self) -> ExecutionReportId {
        self.id
    }

    pub fn order_id(&self) -> OrderId {
        self.order_id
    }

    pub fn fill_id(&self) -> Option<FillId> {
        self.fill_id
    }

    pub fn report_type(&self) -> ExecutionReportType {
        self.report_type
    }

    pub fn status(&self) -> OrderStatus {
        self.status
    }

    pub fn occurred_at(&self) -> DateTime<Utc> {
        self.occurred_at
    }

    pub fn last_qty(&self) -> Decimal {
        self.last_qty
    }

    pub fn last_price(&self) -> Option<Decimal> {
        self.last_price
    }

    pub fn last_fees(&self) -> Decimal {
        self.last_fees
    }

    pub fn cumulative_qty(&self) -> Decimal {
        self.cumulative_qty
    }

    pub fn leaves_qty(&self) -> Decimal {
        self.leaves_qty
    }

    pub fn average_price(&self) -> Option<Decimal> {
        self.average_price
    }

    pub fn cumulative_fees(&self) -> Decimal {
        self.cumulative_fees
    }

    pub fn text(&self) -> Option<&str> {
        self.text.as_deref()
    }

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
