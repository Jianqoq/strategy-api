pub struct BacktestCtx {}

impl BacktestCtx {
    pub fn new() -> Self {
        Self {}
    }
}

impl Default for BacktestCtx {
    fn default() -> Self {
        Self::new()
    }
}
