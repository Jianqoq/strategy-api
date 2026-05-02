pub mod bar;
pub mod ctx;
pub mod strategy;

pub use bar::Bar;
pub use ctx::BacktestCtx;
pub use strategy::{
    BUY, FnOnBar, FnOnFinish, FnOnInit, FnStrategyInfo, HOLD, SELL, Signal, Strategy, StrategyInfo,
};
