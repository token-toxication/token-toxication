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
//! TT proxies the third into the second, so upstream is the single source of
//! truth for which models exist, their context windows, their service tiers, and
//! their instructions. TT filters only where a client could not accept an entry
//! at all, and authors nothing.
//!
//! What that costs a client: a catalog supplied through `model_catalog_url`
//! replaces the bundled one, so an unusable catalog means no models rather than
//! degraded ones. The modules here are built around that: every entry the client
//! would reject is dropped before the body is served, the body is kept under the
//! client's size cap, and a transient upstream failure serves the last known
//! good body instead of an error.

pub mod alias;
pub mod cache;
pub mod remote;
pub mod upstream;
