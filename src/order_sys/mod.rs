pub mod execution_report;
pub mod fill;
pub mod holding;
pub mod lot;
pub mod order;

macro_rules! impl_id {
    ($name:ident) => {
        #[repr(transparent)]
        #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(u64);

        impl $name {
            pub const fn new(value: u64) -> Self {
                Self(value)
            }

            pub const fn value(self) -> u64 {
                self.0
            }
        }
    };
}

impl_id!(HoldingId);
impl_id!(LotId);
impl_id!(OrderId);
impl_id!(FillId);
impl_id!(ExecutionReportId);
