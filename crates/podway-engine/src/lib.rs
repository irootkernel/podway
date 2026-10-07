#![forbid(unsafe_code)]

//! Host-independent Procedure domain services and native workspace boundaries.

pub mod direct_api;
pub mod direct_delegation;
pub mod direct_error;
pub mod direct_execution;
pub mod direct_items;
pub mod direct_maintenance;
mod direct_mutation;
pub mod direct_procedure;
pub mod direct_read_service;
pub mod direct_sessions;
pub mod direct_start;
pub mod direct_workspace;
pub mod execution;
pub mod native_execution;
pub mod v2_read_service;
pub mod workspace;
