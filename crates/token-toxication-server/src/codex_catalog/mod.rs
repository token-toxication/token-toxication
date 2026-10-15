//! Codex subscription model catalog support.
//!
//! The relay distinguishes three catalogs that must not be conflated:
//!
//! - the relay route catalog (`model_catalog` + `provider_model_routes`), which
//!   decides routing and upstream model rewriting;
//! - the client capability catalog, which describes model metadata to clients
//!   such as Codex and is implemented here;
//! - the upstream account catalog, which comes from a specific subscription
//!   account and is never authoritative for every account.
//!
//! This module starts with the pure eligibility and limit policy shared by the
//! static profile path and the future upstream catalog resolver.

pub mod policy;
pub mod remote;
pub mod upstream;
