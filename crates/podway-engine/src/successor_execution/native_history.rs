//! Wrap retained successor projections in a coherent native Direct read.

use super::*;
use crate::{
    direct_execution::DirectExecutionErrorV1,
    execution::ExecutionClockV1,
    workspace::{DirectWorkspaceResolutionErrorV1, DirectWorkspaceResolverV1},
};
use podway_git::{GitResolverContractV1, WorktreeSelectorV1};
use podway_protocol::{OutputEnvelopeV4, serialized_json_len_v1};
use podway_store::{
    ValidatedWorkspaceRootV1,
    direct_store::{DirectReadSnapshotV1, DirectRecordedOutcomeV1},
    successor_store::CandidateHistorySnapshot,
};
use std::time::Instant;

#[derive(Clone, Debug)]
pub enum DirectSuccessorHistoryCommandV1 {
    CurrentEvidence {
        source: EvidenceSourceV3,
        item: podway_core::ItemId,
        expected_attempt: Option<AttemptId>,
    },
    Groups,
    Group {
        execution: GroupExecutionIdV1,
    },
    PriorWork {
        execution: GroupExecutionIdV1,
        definition: Sha256Digest,
    },
    GoalRevisions,
    Dag {
        definition: DirectHistoryDefinitionV1,
    },
    Attempts {
        definition: DirectHistoryDefinitionV1,
    },
    Blockers {
        attempt: ScopedAttemptV1,
    },
    Item {
        attempt: ScopedAttemptV1,
        item: podway_core::ItemId,
    },
}

#[derive(Clone, Debug)]
pub enum DirectHistoryDefinitionV1 {
    Base(Sha256Digest),
    Group {
        execution: GroupExecutionIdV1,
        digest: Sha256Digest,
    },
}

impl DirectHistoryDefinitionV1 {
    fn borrowed(&self) -> SuccessorHistoryDefinition<'_> {
        match self {
            Self::Base(digest) => SuccessorHistoryDefinition::Base(digest),
            Self::Group { execution, digest } => {
                SuccessorHistoryDefinition::Group { execution, digest }
            }
        }
    }
}

impl DirectSuccessorHistoryCommandV1 {
    pub const fn command_name(&self) -> &'static str {
        match self {
            Self::CurrentEvidence { .. } => "evidence.read",
            Self::Groups => "group.list",
            Self::Group { .. } => "group.show",
            Self::PriorWork { .. } => "group.prior_work",
            Self::GoalRevisions => "history.goal_revisions",
            Self::Dag { .. } => "history.dag",
            Self::Attempts { .. } => "history.attempts",
            Self::Blockers { .. } => "history.blockers",
            Self::Item { .. } => "history.item",
        }
    }

    fn project(
        self,
        view: &mut impl SuccessorReadStore,
        session: SuccessorHistorySession<'_>,
        page: SuccessorHistoryPage<'_>,
    ) -> Result<CanonicalExecutionJsonV1> {
        match self {
            Self::CurrentEvidence {
                source,
                item,
                expected_attempt,
            } => read_successor_current_evidence(
                view,
                session,
                expected_attempt.as_ref(),
                &source,
                &item,
                page.continuation,
            ),
            Self::Groups => list_successor_groups(view, session, page),
            Self::Group { execution } => show_successor_group(view, session, &execution, page),
            Self::PriorWork {
                execution,
                definition,
            } => list_successor_prior_work(view, session, &execution, &definition, page),
            Self::GoalRevisions => list_successor_goal_revisions(view, session, page),
            Self::Dag { definition } => {
                read_successor_history_dag(view, session, definition.borrowed(), page.continuation)
            }
            Self::Attempts { definition } => {
                list_successor_history_attempts(view, session, definition.borrowed(), page)
            }
            Self::Blockers { attempt } => {
                list_successor_history_blockers(view, session, &attempt, page)
            }
            Self::Item { attempt, item } => {
                read_successor_history_item(view, session, &attempt, &item, page.continuation)
            }
        }
    }
}

#[derive(Clone, Debug)]
pub struct DirectSuccessorHistoryV1 {
    pub session: Option<SessionId>,
    pub location: CandidateSessionLocation,
    pub limit: usize,
    pub continuation: Option<String>,
    pub command: DirectSuccessorHistoryCommandV1,
}

pub(crate) struct DirectSuccessorHistoryRequestV1 {
    pub request_id: RequestIdV1,
    pub selector: WorktreeSelectorV1,
    pub expected_workspace_id: Option<WorkspaceId>,
    pub deadline: Instant,
    pub input: DirectSuccessorHistoryV1,
}

pub(crate) fn dispatch_successor_history<R: GitResolverContractV1>(
    resolver: &DirectWorkspaceResolverV1<R>,
    request: DirectSuccessorHistoryRequestV1,
    clock: &impl ExecutionClockV1,
) -> std::result::Result<OutputEnvelopeV4, DirectExecutionErrorV1> {
    let DirectSuccessorHistoryV1 {
        session,
        location,
        limit,
        continuation,
        command,
    } = request.input;
    resolver.resolve_existing_and_read(
        request.selector,
        request.expected_workspace_id.as_ref(),
        request.deadline,
        |resolved, snapshot| {
            resolver
                .revalidate(resolved, snapshot.binding())
                .map_err(DirectWorkspaceResolutionErrorV1::from)?;
            let workspace = snapshot.binding().identity().workspace_uuid();
            let name = command.command_name();
            let mut output = OutputEnvelopeInputV4 {
                request_id: request.request_id,
                command: CommandNameV1::new(name)?,
                generated_at: Rfc3339MillisV1::from_unix_millis(clock.now().get())?,
                workspace: Some(WorkspaceOutputV1::new(
                    workspace.clone(),
                    resolved
                        .worktree()
                        .roots()
                        .worktree_root()
                        .display()
                        .as_str(),
                    snapshot.committed_sequence()?,
                )?),
                session: None,
                result: serde_json::Map::new(),
                warnings: Vec::new(),
            };
            let mut empty =
                serde_json::to_value(&output).map_err(|_| DirectStoreErrorV1::Unreadable)?;
            empty["schema"] = json!(OUTPUT_SCHEMA_V4);
            let response_headroom = serialized_json_len_v1(&empty)
                .map_err(|_| SuccessorExecutionError::ResponseBudget)?
                - 2;
            let mut view = PinnedSuccessorRead {
                snapshot,
                root: resolved.workspace_root(),
                response_headroom,
            };
            let session = if matches!(
                &command,
                DirectSuccessorHistoryCommandV1::CurrentEvidence { .. }
            ) {
                if location != CandidateSessionLocation::Current {
                    return Err(DirectExecutionErrorV1::InvalidRequest);
                }
                let actual = snapshot.successor_current_session_id()?;
                if let Some(expected) = session
                    && actual.as_ref() != Some(&expected)
                {
                    return Err(DirectExecutionErrorV1::Domain(
                        DomainError::SessionIdentityMismatch { expected, actual },
                    ));
                }
                actual.ok_or(crate::v2_read_service::GraphViewErrorV2::MissingGraphSession)?
            } else {
                session.ok_or(DirectExecutionErrorV1::InvalidRequest)?
            };
            let target = SuccessorHistorySession {
                workspace,
                session: &session,
                location,
            };
            let page = SuccessorHistoryPage {
                limit,
                continuation: continuation.as_deref(),
            };
            let result = command.project(&mut view, target, page)?;
            output.result = serde_json::from_str(result.as_str())
                .map_err(|_| DirectStoreErrorV1::Unreadable)?;
            let schema = output
                .result
                .get("schema")
                .and_then(Value::as_str)
                .ok_or(DirectStoreErrorV1::Unreadable)?
                .replace("-fixture-", "-");
            output
                .result
                .insert("schema".to_owned(), Value::String(schema));
            Ok(OutputEnvelopeV4::new(output)?)
        },
    )
}

struct PinnedSuccessorRead<'snapshot, 'connection> {
    snapshot: &'snapshot DirectReadSnapshotV1<'connection>,
    root: &'snapshot ValidatedWorkspaceRootV1,
    response_headroom: usize,
}

impl PinnedSuccessorRead<'_, '_> {
    fn admit(&self, workspace: &WorkspaceId) -> std::result::Result<(), CandidateStoreError> {
        let actual = self.snapshot.binding().identity().workspace_uuid();
        if workspace != actual {
            return Err(DirectStoreErrorV1::ExpectedWorkspaceUuidMismatch {
                expected: workspace.clone(),
                actual: actual.clone(),
            }
            .into());
        }
        Ok(())
    }
}

impl SuccessorReadStore for PinnedSuccessorRead<'_, '_> {
    fn response_headroom(&self) -> usize {
        self.response_headroom
    }
    fn workspace_root(&self) -> &ValidatedWorkspaceRootV1 {
        self.root
    }

    fn current_snapshot(
        &mut self,
        workspace: &WorkspaceId,
        session: &SessionId,
        admit: impl Fn(&str) -> std::result::Result<CandidateDefinition, CandidateStoreError>,
    ) -> std::result::Result<(ExecutionSessionV3, CandidateSources, u64), CandidateStoreError> {
        self.admit(workspace)?;
        match self.snapshot.successor_state(admit)? {
            Some((state, sources)) if &state.fence().session == session => {
                Ok((state, sources, self.snapshot.committed_sequence()?))
            }
            other => Err(DomainError::SessionIdentityMismatch {
                expected: session.clone(),
                actual: other.map(|(state, _)| state.fence().session),
            }
            .into()),
        }
    }

    fn history_snapshot_with_digest(
        &mut self,
        workspace: &WorkspaceId,
        session: &SessionId,
        location: CandidateSessionLocation,
        admit: impl Fn(&str) -> std::result::Result<CandidateDefinition, CandidateStoreError>,
    ) -> std::result::Result<CandidateHistorySnapshot, CandidateStoreError> {
        self.admit(workspace)?;
        self.snapshot.successor_history(session, location, admit)
    }

    fn lookup_outcome(
        &self,
        workspace: &WorkspaceId,
        key: &IdempotencyKeyV1,
    ) -> std::result::Result<Option<DirectRecordedOutcomeV1>, CandidateStoreError> {
        self.admit(workspace)?;
        Ok(self.snapshot.outcome(key)?)
    }
}
