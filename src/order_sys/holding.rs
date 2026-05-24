use crate::order_sys::lot::Lot;

pub struct Holding {
    pub id: u64,
    pub symbol: String,
    pub lots: Vec<Lot>,
}
