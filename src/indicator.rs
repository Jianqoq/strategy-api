use rustc_hash::FxHashMap;

use crate::bar::Point;

pub struct Indicator {
    pub series: FxHashMap<&'static str, Vec<Point>>,
}