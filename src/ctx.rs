use std::time::Duration;

use chrono::{DateTime, Utc};
use rustc_hash::FxHashMap;

use crate::{Bar, bar::Point};

// ── Identifiers ───────────────────────────────────────────────────────────────
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

#[derive(Debug, Clone)]
pub struct Fill {
    /// Date and time of the fill
    pub date: DateTime<Utc>,
    /// Price at which the fill was executed
    pub price: f64,
    /// Quantity of the fill
    /// Date and time of the fill
    pub date: DateTime<Utc>,
    /// Price at which the fill was executed
    pub price: f64,
    /// Quantity of the fill
    pub qty: f64,
    /// Commission paid for the fill
    /// Commission paid for the fill
    pub commission: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Open,
    Closing,
    Closed,
}

// ── Position ──────────────────────────────────────────────────────────────────
#[derive(Debug, Clone)]
pub struct Position {
    pub id: PositionId,
    pub side: Side,
    pub open_date: DateTime<Utc>,
    /// the price at which the position was filled
    pub fill_price: f64,
    /// the target quantity of the position
    pub fill_qty: f64,
    pub status: Status,
    /// the date and time until which the position is valid
    pub valid_date: DateTime<Utc>,
    /// record fills while the position is in open state
    pub fills: Vec<Fill>,
    /// record fills while the position is in execution state
    pub closed_fills: Vec<Fill>,
}

impl Position {
    pub fn unrealized_pnl(&self, current_price: f32) -> f64 {
        unimplemented!()
    }
}

pub struct Indicator {
    pub series: FxHashMap<&'static str, Vec<Point>>,
}

// ── StrategyCtx ───────────────────────────────────────────────────────────────
#[derive(Debug, Clone)]
pub struct Strategy {
    pub account: Account,
    // Open positions (Long and Short can coexist)
    pub positions: Vec<Position>,
    // Progress (set by host before each on_bar call)
    pub current_bar: Bar,

    next_position_id: PositionId,
}

impl Strategy {
    pub fn new(account: Account) -> Self {
    pub fn new(account: Account) -> Self {
        Self {
            account,
            positions: Vec::new(),
            current_bar: Bar::default(),
            next_position_id: 1,
        }
    }

    // ── Order submission ──────────────────────────────────────────────────────
    pub fn open_position(
        &mut self,
        side: Side,
        price: f64,
        qty: f64,
        duration: Duration,
    ) -> PositionId {
        let id = self.next_position_id;
        self.next_position_id += 1;
        self.positions.push(Position {
            id,
            side,
            open_date: self.current_bar.time,
            fill_price: price,
            fill_qty: qty,
            valid_date: self.current_bar.time + duration,
            fills: Vec::new(),
            closed_fills: Vec::new(),
            status: Status::Open,
        });
        id
    }
}
