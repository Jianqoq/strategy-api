/// ABI-stable bar passed across the DLL boundary.
///
/// `timestamp_ms` is milliseconds since Unix epoch (UTC).
/// All prices are f32 to match the internal Bar layout.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct Bar {
    pub timestamp_ms: i64,
    pub open: f32,
    pub high: f32,
    pub low: f32,
    pub close: f32,
    pub volume: f64,
}

pub struct Point {
    pub index: usize,
    pub price: f32,
    pub color: [u8; 4],
}
