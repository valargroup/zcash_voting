//! Behavior tests for proxy delegation, grouped by what they pin.

mod fixtures;
pub(crate) mod submission_fixtures;

mod account_deletion;
mod allocation_lifecycle;
mod availability;
mod batch_attempt;
mod batch_plan;
mod delegator_calls;
mod packing;
mod plan_properties;
mod request_admission;
mod rounding;
mod secrets;
mod setup_freeze;
mod slot_positions;
