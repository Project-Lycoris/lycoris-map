//! Place reads, localization, caching, writes, and moderation.
//!
//! [`model`] separates stored rows from public DTOs. [`repository`] owns fixed SQL;
//! [`service`] applies read visibility and localization. [`write`] owns transactional
//! changes and review decisions. [`http`] and [`write_http`] keep transport concerns
//! and authentication at the route boundary.

pub mod cache;
pub mod http;
pub mod localization;
pub mod model;
pub mod moderation;
pub mod repository;
pub mod service;
pub mod write;
pub mod write_http;
pub mod write_model;
