#![allow(irrefutable_let_patterns)]

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
#[path = "int_phase4_endpoint.rs"]
mod int_phase4_endpoint;
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
#[path = "int_phase5_reset_marker.rs"]
mod int_phase5_reset_marker;
#[path = "int_phase8_observability.rs"]
mod int_phase8_observability;
#[path = "int_v2_only_reset_recovery.rs"]
mod int_v2_only_reset_recovery;
#[path = "int_v2dog003_embedded_presets.rs"]
mod int_v2dog003_embedded_presets;
#[path = "int_v2drw001_decide.rs"]
mod int_v2drw001_decide;
#[path = "int_v2drw003_rework.rs"]
mod int_v2drw003_rework;
#[path = "int_v2drw005_readback.rs"]
mod int_v2drw005_readback;
#[path = "int_v2drw006_failures.rs"]
mod int_v2drw006_failures;
#[path = "int_v2drw_epic_identity.rs"]
mod int_v2drw_epic_identity;
#[path = "int_v2gol001_goal_revision.rs"]
mod int_v2gol001_goal_revision;
#[path = "int_v2gol002_criterion_assessment.rs"]
mod int_v2gol002_criterion_assessment;
#[path = "int_v2gol003_goal_outcomes.rs"]
mod int_v2gol003_goal_outcomes;
#[path = "int_v2gol004_goal_readback.rs"]
mod int_v2gol004_goal_readback;
#[path = "int_v2gol005_failures.rs"]
mod int_v2gol005_failures;
#[path = "int_v2gol_epic_acceptance.rs"]
mod int_v2gol_epic_acceptance;
#[path = "int_v2rel002_maximum_next.rs"]
mod int_v2rel002_maximum_next;
#[path = "int_v2run001_start.rs"]
mod int_v2run001_start;
#[path = "int_v2run002_views.rs"]
mod int_v2run002_views;
#[path = "int_v2run003_runtime.rs"]
mod int_v2run003_runtime;
#[path = "int_v2run004_retry.rs"]
mod int_v2run004_retry;
#[path = "int_v2run005_skip.rs"]
mod int_v2run005_skip;
#[path = "int_v2run006_states.rs"]
mod int_v2run006_states;
#[path = "int_v2run007_preconditions.rs"]
mod int_v2run007_preconditions;
#[path = "int_v2run008_recovery.rs"]
mod int_v2run008_recovery;
