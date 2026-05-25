pub mod bar;
pub mod indicator;
pub mod matcher;
pub mod order_sys;
pub mod strategy;

pub use bar::{Bar, Point};
pub use matcher::{
    BarMarketFillPrice, BarMatcher, BarMatcherConfig, MatchError, MatchResult, PriceTrigger, Tick,
    TickError, TickMatcher, TickMatcherConfig,
};

pub use strategy::{
    BUY, FnOnIndicatorBar, FnOnIndicatorFinish, FnOnInit, FnOnRestoreState, FnOnStrategyBar,
    FnOnStrategyFinish, HOLD, OnIndicator, OnStrategy, OrderKind, SELL, Side, Signal, StateBlob,
    Strategy, StrategyError,
};
