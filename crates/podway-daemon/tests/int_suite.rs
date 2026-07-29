#[path = "../src/observability.rs"]
#[allow(dead_code)]
mod observability;
#[path = "../src/registry.rs"]
#[allow(dead_code)]
mod registry_under_test;
#[path = "support/phase4_workspace.rs"]
#[allow(dead_code)]
mod support_phase4_workspace;

#[path = "int_phase4_blocking.rs"]
mod int_phase4_blocking;
#[path = "int_phase4_daemon_binary.rs"]
mod int_phase4_daemon_binary;
#[path = "int_phase4_daemon_runtime.rs"]
mod int_phase4_daemon_runtime;
#[path = "int_phase4_dispatch.rs"]
mod int_phase4_dispatch;
#[path = "int_phase4_endpoint.rs"]
mod int_phase4_endpoint;
#[path = "int_phase4_execution.rs"]
mod int_phase4_execution;
#[path = "int_phase4_native_execution.rs"]
mod int_phase4_native_execution;
#[path = "int_phase4_production.rs"]
mod int_phase4_production;
#[path = "int_phase4_read_service.rs"]
mod int_phase4_read_service;
#[path = "int_phase4_registry.rs"]
mod int_phase4_registry;
#[path = "int_phase4_runtime_workspace.rs"]
mod int_phase4_runtime_workspace;
#[path = "int_phase4_scheduler.rs"]
mod int_phase4_scheduler;
#[path = "int_phase4_server.rs"]
mod int_phase4_server;
#[path = "int_phase4_worker.rs"]
mod int_phase4_worker;
#[path = "int_phase4_workspace.rs"]
mod int_phase4_workspace;
#[path = "int_phase5_dispatch.rs"]
mod int_phase5_dispatch;
#[path = "int_phase5_execution.rs"]
mod int_phase5_execution;
#[path = "int_phase5_reset_marker.rs"]
mod int_phase5_reset_marker;
#[path = "int_phase5_reset_runtime.rs"]
mod int_phase5_reset_runtime;
#[path = "int_phase8_observability.rs"]
mod int_phase8_observability;
