//! Core order-system domain model.
//!
//! This module groups the primary building blocks needed to model an
//! audit-friendly trading workflow:
//!
//! - [`order`] describes client intent and the order state machine.
//! - [`fill`] stores immutable execution facts reported by a venue or simulator.
//! - [`lot`] stores tax-lot style position fragments and realized PnL history.
//! - [`holding`] aggregates many lots under one symbol and applies lot-relief rules.
//! - [`execution_report`] snapshots order state for downstream consumers.
//!
//! The identifiers defined in this module are intentionally strong types so that
//! order, fill, holding, lot, and execution-report references cannot be mixed up
//! accidentally at compile time.

/// Execution report snapshots derived from order-state transitions.
pub mod execution_report;
/// Immutable execution facts such as price, quantity, fees, and venue metadata.
pub mod fill;
/// A per-symbol aggregate that owns lots and applies lot-relief policies.
pub mod holding;
/// Tax-lot style position fragments with detailed open/close audit history.
pub mod lot;
/// Order intent and order lifecycle state transitions.
pub mod order;

macro_rules! impl_id {
    ($name:ident) => {
        #[doc = concat!(
                            "Strongly typed identifier wrapper for `",
                            stringify!($name),
                            "` values inside `order_sys`."
                        )]
        #[repr(transparent)]
        #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(u64);

        impl $name {
            #[doc = "Creates a new identifier from a raw numeric value."]
            pub const fn new(value: u64) -> Self {
                Self(value)
            }

            #[doc = "Returns the underlying numeric identifier value."]
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
