use chrono::{DateTime, Utc};
use rustc_hash::FxHashMap;

use crate::bar::Point;

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
    pub qty: f64,
    /// Commission paid for the fill
    pub commission: f64,
}

// ── Position ──────────────────────────────────────────────────────────────────
#[derive(Debug, Clone)]
pub struct Position {
    pub id: PositionId,
    pub side: Side,
    pub open_date: DateTime<Utc>,
    pub qty: f64,
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
    pub current_bar_index: usize,

    next_position_id: PositionId,
}

impl Strategy {
    pub fn new(account: Account) -> Self {
        Self {
            account,
            positions: Vec::new(),
            current_bar_index: 0,
            next_position_id: 1,
        }
    }

    // ── Order submission ──────────────────────────────────────────────────────
    pub fn open_position(&mut self, side: Side, qty: f64) -> PositionId {
        let id = self.next_position_id;
        self.next_position_id += 1;
        id
    }
}
