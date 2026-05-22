use rustc_hash::FxHashMap;

// ── Identifiers ───────────────────────────────────────────────────────────────

pub type OrderId = u64;
pub type PositionId = u64;

// ── Side ──────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Long,
    Short,
}

// ── Account ───────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct SpotAccount {
    pub initial_capital: f64,
    pub cash: f64,
    pub commission_rate: f64,
    pub slippage: f32,
}

impl SpotAccount {
    pub fn new(initial_capital: f64, commission_rate: f64, slippage: f32) -> Self {
        Self {
            initial_capital,
            cash: initial_capital,
            commission_rate,
            slippage,
        }
    }
}

#[derive(Debug, Clone)]
pub struct FuturesAccount {
    pub initial_capital: f64,
    pub cash: f64,
    pub commission_rate: f64,
    pub slippage: f32,
    pub leverage: f64,
}

impl FuturesAccount {
    pub fn new(initial_capital: f64, commission_rate: f64, slippage: f32, leverage: f64) -> Self {
        Self {
            initial_capital,
            cash: initial_capital,
            commission_rate,
            slippage,
            leverage,
        }
    }
}

#[derive(Debug, Clone)]
pub enum Account {
    Spot(SpotAccount),
    Futures(FuturesAccount),
}

impl Account {
    pub fn initial_capital(&self) -> f64 {
        match self {
            Account::Spot(a) => a.initial_capital,
            Account::Futures(a) => a.initial_capital,
        }
    }

    pub fn cash(&self) -> f64 {
        match self {
            Account::Spot(a) => a.cash,
            Account::Futures(a) => a.cash,
        }
    }

    pub fn commission_rate(&self) -> f64 {
        match self {
            Account::Spot(a) => a.commission_rate,
            Account::Futures(a) => a.commission_rate,
        }
    }

    pub fn slippage(&self) -> f32 {
        match self {
            Account::Spot(a) => a.slippage,
            Account::Futures(a) => a.slippage,
        }
    }

    /// Deduct cost of opening a position. Returns the margin reserved.
    fn deduct_open(&mut self, notional: f64, commission: f64) -> f64 {
        match self {
            Account::Spot(a) => {
                let margin = notional;
                a.cash -= margin + commission;
                margin
            }
            Account::Futures(a) => {
                let margin = notional / a.leverage;
                a.cash -= margin + commission;
                margin
            }
        }
    }

    /// Return funds after closing a position.
    fn refund_close(&mut self, released_margin: f64, net_pnl: f64) {
        match self {
            Account::Spot(a) => a.cash += released_margin + net_pnl,
            Account::Futures(a) => a.cash += released_margin + net_pnl,
        }
    }

    /// Return ids of positions to liquidate (worst PnL first).
    /// Spot always returns empty.
    fn liquidation_candidates(
        &self,
        positions: &[Position],
        current_price: f32,
    ) -> Vec<PositionId> {
        match self {
            Account::Spot(_) => Vec::new(),
            Account::Futures(a) => {
                let unrealized: f64 = positions
                    .iter()
                    .map(|p| p.unrealized_pnl(current_price))
                    .sum();
                let equity = a.cash + unrealized;
                let total_maintenance: f64 = positions.iter().map(|p| p.margin).sum();

                if equity >= total_maintenance {
                    return Vec::new();
                }

                let mut candidates: Vec<(PositionId, f64)> = positions
                    .iter()
                    .map(|p| (p.id, p.unrealized_pnl(current_price)))
                    .collect();
                candidates.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
                candidates.into_iter().map(|(id, _)| id).collect()
            }
        }
    }

    fn on_liquidation(&mut self, released_margin: f64, net_pnl: f64) {
        match self {
            Account::Spot(a) => {
                a.cash += released_margin + net_pnl;
                if a.cash < 0.0 {
                    a.cash = 0.0;
                }
            }
            Account::Futures(a) => {
                a.cash += released_margin + net_pnl;
                if a.cash < 0.0 {
                    a.cash = 0.0;
                }
            }
        }
    }
}

// ── Order ─────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum OrderKind {
    Market,
    Limit { limit_price: f32 },
    Stop { stop_price: f32 },
    StopLimit { stop_price: f32, limit_price: f32 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrderStatus {
    Pending,
    PartiallyFilled,
    Filled,
    Cancelled,
    Rejected,
}

#[derive(Debug, Clone)]
pub struct Fill {
    pub bar_index: usize,
    pub price: f32,
    pub qty: f64,
    pub commission: f64,
}

#[derive(Debug, Clone)]
pub struct Order {
    pub id: OrderId,
    pub submitted_bar: usize,
    pub side: Side,
    pub kind: OrderKind,
    pub qty: f64,
    pub filled_qty: f64,
    pub avg_fill_price: f64,
    pub status: OrderStatus,
    pub fills: Vec<Fill>,
}

impl Order {
    pub fn remaining_qty(&self) -> f64 {
        self.qty - self.filled_qty
    }

    pub fn is_terminal(&self) -> bool {
        matches!(
            self.status,
            OrderStatus::Filled | OrderStatus::Cancelled | OrderStatus::Rejected
        )
    }
}

// ── Position ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct Position {
    pub id: PositionId,
    pub side: Side,
    pub open_bar: usize,
    pub qty: f64,
    pub avg_cost: f64,
    /// Margin reserved for this position (cash locked).
    pub margin: f64,
    pub entry_fills: Vec<Fill>,
    pub exit_fills: Vec<Fill>,
}

impl Position {
    pub fn unrealized_pnl(&self, current_price: f32) -> f64 {
        let price = current_price as f64;
        match self.side {
            Side::Long => (price - self.avg_cost) * self.qty,
            Side::Short => (self.avg_cost - price) * self.qty,
        }
    }
}

// ── Liquidation record ────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct Liquidation {
    pub position_id: PositionId,
    pub bar_index: usize,
    pub side: Side,
    pub qty: f64,
    pub price: f32,
    pub loss: f64,
}

// ── Trade ─────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct Trade {
    pub position_id: PositionId,
    pub side: Side,
    pub open_bar: usize,
    pub close_bar: usize,
    pub qty: f64,
    pub avg_entry: f64,
    pub avg_exit: f64,
    pub gross_pnl: f64,
    pub commission: f64,
    pub net_pnl: f64,
}

// ── Annotation ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnnotationKind {
    ArrowUp,
    ArrowDown,
    HLine,
}

#[derive(Debug, Clone, Copy)]
pub struct Annotation {
    pub bar_index: usize,
    pub price: f32,
    /// ARGB packed: 0xAARRGGBB
    pub color: u32,
    pub kind: AnnotationKind,
}

// ── BacktestCtx ───────────────────────────────────────────────────────────────
#[derive(Debug, Clone)]
pub struct BacktestCtx {
    pub account: Account,

    // Live orders: Pending or PartiallyFilled, keyed by order id
    pub open_orders: FxHashMap<OrderId, Order>,
    // Terminal orders: Filled, Cancelled, or Rejected
    pub closed_orders: Vec<Order>,

    // Open positions (Long and Short can coexist)
    pub open_positions: Vec<Position>,
    // Completed trade records
    pub closed_trades: Vec<Trade>,
    // Liquidation records
    pub liquidations: Vec<Liquidation>,

    // Equity tracking
    pub equity_curve: Vec<f64>,
    pub peak_equity: f64,
    pub max_drawdown: f64,

    // Chart annotations
    pub annotations: Vec<Annotation>,

    // Progress (set by host before each on_bar call)
    pub current_bar_index: usize,
    pub total_bars: usize,

    next_order_id: OrderId,
    next_position_id: PositionId,
}

impl BacktestCtx {
    pub fn new(account: Account, total_bars: usize) -> Self {
        let peak_equity = account.initial_capital();
        Self {
            account,
            open_orders: FxHashMap::default(),
            closed_orders: Vec::new(),
            open_positions: Vec::new(),
            closed_trades: Vec::new(),
            liquidations: Vec::new(),
            equity_curve: Vec::with_capacity(total_bars),
            peak_equity,
            max_drawdown: 0.0,
            annotations: Vec::new(),
            current_bar_index: 0,
            total_bars,
            next_order_id: 1,
            next_position_id: 1,
        }
    }

    // ── Order submission ──────────────────────────────────────────────────────

    pub fn submit_order(&mut self, side: Side, kind: OrderKind, qty: f64) -> OrderId {
        let id = self.next_order_id;
        self.next_order_id += 1;
        self.open_orders.insert(
            id,
            Order {
                id,
                submitted_bar: self.current_bar_index,
                side,
                kind,
                qty,
                filled_qty: 0.0,
                avg_fill_price: 0.0,
                status: OrderStatus::Pending,
                fills: Vec::new(),
            },
        );
        id
    }

    pub fn cancel_order(&mut self, order_id: OrderId) -> bool {
        if let Some(mut order) = self.open_orders.remove(&order_id) {
            order.status = OrderStatus::Cancelled;
            self.closed_orders.push(order);
            true
        } else {
            false
        }
    }

    // ── Fill application (called by host engine) ──────────────────────────────

    /// Apply a (partial) fill to an order. Creates or updates the matching position.
    /// Returns the fill commission.
    pub fn apply_fill(&mut self, order_id: OrderId, fill_price: f32, fill_qty: f64) -> f64 {
        let order = match self.open_orders.get_mut(&order_id) {
            Some(o) => o,
            None => return 0.0,
        };

        let fill_qty = fill_qty.min(order.remaining_qty());
        if fill_qty <= 0.0 {
            return 0.0;
        }
        let commission = fill_price as f64 * fill_qty * self.account.commission_rate();

        let fill = Fill {
            bar_index: self.current_bar_index,
            price: fill_price,
            qty: fill_qty,
            commission,
        };

        let prev_value = order.avg_fill_price * order.filled_qty;
        order.filled_qty += fill_qty;
        order.avg_fill_price = (prev_value + fill_price as f64 * fill_qty) / order.filled_qty;
        order.fills.push(fill.clone());

        if (order.qty - order.filled_qty).abs() < 1e-9 {
            order.status = OrderStatus::Filled;
        } else {
            order.status = OrderStatus::PartiallyFilled;
        }

        let side = order.side;
        let is_terminal = order.is_terminal();

        if is_terminal {
            let order = self.open_orders.remove(&order_id).unwrap();
            self.closed_orders.push(order);
        }

        let notional = fill_price as f64 * fill_qty;
        let margin = self.account.deduct_open(notional, commission);

        if let Some(pos) = self.open_positions.iter_mut().find(|p| p.side == side) {
            let prev_value = pos.avg_cost * pos.qty;
            pos.qty += fill_qty;
            pos.avg_cost = (prev_value + fill_price as f64 * fill_qty) / pos.qty;
            pos.margin += margin;
            pos.entry_fills.push(fill);
        } else {
            let id = self.next_position_id;
            self.next_position_id += 1;
            self.open_positions.push(Position {
                id,
                side,
                open_bar: self.current_bar_index,
                qty: fill_qty,
                avg_cost: fill_price as f64,
                margin,
                entry_fills: vec![fill],
                exit_fills: Vec::new(),
            });
        }

        commission
    }

    // ── Position closing ──────────────────────────────────────────────────────

    /// Close `close_qty` of the position at `exit_price`.
    /// Returns net pnl, or None if position not found.
    pub fn close_position(
        &mut self,
        position_id: PositionId,
        close_qty: f64,
        exit_price: f32,
    ) -> Option<f64> {
        let pos_idx = self
            .open_positions
            .iter()
            .position(|p| p.id == position_id)?;

        let pos = &mut self.open_positions[pos_idx];
        let actual_qty = close_qty.min(pos.qty);
        let avg_entry = pos.avg_cost;
        let side = pos.side;
        let open_bar = pos.open_bar;
        let released_margin = pos.margin * (actual_qty / pos.qty);

        let commission = exit_price as f64 * actual_qty * self.account.commission_rate();
        let gross_pnl = match side {
            Side::Long => (exit_price as f64 - avg_entry) * actual_qty,
            Side::Short => (avg_entry - exit_price as f64) * actual_qty,
        };
        let net_pnl = gross_pnl - commission;

        pos.exit_fills.push(Fill {
            bar_index: self.current_bar_index,
            price: exit_price,
            qty: actual_qty,
            commission,
        });
        pos.qty -= actual_qty;
        pos.margin -= released_margin;

        self.closed_trades.push(Trade {
            position_id,
            side,
            open_bar,
            close_bar: self.current_bar_index,
            qty: actual_qty,
            avg_entry,
            avg_exit: exit_price as f64,
            gross_pnl,
            commission,
            net_pnl,
        });

        if self.open_positions[pos_idx].qty < 1e-9 {
            self.open_positions.remove(pos_idx);
        }

        self.account.refund_close(released_margin, net_pnl);

        Some(net_pnl)
    }

    // ── Liquidation check (call once per bar) ─────────────────────────────────

    /// Check all open positions for liquidation at `current_price`.
    /// Only relevant for `Account::Futures`; spot accounts are no-ops.
    /// Returns the ids of liquidated positions.
    pub fn check_liquidation(&mut self, current_price: f32) -> Vec<PositionId> {
        let candidates = self
            .account
            .liquidation_candidates(&self.open_positions, current_price);
        if candidates.is_empty() {
            return Vec::new();
        }

        let mut liquidated = Vec::new();

        for pos_id in candidates {
            let still_needed = {
                let unrealized: f64 = self
                    .open_positions
                    .iter()
                    .map(|p| p.unrealized_pnl(current_price))
                    .sum();
                let equity = self.account.cash() + unrealized;
                let total_maintenance: f64 = self.open_positions.iter().map(|p| p.margin).sum();
                equity < total_maintenance
            };
            if !still_needed {
                break;
            }

            let pos_idx = match self.open_positions.iter().position(|p| p.id == pos_id) {
                Some(i) => i,
                None => continue,
            };

            let pos = &self.open_positions[pos_idx];
            let qty = pos.qty;
            let side = pos.side;
            let margin = pos.margin;
            let avg_cost = pos.avg_cost;
            let open_bar = pos.open_bar;

            let commission = current_price as f64 * qty * self.account.commission_rate();
            let gross_pnl = match side {
                Side::Long => (current_price as f64 - avg_cost) * qty,
                Side::Short => (avg_cost - current_price as f64) * qty,
            };
            let net_pnl = gross_pnl - commission;
            let loss = -net_pnl.min(0.0);

            self.open_positions[pos_idx].exit_fills.push(Fill {
                bar_index: self.current_bar_index,
                price: current_price,
                qty,
                commission,
            });
            self.open_positions.remove(pos_idx);

            self.liquidations.push(Liquidation {
                position_id: pos_id,
                bar_index: self.current_bar_index,
                side,
                qty,
                price: current_price,
                loss,
            });

            self.closed_trades.push(Trade {
                position_id: pos_id,
                side,
                open_bar,
                close_bar: self.current_bar_index,
                qty,
                avg_entry: avg_cost,
                avg_exit: current_price as f64,
                gross_pnl,
                commission,
                net_pnl,
            });

            self.account.on_liquidation(margin, net_pnl);
            liquidated.push(pos_id);
        }

        liquidated
    }

    // ── Equity snapshot (call once per bar after fills) ───────────────────────

    pub fn snapshot_equity(&mut self, current_price: f32) {
        let equity = self.equity(current_price);
        self.equity_curve.push(equity);

        if equity > self.peak_equity {
            self.peak_equity = equity;
        }
        let drawdown = self.peak_equity - equity;
        if drawdown > self.max_drawdown {
            self.max_drawdown = drawdown;
        }
    }

    // ── Annotation helpers ────────────────────────────────────────────────────

    pub fn annotate(&mut self, kind: AnnotationKind, price: f32, color: u32) {
        self.annotations.push(Annotation {
            bar_index: self.current_bar_index,
            price,
            color,
            kind,
        });
    }

    pub fn mark_buy(&mut self, price: f32) {
        self.annotate(AnnotationKind::ArrowUp, price, 0xFF00CC44);
    }

    pub fn mark_sell(&mut self, price: f32) {
        self.annotate(AnnotationKind::ArrowDown, price, 0xFFCC2200);
    }

    // ── Convenience queries ───────────────────────────────────────────────────

    pub fn equity(&self, current_price: f32) -> f64 {
        let unrealized: f64 = self
            .open_positions
            .iter()
            .map(|p| p.unrealized_pnl(current_price))
            .sum();
        self.account.cash() + unrealized
    }

    pub fn margin_used(&self) -> f64 {
        self.open_positions.iter().map(|p| p.margin).sum()
    }

    pub fn position_by_id(&self, id: PositionId) -> Option<&Position> {
        self.open_positions.iter().find(|p| p.id == id)
    }

    pub fn order_by_id(&self, id: OrderId) -> Option<&Order> {
        self.open_orders
            .get(&id)
            .or_else(|| self.closed_orders.iter().find(|o| o.id == id))
    }

    pub fn long_positions(&self) -> impl Iterator<Item = &Position> {
        self.open_positions.iter().filter(|p| p.side == Side::Long)
    }

    pub fn short_positions(&self) -> impl Iterator<Item = &Position> {
        self.open_positions.iter().filter(|p| p.side == Side::Short)
    }
}

impl Default for BacktestCtx {
    fn default() -> Self {
        Self::new(Account::Spot(SpotAccount::new(100_000.0, 0.001, 0.0)), 0)
    }
}
