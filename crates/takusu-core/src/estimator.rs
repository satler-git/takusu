//! Compatibility facade for the shared duration-distribution estimator.
//!
//! The implementation lives in `takusu-types/src/estimator.rs` so it can be
//! shared by clients and the server without a `takusu-core` dependency in
//! `takusu-types`.

pub use takusu_types::estimator::*;
