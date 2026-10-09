//! Public direct failures retain domain categories without exposing infrastructure diagnostics.

use std::sync::OnceLock;

use podway_core::{DomainError, SessionLifecycle};
use podway_protocol::{
    CommandNameV1, ErrorCodeV1, ErrorEnvelopeInputV2, ErrorEnvelopeV2, ExitCodeV1, ProtocolError,
    RecoveryRecipeV2, RequestIdV1, Rfc3339MillisV1,
};
use podway_store::{
    GraphMutationErrorV2, IdempotencyKeyV1, StoreErrorV1, direct_store::DirectStoreErrorV1,
};
use serde_json::{Value, json};

use crate::{
    direct_execution::{DirectDelegationFailureV1, DirectExecutionErrorV1},
    execution::{
        ExecutionBoundaryErrorV1, ExecutionErrorV1, ProcedureV2SourceAdmissionErrorV1,
        ProcedureV2StartPreparationErrorV1,
    },
    v2_read_service::{EvidenceReadErrorV2, GraphViewErrorV2},
    workspace::{DirectWorkspaceResolutionErrorV1, WorkspaceResolutionErrorV1},
};

/// Error rendering uses the host's actual request observation time, independently of the
/// mutation clock. A rejected transition or an exact replay never reads that mutation clock.
#[derive(Clone, Debug)]
pub struct DirectErrorContextV1 {
    pub request_id: RequestIdV1,
    pub command: CommandNameV1,
    pub observed_at: Rfc3339MillisV1,
    pub idempotency_key: Option<IdempotencyKeyV1>,
    pub timeout_ms: u64,
    pub expected_revision: Option<podway_core::Revision>,
    pub maintenance: Option<DirectMaintenanceRecoveryV1>,
}

#[derive(Clone, Debug)]
pub enum DirectMaintenanceRecoveryV1 {
    Reset {
        key: IdempotencyKeyV1,
        workspace: Option<podway_core::WorkspaceId>,
    },
    Removal {
        key: IdempotencyKeyV1,
        workspace: Option<podway_core::WorkspaceId>,
        force: bool,
    },
}

struct Presentation {
    code: &'static str,
    details: Value,
}

fn simple(code: &'static str) -> Presentation {
    Presentation {
        code,
        details: json!({}),
    }
}

fn refresh() -> Value {
    json!({"action":"refresh_state", "command":"session.observe", "argv":["podway","--json","observe"],
        "reason":"Read the current committed state before retrying.", "requires_explicit_authorization":false})
}

fn diagnose() -> Value {
    json!({"action":"diagnose_workspace", "command":"workspace.doctor", "argv":["podway","--json","doctor"],
        "reason":"Inspect the worktree-local Store and its binding.", "requires_explicit_authorization":false})
}

fn recovery(code: &'static str) -> Presentation {
    Presentation {
        code,
        details: json!({"schema":"podway.workspace-recovery-details/v2", "recovery":diagnose()}),
    }
}

fn runtime(code: &'static str, mut fields: Value) -> Presentation {
    let recoverable = matches!(
        code,
        "EVIDENCE_REFERENCE_STALE" | "GOAL_REVISION_STALE" | "EVIDENCE_PAGE_TOKEN_STALE"
    );
    fields["schema"] = json!(if recoverable {
        "podway.recoverable-v2-runtime-error-details/v2"
    } else {
        "podway.v2-runtime-error-details/v2"
    });
    fields["kind"] = json!(code);
    if recoverable {
        fields["recovery"] = refresh();
    }
    Presentation {
        code,
        details: fields,
    }
}

fn revision(
    code: &'static str,
    expected: impl serde::Serialize,
    actual: impl serde::Serialize,
) -> Presentation {
    Presentation {
        code,
        details: json!({"schema":"podway.revision-conflict-details/v3",
        "expected_revision":expected, "current_revision":actual, "recovery":refresh()}),
    }
}

fn session(expected: impl serde::Serialize, actual: impl serde::Serialize) -> Presentation {
    Presentation {
        code: "SESSION_ID_MISMATCH",
        details: json!({"schema":"podway.session-id-mismatch-details/v3",
        "expected_session_id":expected,"actual_session_id":actual,"recovery":refresh()}),
    }
}

fn workspace(expected: impl serde::Serialize, actual: impl serde::Serialize) -> Presentation {
    Presentation {
        code: "WORKSPACE_UUID_MISMATCH",
        details: json!({"schema":"podway.workspace-uuid-mismatch-details/v3",
        "expected_workspace_uuid":expected,"actual_workspace_uuid":actual,"recovery":diagnose()}),
    }
}

fn attempt(expected: impl serde::Serialize, actual: impl serde::Serialize) -> Presentation {
    let mut details = json!({"schema":"podway.attempt-conflict-details/v3",
        "expected_attempt_id":expected,"actual_attempt_id":actual,"recovery":refresh()});
    if details["actual_attempt_id"].is_null() {
        details
            .as_object_mut()
            .expect("details are an object")
            .remove("actual_attempt_id");
    }
    Presentation {
        code: "ATTEMPT_NOT_CURRENT",
        details,
    }
}

fn digest(expected: impl serde::Serialize, actual: impl serde::Serialize) -> Presentation {
    Presentation {
        code: "PROCEDURE_DIGEST_MISMATCH",
        details: json!({"schema":"podway.procedure-digest-mismatch-details/v3",
        "expected_procedure_digest":expected,"actual_procedure_digest":actual,"recovery":refresh()}),
    }
}

fn delegation(code: &'static str, reason: &'static str) -> Presentation {
    Presentation {
        code,
        details: json!({"schema":"podway.delegation-error-details/v2","reason":reason}),
    }
}

fn lifecycle(value: SessionLifecycle) -> &'static str {
    match value {
        SessionLifecycle::Prepared => "prepared",
        SessionLifecycle::Running => "running",
        SessionLifecycle::Completed => "completed",
        SessionLifecycle::Cancelled => "cancelled",
    }
}

fn allowed_actions(value: SessionLifecycle, disposed: bool) -> &'static [&'static str] {
    match (value, disposed) {
        (SessionLifecycle::Prepared, _) => &["continue", "delete"],
        (SessionLifecycle::Running, _) => &["continue", "preserve", "delete"],
        (_, false) => &["continue", "record_disposition"],
        (_, true) => &["start"],
    }
}

fn domain(error: &DomainError, context: &DirectErrorContextV1) -> Presentation {
    match error {
        DomainError::SessionIdentityMismatch { expected, actual } => session(expected, actual),
        DomainError::AttemptNotCurrent { expected, actual } => attempt(expected, actual),
        DomainError::PreconditionFailed { expected, actual } => revision(
            if context.command.as_str().starts_with("item.")
                && context.command.as_str() != "item.record_many"
            {
                "ITEM_REVISION_CONFLICT"
            } else {
                "SESSION_REVISION_CONFLICT"
            },
            expected,
            actual,
        ),
        DomainError::BoundExceeded {
            field,
            actual,
            maximum,
            unit,
        } => runtime(
            if *field == podway_core::ATTEMPT_CONTENT_FIELD_V2
                && *unit == podway_core::BoundUnitV2::Scalars
            {
                "ATTEMPT_CONTENT_LIMIT_EXCEEDED"
            } else {
                "ITEM_CONSTRAINT_FAILED"
            },
            json!({"field":field,"actual":actual,"maximum":maximum,"unit":unit.as_str()}),
        ),
        DomainError::RequiredItemsMissing => simple("REQUIRED_ITEMS_MISSING"),
        DomainError::BlockersPresent => simple("BLOCKERS_PRESENT"),
        DomainError::BlockerLimitReached {
            maximum_open_blockers,
        } => Presentation {
            code: "BLOCKER_LIMIT_REACHED",
            details: json!({"schema":"podway.blocker-limit-details/v2","maximum_open_blockers":maximum_open_blockers}),
        },
        DomainError::ArtifactChanged => simple("ARTIFACT_CHANGED"),
        DomainError::ItemNotFound { .. } => simple("ITEM_NOT_FOUND"),
        DomainError::BlockerNotCurrent => simple("BLOCKER_NOT_CURRENT"),
        DomainError::InvalidState { .. } => simple("REQUEST_INVALID"),
        DomainError::InvalidTransition { .. } => {
            simple(if context.command.as_str() == "session.skip" {
                "STAGE_NOT_SKIPPABLE"
            } else {
                "SESSION_NOT_RUNNING"
            })
        }
        DomainError::RevisionOverflow { .. } => simple("INTERNAL_ERROR"),
        DomainError::EmptyValue { .. }
        | DomainError::ValueTooLong { .. }
        | DomainError::InvalidUuid { .. }
        | DomainError::InvalidIdentifier { .. }
        | DomainError::InvalidSha256Digest => simple("REQUEST_INVALID"),
    }
}

fn graph(error: &GraphMutationErrorV2, context: &DirectErrorContextV1) -> Presentation {
    use GraphMutationErrorV2 as E;
    match error {
        E::Domain(error) => domain(error, context),
        E::InvalidState(_) => simple("INTERNAL_ERROR"),
        E::GoalTrackingNotEnabled => runtime("GOAL_TRACKING_NOT_ENABLED", json!({})),
        E::SessionGoalAlreadyDefined { goal_revision } => runtime(
            "SESSION_GOAL_ALREADY_DEFINED",
            json!({"goal_revision":goal_revision.get()}),
        ),
        E::GoalRevisionStale { expected, actual } => runtime(
            "GOAL_REVISION_STALE",
            json!({"expected_goal_revision":expected.get(),"actual_goal_revision":actual.get()}),
        ),
        E::GoalRevisionTargetNotAllowed {
            target_graph_node_id,
        } => runtime(
            "GOAL_REVISION_TARGET_NOT_ALLOWED",
            json!({"target_graph_node_id":target_graph_node_id}),
        ),
        E::GoalRevisionTargetNotRevisionSafe {
            target_graph_node_id,
        } => runtime(
            "GOAL_REVISION_TARGET_NOT_REVISION_SAFE",
            json!({"target_graph_node_id":target_graph_node_id}),
        ),
        E::ReactivationFlagRequired => runtime("REACTIVATION_FLAG_REQUIRED", json!({})),
        E::SessionNotPrepared => simple(if context.command.as_str() == "session.begin" {
            "SESSION_NOT_RUNNING"
        } else {
            "REQUEST_INVALID"
        }),
        E::SessionNotRunning => simple("SESSION_NOT_RUNNING"),
        E::SessionRevisionConflict { expected, actual } => {
            revision("SESSION_REVISION_CONFLICT", expected, actual)
        }
        E::AttemptNotCurrent { expected, actual } => attempt(expected, actual),
        E::GraphNodeTypeMismatch { .. } if context.command.as_str() == "session.skip" => {
            simple("STAGE_NOT_SKIPPABLE")
        }
        E::GraphNodeTypeMismatch {
            graph_node_id,
            actual,
        } => runtime(
            "GRAPH_NODE_TYPE_MISMATCH",
            json!({"graph_node_id":graph_node_id,
            "actual_node_type":actual.as_str(),"expected_node_type":if context.command.as_str() == "session.decide" { "decision" } else { "action" }}),
        ),
        E::OptionNotAllowed {
            graph_node_id,
            option_id,
            allowed_option_ids,
        } => runtime(
            "OPTION_NOT_ALLOWED",
            json!({"graph_node_id":graph_node_id,"option_id":option_id,"allowed_option_ids":allowed_option_ids}),
        ),
        E::OptionGuardUnsatisfied {
            graph_node_id,
            option_id,
            state,
            predicates,
        } => runtime(
            "OPTION_GUARD_UNSATISFIED",
            json!({"graph_node_id":graph_node_id,"option_id":option_id,"state":state.as_str(),"predicates":predicates}),
        ),
        E::RouteNotAllowed {
            graph_node_id,
            option_id,
        } => runtime(
            "ROUTE_NOT_ALLOWED",
            json!({"graph_node_id":graph_node_id,"option_id":option_id}),
        ),
        E::DecisionReasonMissing { graph_node_id } => runtime(
            "DECISION_REASON_MISSING",
            json!({"graph_node_id":graph_node_id}),
        ),
        E::EvidenceReferenceUnresolved {
            graph_node_id,
            source_graph_node_ids,
        } => runtime(
            "EVIDENCE_REFERENCE_UNRESOLVED",
            json!({"graph_node_id":graph_node_id,"source_graph_node_ids":source_graph_node_ids}),
        ),
        E::EvidenceReferenceStale {
            graph_node_id,
            source_graph_node_id,
            expected_source_attempt_id,
            current_source_attempt_id,
        } => runtime(
            "EVIDENCE_REFERENCE_STALE",
            json!({"graph_node_id":graph_node_id,"source_graph_node_id":source_graph_node_id,"expected_source_attempt_id":expected_source_attempt_id,"current_source_attempt_id":current_source_attempt_id}),
        ),
        E::GoalAssessmentDecisionRequiresAssessment { .. }
        | E::GoalAssessmentDecisionRequired { .. } => simple("REQUEST_INVALID"),
        E::CriterionNotFound { criterion_id } => {
            runtime("CRITERION_NOT_FOUND", json!({"criterion_id":criterion_id}))
        }
        E::CriterionResultAlreadyRecorded { .. } => simple("REQUEST_INVALID"),
        E::CriterionModeMixed {
            criterion_id,
            expected_mode,
            actual_status,
        } => runtime(
            "CRITERION_MODE_MIXED",
            json!({"criterion_id":criterion_id,"expected_mode":expected_mode.as_str(),"actual_status":actual_status.as_str()}),
        ),
        E::CriterionCitationInvalid {
            criterion_id,
            citation,
        } => runtime(
            "CRITERION_CITATION_INVALID",
            json!({"criterion_id":criterion_id,"citation":match citation { podway_core::CriterionCitationV2::Evidence(node) => json!({"reference_graph_node_id":node}), podway_core::CriterionCitationV2::Item(item) => json!({"local_item_id":item}) }}),
        ),
        E::CriterionResultMissing {
            missing_criterion_ids,
        } => runtime(
            "CRITERION_RESULT_MISSING",
            json!({"missing_criterion_ids":missing_criterion_ids}),
        ),
        E::GoalAssessmentOutcomeNotAllowed {
            option_id,
            determined_outcome,
            allowed_option_ids,
        } => runtime(
            "GOAL_ASSESSMENT_OUTCOME_NOT_ALLOWED",
            json!({"option_id":option_id,"determined_outcome":determined_outcome.as_str(),"allowed_option_ids":allowed_option_ids}),
        ),
        E::SessionCancelled => simple("SESSION_CANCELLED"),
        E::ManualReworkTargetNotAllowed {
            target_graph_node_id,
        } => runtime(
            "MANUAL_REWORK_TARGET_NOT_ALLOWED",
            json!({"target_graph_node_id":target_graph_node_id}),
        ),
        E::ManualReworkTargetNotOnTrace {
            target_graph_node_id,
        } => runtime(
            "MANUAL_REWORK_TARGET_NOT_ON_TRACE",
            json!({"target_graph_node_id":target_graph_node_id}),
        ),
        E::SkipNotAllowed { .. } => simple("STAGE_NOT_SKIPPABLE"),
        E::SkipReasonRequired { .. } => simple("STAGE_NOT_SKIPPABLE"),
        E::BlockerIdAlreadyUsed { .. } | E::BlockerAlreadyResolved { .. } => {
            simple("REQUEST_INVALID")
        }
        E::TooManyOpenBlockers { maximum } => Presentation {
            code: "BLOCKER_LIMIT_REACHED",
            details: json!({"schema":"podway.blocker-limit-details/v2","maximum_open_blockers":maximum}),
        },
        E::BlockerNotFound { .. } => simple("BLOCKER_NOT_FOUND"),
        E::BlockerNotCurrent { .. } => simple("BLOCKER_NOT_CURRENT"),
        E::NoOpenBlockers => simple("REQUEST_INVALID"),
        E::ItemMutationBatchEmpty
        | E::TooManyItemMutations { .. }
        | E::DuplicateItemMutation { .. } => simple("REQUEST_INVALID"),
        E::ItemNotFound { .. } => simple("ITEM_NOT_FOUND"),
        E::ItemRevisionConflict { expected, actual } => {
            revision("ITEM_REVISION_CONFLICT", expected, actual)
        }
        E::ItemTypeMismatch => simple("ITEM_TYPE_MISMATCH"),
        E::ItemConstraintFailed => simple("ITEM_CONSTRAINT_FAILED"),
        E::ListValueNotFound => simple("LIST_VALUE_NOT_FOUND"),
        E::ListValueDuplicate => simple("LIST_VALUE_DUPLICATE"),
        E::RequiredItemsMissing { item_ids } => Presentation {
            code: "REQUIRED_ITEMS_MISSING",
            details: json!({"item_ids":item_ids}),
        },
        E::BlockersPresent => simple("BLOCKERS_PRESENT"),
        E::SessionGoalMissing => runtime("SESSION_GOAL_MISSING", json!({})),
        E::FreshGoalAssessmentMissing { goal_revision } => runtime(
            "FRESH_GOAL_ASSESSMENT_MISSING",
            json!({"goal_revision":goal_revision.get()}),
        ),
    }
}

fn state(error: &StoreErrorV1, context: &DirectErrorContextV1) -> Presentation {
    match error {
        StoreErrorV1::SessionResetNotEligibleV1 {
            lifecycle: value,
            current_terminal_disposition,
        } => Presentation {
            code: "SESSION_RESET_NOT_ELIGIBLE",
            details: json!({"schema":"podway.session-reset-not-eligible-details/v2", "lifecycle":lifecycle(*value),
                "required_action":if *value == SessionLifecycle::Running { "force" } else { "record_disposition" },
                "current_terminal_disposition":current_terminal_disposition}),
        },
        StoreErrorV1::SessionArchiveNotEligibleV1 {
            lifecycle: value,
            current_terminal_disposition,
        } => Presentation {
            code: "SESSION_ARCHIVE_NOT_ELIGIBLE",
            details: json!({"schema":"podway.session-archive-not-eligible-details/v2", "lifecycle":lifecycle(*value),
                "required_action":if matches!(value, SessionLifecycle::Prepared | SessionLifecycle::Running) { "complete_or_cancel" } else { "record_disposition" },
                "current_terminal_disposition":current_terminal_disposition}),
        },
        StoreErrorV1::SessionArchiveLimitReachedV1 { maximum } => Presentation {
            code: "SESSION_ARCHIVE_LIMIT_REACHED",
            details: json!({"schema":"podway.session-archive-limit-details/v2","maximum":maximum}),
        },
        StoreErrorV1::SessionArchiveNotFoundV1 { session_id } => Presentation {
            code: "SESSION_ARCHIVE_NOT_FOUND",
            details: json!({"schema":"podway.session-archive-not-found-details/v2","session_id":session_id}),
        },
        StoreErrorV1::SessionStartStateConflictV1 {
            lifecycle: value,
            current_terminal_disposition,
        } => Presentation {
            code: "SESSION_START_STATE_CONFLICT",
            details: json!({"schema":"podway.session-start-state-conflict-details/v2", "lifecycle":lifecycle(*value),
                "current_terminal_disposition":current_terminal_disposition,"allowed_actions":allowed_actions(*value,*current_terminal_disposition)}),
        },
        StoreErrorV1::SessionIdentityConflictV1 {
            expected: Some(expected),
            actual,
        } => session(expected, actual),
        StoreErrorV1::SessionIdentityConflictV1 {
            expected: None,
            actual: Some(_),
        } => simple("SESSION_ALREADY_EXISTS"),
        StoreErrorV1::PreconditionConflictV1 {
            expected: Some(expected),
            actual: Some(actual),
        } => revision("SESSION_REVISION_CONFLICT", expected, actual),
        StoreErrorV1::TerminalDispositionAlreadyRecordedV1 { .. } => simple("REQUEST_INVALID"),
        StoreErrorV1::StorageUnavailableV1 { .. } | StoreErrorV1::CorruptStateV1 { .. } => {
            recovery("WORKSPACE_STATE_UNREADABLE")
        }
        StoreErrorV1::InternalInvariantViolationV1 { .. }
        | StoreErrorV1::InvalidStateV1(_)
        | StoreErrorV1::PreconditionConflictV1 { .. }
        | StoreErrorV1::SessionIdentityConflictV1 {
            expected: None,
            actual: None,
        } => {
            let _ = context;
            simple("INTERNAL_ERROR")
        }
    }
}

fn maintenance_argv(
    removal: bool,
    key: &IdempotencyKeyV1,
    workspace: Option<&podway_core::WorkspaceId>,
    force: bool,
) -> Vec<String> {
    let mut argv = if removal {
        vec!["podway", "--json", "workspace", "remove"]
    } else {
        vec!["podway", "--json", "reset", "--all"]
    }
    .into_iter()
    .map(str::to_owned)
    .collect::<Vec<_>>();
    if force {
        argv.push("--force".into());
    }
    argv.push("--yes".into());
    if let Some(workspace) = workspace {
        argv.extend(["--if-workspace-uuid".into(), workspace.as_str().into()]);
    }
    argv.extend(["--idempotency-key".into(), key.as_str().into()]);
    argv
}

fn store(error: &DirectStoreErrorV1, context: &DirectErrorContextV1) -> Presentation {
    use DirectStoreErrorV1 as E;
    match error {
        E::NotInitialized => simple("WORKSPACE_NOT_INITIALIZED"),
        E::Maintenance => {
            let (boundary, recipe) = match &context.maintenance {
                Some(DirectMaintenanceRecoveryV1::Reset { key, workspace }) => (
                    "reset_marker",
                    json!({
                    "action":"resume_maintenance","command":"workspace.reset_all",
                    "argv":maintenance_argv(false,key,workspace.as_ref(),false),
                    "reason":"Resume the published reset with its original idempotency key.","requires_explicit_authorization":true}),
                ),
                Some(DirectMaintenanceRecoveryV1::Removal {
                    key,
                    workspace,
                    force,
                }) => (
                    "removal_marker",
                    if workspace.is_none() || !force {
                        let mut recipe = diagnose();
                        recipe["reason"] = json!(
                            "The original typed removal lacks the UUID fence or force required by the JSON CLI; retry the original typed request."
                        );
                        recipe
                    } else {
                        json!({
                    "action":"resume_maintenance","command":"workspace.remove",
                    "argv":maintenance_argv(true,key,workspace.as_ref(),*force),
                    "reason":"Resume the published removal with its original idempotency key.","requires_explicit_authorization":true})
                    },
                ),
                None => ("maintenance_lock", diagnose()),
            };
            Presentation {
                code: "WORKSPACE_MAINTENANCE",
                details: json!({"schema":"podway.workspace-maintenance-details/v1",
                "boundary":boundary,"timeout_ms":context.timeout_ms,"recovery":recipe}),
            }
        }
        E::Busy | E::ConnectionBusy | E::ReadAdmissionBusy => Presentation {
            code: "WORKSPACE_BUSY",
            details: json!({"schema":"podway.workspace-busy-details/v1",
            "boundary": if matches!(error, E::ConnectionBusy) {"connection_gate"}
                else if matches!(error, E::ReadAdmissionBusy) {"sqlite_read_admission"}
                else {"sqlite_write_lock"},
            "timeout_ms":context.timeout_ms,"recovery":refresh()}),
        },
        E::Legacy { generation, marker } => Presentation {
            code: "LEGACY_RUNTIME_STATE_UNSUPPORTED",
            details: json!({
            "schema":"podway.legacy-runtime-state-unsupported-details/v1", "store_schema_version":generation,
            "markers":marker.iter().collect::<Vec<_>>(), "recovery":{"action":"reset_legacy_state","command":"workspace.reset_all",
                "argv":["podway","--json","reset","--all","--yes","--idempotency-key","<new-idempotency-key>"], "reason":"Replace <new-idempotency-key> with a fresh caller-owned key, then explicitly reset unsupported daemon-era runtime state.","requires_explicit_authorization":true}}),
        },
        E::UnsupportedGeneration { .. } => recovery("WORKSPACE_SCHEMA_UNSUPPORTED"),
        E::Unreadable | E::SchemaMismatch | E::Storage(_) | E::Sqlite(_) => {
            recovery("WORKSPACE_STATE_UNREADABLE")
        }
        E::WriterMismatch { expected, actual } => Presentation {
            code: "WORKSPACE_CONTRACT_MISMATCH",
            details: json!({
            "schema":"podway.workspace-contract-mismatch-details/v1", "expected_contract_manifest_digest":expected,
            "actual_contract_manifest_digest":actual,"recovery":diagnose()}),
        },
        E::BindingMismatch => simple("WORKSPACE_ID_CONFLICT"),
        E::ExpectedWorkspaceUuidMismatch { expected, actual } => workspace(expected, actual),
        E::IdempotencyKeyReused => Presentation {
            code: "IDEMPOTENCY_KEY_REUSED",
            details: json!({"schema":"podway.idempotency-key-reused-details/v2"}),
        },
        E::PreparedStateChanged { .. } => match context.expected_revision {
            Some(expected) => Presentation {
                code: if context.command.as_str().starts_with("item.")
                    && context.command.as_str() != "item.record_many"
                {
                    "ITEM_REVISION_CONFLICT"
                } else {
                    "SESSION_REVISION_CONFLICT"
                },
                details: json!({"schema":"podway.revision-conflict-details/v3","expected_revision":expected,"recovery":refresh()}),
            },
            None => simple("INTERNAL_ERROR"),
        },
        E::MutationOutcomeUnknown => {
            let Some(key) = &context.idempotency_key else {
                return simple("INTERNAL_ERROR");
            };
            Presentation {
                code: "MUTATION_OUTCOME_UNKNOWN",
                details: json!({"schema":"podway.mutation-outcome-unknown-details/v3",
                "outcome":"unknown","idempotency_key":key.as_str(),"reconcile":{"command":"outcome.lookup","idempotency_key":key.as_str()},
                "recovery":{"action":"reconcile_mutation","command":"outcome.lookup",
                    "argv":["podway","--json","outcome","lookup","--idempotency-key",key.as_str()],
                    "reason":"Look up the exact successful outcome before any retry.","requires_explicit_authorization":false}}),
            }
        }
        E::InvalidInput { .. } => simple("REQUEST_INVALID"),
        E::Domain(error) => graph(error, context),
        E::State(error) => state(error, context),
    }
}

fn view(error: &GraphViewErrorV2) -> Presentation {
    match error {
        GraphViewErrorV2::MissingGraphSession => simple("SESSION_NOT_FOUND"),
        GraphViewErrorV2::TerminalSessionHasNoNext | GraphViewErrorV2::NoActiveAttempt => {
            simple("SESSION_NOT_RUNNING")
        }
        GraphViewErrorV2::DelegationNotApplicable => {
            delegation("DELEGATION_NOT_APPLICABLE", "direct_attempt")
        }
        GraphViewErrorV2::DelegationReceiptMissing => {
            delegation("DELEGATION_RECEIPT_REQUIRED", "receipt_missing")
        }
        GraphViewErrorV2::AttemptNotFound => simple("ATTEMPT_NOT_FOUND"),
        GraphViewErrorV2::TimestampOutOfRange => simple("INTERNAL_ERROR"),
        GraphViewErrorV2::InvalidSnapshot | GraphViewErrorV2::InconsistentState(_) => {
            recovery("WORKSPACE_STATE_UNREADABLE")
        }
    }
}

fn evidence(error: &EvidenceReadErrorV2) -> Presentation {
    match error {
        EvidenceReadErrorV2::View(error) => view(error),
        EvidenceReadErrorV2::TokenNotBound => simple("REQUEST_INVALID"),
        EvidenceReadErrorV2::NotAvailable {
            graph_node_id,
            source_graph_node_id,
            item_id,
            reference_state,
        } => runtime(
            "EVIDENCE_NOT_AVAILABLE",
            json!({"graph_node_id":graph_node_id,"source_graph_node_id":source_graph_node_id,"item_id":item_id,"reference_state":reference_state}),
        ),
        EvidenceReadErrorV2::ReferenceStale {
            graph_node_id,
            source_graph_node_id,
            expected_source_attempt_id,
        } => runtime(
            "EVIDENCE_REFERENCE_STALE",
            json!({"graph_node_id":graph_node_id,"source_graph_node_id":source_graph_node_id,"expected_source_attempt_id":expected_source_attempt_id,"current_source_attempt_id":null}),
        ),
        EvidenceReadErrorV2::TokenStale {
            graph_node_id,
            source_graph_node_id,
            item_id,
            expected_value_digest,
            current_value_digest,
        } => runtime(
            "EVIDENCE_PAGE_TOKEN_STALE",
            json!({"graph_node_id":graph_node_id,"source_graph_node_id":source_graph_node_id,"item_id":item_id,"expected_value_digest":expected_value_digest,"current_value_digest":current_value_digest}),
        ),
        EvidenceReadErrorV2::TokenExhausted {
            source_graph_node_id,
            item_id,
            offset,
            total_size,
            unit,
        } => runtime(
            "EVIDENCE_PAGE_TOKEN_EXHAUSTED",
            json!({"source_graph_node_id":source_graph_node_id,"item_id":item_id,"offset":offset,"total_size":total_size,"unit":unit}),
        ),
    }
}

fn boundary(error: &ExecutionBoundaryErrorV1, context: &DirectErrorContextV1) -> Presentation {
    match error {
        ExecutionBoundaryErrorV1::Domain(error) => domain(error, context),
        ExecutionBoundaryErrorV1::LocalArtifact(error) => simple(match error {
            crate::execution::LocalArtifactFailureV1::NotFound => "ARTIFACT_NOT_FOUND",
            crate::execution::LocalArtifactFailureV1::Unreadable => "ARTIFACT_UNREADABLE",
            crate::execution::LocalArtifactFailureV1::PathOutsideWorktree => {
                "PATH_OUTSIDE_WORKTREE"
            }
            crate::execution::LocalArtifactFailureV1::PathUnsafe => "WORKSPACE_PATH_UNSAFE",
        }),
        ExecutionBoundaryErrorV1::ProcedureV2Unsupported => simple("PROCEDURE_SCHEMA_UNSUPPORTED"),
        ExecutionBoundaryErrorV1::Transient { .. } => recovery("WORKSPACE_STATE_UNREADABLE"),
    }
}

fn preparation(error: &ExecutionErrorV1, _context: &DirectErrorContextV1) -> Presentation {
    match error {
        ExecutionErrorV1::InvalidPersistedExecution { .. } => {
            recovery("WORKSPACE_STATE_UNREADABLE")
        }
    }
}

fn start(
    error: &ProcedureV2StartPreparationErrorV1,
    context: &DirectErrorContextV1,
) -> Presentation {
    use ProcedureV2StartPreparationErrorV1 as E;
    match error {
        E::Source(ProcedureV2SourceAdmissionErrorV1::SchemaInvalid { diagnostic_codes }) => {
            runtime(
                "PROCEDURE_V2_SCHEMA_INVALID",
                json!({"diagnostic_codes":diagnostic_codes}),
            )
        }
        E::Source(ProcedureV2SourceAdmissionErrorV1::Rejected(
            ExecutionBoundaryErrorV1::Domain(_),
        )) => simple("PROCEDURE_INVALID"),
        E::Source(ProcedureV2SourceAdmissionErrorV1::Rejected(error)) => boundary(error, context),
        E::PresetNotFound => simple("PRESET_NOT_FOUND"),
        E::DigestConfirmationRequired { procedure_digest } => runtime(
            "DIGEST_CONFIRMATION_REQUIRED",
            json!({"procedure_digest":procedure_digest}),
        ),
        E::ProcedureDigestMismatch { expected, actual }
        | E::PinnedPresetDigestMismatch { expected, actual } => digest(expected, actual),
        E::Domain(error) => domain(error, context),
        E::InvalidStoreValue(_) => simple("INTERNAL_ERROR"),
        E::Execution(error) => preparation(error, context),
        E::GoalMutation(error) => graph(error, context),
    }
}

fn git(source: &podway_git::GitResolveErrorV1) -> Presentation {
    match source {
        podway_git::GitResolveErrorV1::NonGitRepository => simple("NOT_A_GIT_WORKTREE"),
        podway_git::GitResolveErrorV1::BareRepository => simple("BARE_GIT_REPOSITORY"),
        podway_git::GitResolveErrorV1::WorktreeDeleted => simple("WORKTREE_GONE"),
        podway_git::GitResolveErrorV1::IdentityMismatch { .. }
        | podway_git::GitResolveErrorV1::MoveConflict { .. } => simple("WORKSPACE_ID_CONFLICT"),
        podway_git::GitResolveErrorV1::Selector { .. } => simple("REQUEST_INVALID"),
        podway_git::GitResolveErrorV1::PathEscape { .. }
        | podway_git::GitResolveErrorV1::SymlinkEscape { .. }
        | podway_git::GitResolveErrorV1::Representation { .. } => simple("WORKSPACE_PATH_UNSAFE"),
        podway_git::GitResolveErrorV1::PermissionDenied { .. }
        | podway_git::GitResolveErrorV1::Io { .. }
        | podway_git::GitResolveErrorV1::WorkspaceLayoutCleanup { .. }
        | podway_git::GitResolveErrorV1::Invariant { .. } => recovery("WORKSPACE_STATE_UNREADABLE"),
    }
}

fn maintenance(
    error: &crate::direct_maintenance::DirectMaintenanceErrorV1,
    context: &DirectErrorContextV1,
) -> Presentation {
    use crate::{
        direct_maintenance::DirectMaintenanceErrorV1 as E,
        direct_workspace::DirectWorkspaceErrorV1 as W,
        workspace::WorkspaceRemovalFilesystemErrorV1 as F,
    };
    match error {
        E::ConfirmationRequired => simple("CONFIRMATION_REQUIRED"),
        E::ForceRequired => simple("REQUEST_INVALID"),
        E::Filesystem(F::UnsafePath | F::UnsupportedPlatform | F::NotEmptyResidual) => {
            simple("WORKSPACE_PATH_UNSAFE")
        }
        E::Filesystem(F::MarkerChanged | F::MarkerMissing) => {
            store(&DirectStoreErrorV1::Maintenance, context)
        }
        E::Filesystem(F::Path { .. }) => recovery("WORKSPACE_STATE_UNREADABLE"),
        E::Workspace(W::Resolution(DirectWorkspaceResolutionErrorV1::Resolution(error))) => {
            resolution(error, context)
        }
        E::Workspace(
            W::Resolution(DirectWorkspaceResolutionErrorV1::Store(error)) | W::Store(error),
        ) => store(error, context),
        E::Workspace(W::Protocol(_)) => simple("REQUEST_INVALID"),
        E::Workspace(W::Layout(error)) => match error {
            podway_git::WorkspaceLayoutErrorV1::Revalidation { source }
            | podway_git::WorkspaceLayoutErrorV1::Initialization { source } => git(source),
        },
        E::Workspace(W::Boundary(error)) => boundary(error, context),
    }
}

fn resolution(error: &WorkspaceResolutionErrorV1, _context: &DirectErrorContextV1) -> Presentation {
    use WorkspaceResolutionErrorV1 as E;
    match error {
        E::ExpectedWorkspaceUuidMismatch { expected, actual } => workspace(expected, actual),
        E::ExistingBindingMissing => simple("WORKSPACE_NOT_INITIALIZED"),
        E::BootstrapBindingAlreadyPresent => simple("WORKSPACE_ALREADY_INITIALIZED"),
        E::Git { source, .. } => git(source),
        E::Selector { .. } => simple("REQUEST_INVALID"),
        E::WorkspaceConfigInvalid => Presentation {
            code: "WORKSPACE_CONFIG_INVALID",
            details: json!({"guidance":"Remove any non-prod mode from .podway/config.yaml, then run podway reset --all with explicit authorization."}),
        },
        E::GitStoreFingerprintMismatch { .. }
        | E::PreliminaryIdentityWasNotCandidate { .. }
        | E::RevalidatedIdentityStateMismatch { .. }
        | E::RevalidatedStoreIdentityMismatch { .. } => simple("WORKSPACE_ID_CONFLICT"),
        E::StoredRootPathInvalid { .. }
        | E::WorkspaceRootPathInvalid { .. }
        | E::RuntimeDirectoryPathInvalid { .. }
        | E::RuntimeDirectoryPathUnsupportedPlatform
        | E::RuntimeDatabasePathChangedDuringResolution
        | E::WorkspaceRootConversion { .. } => simple("WORKSPACE_PATH_UNSAFE"),
    }
}

/// Produce only validated error-v2 and recovery-v2 documents from a typed failure.
pub fn render_direct_error_v1(
    error: &DirectExecutionErrorV1,
    context: &DirectErrorContextV1,
) -> Result<ErrorEnvelopeV2, ProtocolError> {
    let mut presentation = match error {
        DirectExecutionErrorV1::InputTooLarge => simple("REQUEST_TOO_LARGE"),
        DirectExecutionErrorV1::SessionNotTerminal => simple("SESSION_NOT_TERMINAL"),
        DirectExecutionErrorV1::Workspace(DirectWorkspaceResolutionErrorV1::Resolution(error)) => {
            resolution(error, context)
        }
        DirectExecutionErrorV1::Workspace(DirectWorkspaceResolutionErrorV1::Store(error))
        | DirectExecutionErrorV1::Store(error) => store(error, context),
        DirectExecutionErrorV1::Layout(error) => match error {
            podway_git::WorkspaceLayoutErrorV1::Revalidation { source }
            | podway_git::WorkspaceLayoutErrorV1::Initialization { source } => git(source),
        },
        DirectExecutionErrorV1::Maintenance(error) => maintenance(error, context),
        DirectExecutionErrorV1::Protocol(_) => simple("REQUEST_INVALID"),
        DirectExecutionErrorV1::View(error) => view(error),
        DirectExecutionErrorV1::Evidence(error) => evidence(error),
        DirectExecutionErrorV1::Domain(error) => domain(error, context),
        DirectExecutionErrorV1::Boundary(error) => boundary(error, context),
        DirectExecutionErrorV1::Preparation(error) => preparation(error, context),
        DirectExecutionErrorV1::Start(error) => start(error, context),
        DirectExecutionErrorV1::StartDecisionRequired {
            lifecycle: value,
            current_terminal_disposition,
            ..
        } => Presentation {
            code: "SESSION_START_DECISION_REQUIRED",
            details: json!({"schema":"podway.session-start-decision-required-details/v2",
                "lifecycle":lifecycle(*value),"current_terminal_disposition":current_terminal_disposition,
                "allowed_actions":allowed_actions(*value,*current_terminal_disposition)}),
        },
        DirectExecutionErrorV1::Delegation(error) => match error {
            DirectDelegationFailureV1::ReceiptRequired => {
                delegation("DELEGATION_RECEIPT_REQUIRED", "receipt_missing")
            }
            DirectDelegationFailureV1::PackageChanged => {
                delegation("DELEGATION_RECEIPT_STALE", "package_changed")
            }
            DirectDelegationFailureV1::ItemsChanged => {
                delegation("DELEGATION_RECEIPT_STALE", "items_changed")
            }
            DirectDelegationFailureV1::NotApplicable => {
                delegation("DELEGATION_NOT_APPLICABLE", "direct_attempt")
            }
            DirectDelegationFailureV1::AlreadyAccepted => {
                delegation("DELEGATION_ALREADY_ACCEPTED", "duplicate_acceptance")
            }
            DirectDelegationFailureV1::BindingMismatch(reason) => {
                delegation("DELEGATION_BINDING_MISMATCH", reason.public_reason())
            }
        },
    };
    if presentation.code == "INTERNAL_ERROR" {
        presentation.details =
            json!({"schema":"podway.internal-error-details/v2","diagnostic_id":context.request_id});
    }
    if let Some(value) = presentation.details.get("recovery") {
        RecoveryRecipeV2::from_value(value.clone())?;
    }
    static CATALOG: OnceLock<Value> = OnceLock::new();
    let catalog = CATALOG.get_or_init(|| {
        serde_json::from_str(include_str!(
            "../../../assets/specifications/error-codes.json"
        ))
        .expect("the canonical error catalog is valid JSON")
    });
    let entry = catalog["errors"]
        .as_array()
        .and_then(|entries| {
            entries
                .iter()
                .find(|entry| entry["code"] == presentation.code)
        })
        .ok_or_else(|| ProtocolError::InvalidErrorDetails {
            code: presentation.code.to_owned(),
        })?;
    let (_, exit, retryable) = podway_protocol::error_code_catalog_v1()
        .find(|(code, _, _)| *code == presentation.code)
        .ok_or_else(|| ProtocolError::InvalidErrorDetails {
            code: presentation.code.to_owned(),
        })?;
    ErrorEnvelopeV2::new(ErrorEnvelopeInputV2 {
        request_id: context.request_id.clone(),
        command: context.command.clone(),
        generated_at: context.observed_at.clone(),
        code: ErrorCodeV1::new(presentation.code)?,
        message: entry["summary"]
            .as_str()
            .expect("the canonical catalog has summaries")
            .to_owned(),
        retryable,
        exit_code: ExitCodeV1::new(exit)?,
        workspace: None,
        details: presentation
            .details
            .as_object()
            .expect("details are an object")
            .clone(),
    })
}
