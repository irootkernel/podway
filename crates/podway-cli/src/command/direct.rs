//! Typed CLI construction at the Engine boundary; no transport request is synthesized.

use podway_core::{BlockerId, TraceSequenceV2};
use podway_engine::{
    direct_api::DirectCommandV1 as Direct,
    direct_execution::{DirectCursorCommandV1 as Cursor, DirectReadCommandV1 as ReadCommand},
    direct_items::DirectItemCommandV1 as ItemCommand,
    direct_sessions::DirectSessionCommandV1 as SessionCommand,
    direct_start::DirectStartCommandV1 as StartCommand,
    v2_read_service::GraphStatusTierV2,
};
use podway_protocol as protocol;

use super::*;
use std::ops::ControlFlow;

pub(super) struct Invocation {
    context: protocol_boundary::DirectInvocationContextV1,
    #[cfg(test)]
    request_ids: Vec<RequestIdV1>,
    resolver:
        podway_engine::workspace::DirectWorkspaceResolverV1<protocol_boundary::NativeGitResolverV1>,
    selector: protocol_boundary::WorktreeSelectorV1,
    deadline: Instant,
    timeout_ms: u64,
    observed_at: Rfc3339MillisV1,
    generation: protocol::ContractGenerationV1,
}

use podway_engine::direct_api as protocol_boundary;

impl Invocation {
    fn new(
        target: &WorkspaceTarget,
        timeout_ms: u64,
        started: Instant,
        generation: protocol::ContractGenerationV1,
    ) -> Result<Self, LocalFailure> {
        let selector = protocol_boundary::WorktreeSelectorV1::new(
            protocol_boundary::WORKTREE_SELECTOR_VERSION_V1,
            None,
            protocol_boundary::LosslessPathV1::from_raw_bytes(
                &target.path_bytes,
                protocol_boundary::DiagnosticPathDisplayV1::new(target.display.clone())
                    .map_err(|_| LocalFailure::request_invalid("worktree display is invalid"))?,
            )
            .map_err(|_| LocalFailure::request_invalid("worktree path bytes are invalid"))?,
        )
        .map_err(|_| LocalFailure::request_invalid("workspace selector is invalid"))?;
        let manifest = Sha256Digest::new(build_identity_v1().contract_manifest_digest().to_owned())
            .map_err(invalid_v2_value)?;
        Ok(Self {
            context: protocol_boundary::DirectInvocationContextV1::default(),
            #[cfg(test)]
            request_ids: Vec::new(),
            resolver: podway_engine::workspace::DirectWorkspaceResolverV1::new(
                protocol_boundary::NativeGitResolverV1::new(),
                manifest,
            ),
            selector,
            deadline: started
                .checked_add(Duration::from_millis(timeout_ms))
                .ok_or_else(|| {
                    LocalFailure::request_invalid("timeout exceeds the monotonic clock range")
                })?,
            timeout_ms,
            observed_at: local_generated_at(&SystemLocalEnvelopeClock)?,
            generation,
        })
    }

    pub(super) fn execute(
        &mut self,
        command: Direct,
        workspace: Option<WorkspaceId>,
        key: Option<protocol_boundary::IdempotencyKeyV1>,
    ) -> Result<protocol_boundary::DirectResponseV1, LocalFailure> {
        let request = protocol_boundary::DirectRequestV1 {
            request_id: RequestIdV1::new(Uuid::new_v4().to_string()).map_err(invalid_v2_value)?,
            selector: self.selector.clone(),
            expected_workspace_id: workspace,
            command,
            idempotency_key: key,
            deadline: self.deadline,
            observed_at: self.observed_at.clone(),
            timeout_ms: self.timeout_ms,
            generation: self.generation,
        };
        #[cfg(test)]
        self.request_ids.push(request.request_id.clone());
        #[cfg(debug_assertions)]
        if let Some(stage) = qualification_pause_stage() {
            return protocol_boundary::execute_direct_with_context_and_maintenance_observer_v1(
                &self.resolver,
                request,
                &podway_engine::native_execution::WallUtcExecutionClockV1,
                &podway_engine::direct_execution::NativeDirectIdSourceV1,
                &mut self.context,
                move |boundary| qualification_pause(stage, boundary),
            )
            .map_err(|_| {
                LocalFailure::internal("the Engine could not construct a valid response")
            });
        }
        protocol_boundary::execute_direct_with_context_v1(
            &self.resolver,
            request,
            &podway_engine::native_execution::WallUtcExecutionClockV1,
            &podway_engine::direct_execution::NativeDirectIdSourceV1,
            &mut self.context,
        )
        .map_err(|_| LocalFailure::internal("the Engine could not construct a valid response"))
    }

    /// READS-004 typed fence-preflight facts: exactly one fresh request
    /// identity, the original selector, fences, observation and deadline, and
    /// no idempotency-key surface. Failures render through the same shared
    /// Engine error finisher with `session.status` attribution; the outer
    /// protocol failure keeps the direct invocation's local mapping.
    pub(super) fn preflight_facts(
        &mut self,
        workspace: Option<WorkspaceId>,
        expected_session_id: Option<SessionId>,
    ) -> Result<podway_engine::direct_execution::DirectPreflightFactsResponseV1, LocalFailure> {
        let request = podway_engine::direct_execution::DirectPreflightFactsRequestV1 {
            request_id: RequestIdV1::new(Uuid::new_v4().to_string()).map_err(invalid_v2_value)?,
            selector: self.selector.clone(),
            expected_workspace_id: workspace,
            expected_session_id,
            deadline: self.deadline,
            observed_at: self.observed_at.clone(),
            timeout_ms: self.timeout_ms,
            generation: self.generation,
        };
        #[cfg(test)]
        self.request_ids.push(request.request_id.clone());
        podway_engine::direct_api::execute_direct_preflight_facts_with_context_v1(
            &self.resolver,
            request,
            &podway_engine::native_execution::WallUtcExecutionClockV1,
            &mut self.context,
        )
        .map_err(|_| LocalFailure::internal("the Engine could not construct a valid response"))
    }
}

#[cfg(debug_assertions)]
fn qualification_pause_stage() -> Option<&'static str> {
    if env::var_os("PODWAY_TEST_DIRECT_MAINTENANCE_TOKEN").as_deref()
        != Some(OsStr::new("podway-direct-maintenance-test-v1"))
    {
        return None;
    }
    let requested = env::var_os("PODWAY_TEST_DIRECT_MAINTENANCE_PAUSE")?;
    [
        "lease-acquired",
        "intent-published",
        "marker-published",
        "runtime-cleared",
        "phase-advanced",
        "before-final-removal",
        "after-first-removal-marker",
        "after-last-removal-marker",
        "directories-removed",
        "before-store-publication",
    ]
    .into_iter()
    .find(|stage| requested == OsStr::new(stage))
}

#[cfg(debug_assertions)]
fn qualification_pause(
    stage: &'static str,
    boundary: podway_engine::direct_maintenance::DirectMaintenanceBoundaryV1,
) {
    use podway_engine::direct_maintenance::DirectMaintenanceBoundaryV1 as Boundary;
    let actual = match boundary {
        Boundary::LeaseAcquired { .. } => "lease-acquired",
        Boundary::IntentPublished => "intent-published",
        Boundary::MarkerPublished => "marker-published",
        Boundary::RuntimeCleared => "runtime-cleared",
        Boundary::PhaseAdvanced => "phase-advanced",
        Boundary::BeforeFinalRemoval => "before-final-removal",
        Boundary::AfterFirstRemovalMarker => "after-first-removal-marker",
        Boundary::AfterLastRemovalMarker => "after-last-removal-marker",
        Boundary::DirectoriesRemoved => "directories-removed",
        Boundary::BeforeStorePublication => "before-store-publication",
        _ => return,
    };
    if stage != actual {
        return;
    }
    if writeln!(
        io::stderr().lock(),
        "podway-test-maintenance-boundary:{stage}"
    )
    .is_err()
    {
        std::process::exit(LOCAL_CLIENT_EXIT);
    }
    loop {
        std::thread::park();
    }
}

pub(super) fn execute(mut cli: Cli) -> Result<RunResult, LocalFailure> {
    let started = Instant::now();
    // Generation selection is validated after parsing and before local
    // execution, stdin preparation or workspace access (ADR-0039).
    let generation = match cli.contract_generation.as_deref() {
        None => protocol::ContractGenerationV1::Generation1,
        Some(value) => protocol::ContractGenerationV1::parse_generation_value_v1(value)
            .ok_or_else(|| LocalFailure::request_invalid("--contract-generation accepts 1 or 2"))?,
    };
    if let Command::CompleteDynamic { kind } = &cli.command {
        let target = match workspace_target(cli.worktree.clone()) {
            Ok(target) => target,
            Err(_) => return Ok(empty_dynamic_completion()),
        };
        let mut invocation = Invocation::new(&target, 500, started, generation)?;
        return match invocation.execute(
            Direct::Read {
                command: ReadCommand::Status {
                    tier: GraphStatusTierV2::Standard,
                    history_before: None,
                },
                expected_session_id: None,
            },
            None,
            None,
        ) {
            Ok(protocol_boundary::DirectResponseV1::Success(output)) => {
                let result = output.as_value()["result"]
                    .as_object()
                    .ok_or_else(|| LocalFailure::internal("status result is invalid"))?;
                Ok(local_result(
                    "__complete",
                    json!({}),
                    dynamic_candidates_v2(result, kind).join("\n"),
                ))
            }
            _ => Ok(empty_dynamic_completion()),
        };
    }
    if let Some(local) = execute_local(&cli)? {
        return Ok(local);
    }
    let route = cli.command.canonical_route();
    prepare_stdin_payload(&mut cli.command)?;
    apply_record_input(&mut cli)?;
    validate_direct_flags(&cli)?;
    validate_command_shape(&cli.command)?;
    // The offline preview applies only to unfenced plain dry runs; a fenced
    // dry run verifies the fences and the lifecycle read-only through the
    // Engine, like a workspace-fenced dry run.
    if matches!(
        &cli.command,
        Command::Start(StartArgs {
            dry_run: true,
            on_existing: None,
            ..
        })
    ) && cli.if_workspace_uuid.is_none()
        && cli.if_session_id.is_none()
    {
        return execute_start_dry_run(&cli);
    }
    let mut target = workspace_target(cli.worktree.clone())?;
    if matches!(
        &cli.command,
        Command::Workspace {
            command: WorkspaceCommand::Remove { .. }
        }
    ) {
        target = workspace_removal_target(target)?;
    }
    confirm_if_required(&cli, route, &target)?;
    let mut fences = ExplicitPreconditions::parse(&cli)?;
    apply_archive_purge_revision(&mut cli.command, &fences)?;
    let mut invocation =
        Invocation::new(&target, cli.timeout.unwrap_or(5_000), started, generation)?;
    let preflight = match preflight::prepare(&mut invocation, &cli, &mut fences)? {
        ControlFlow::Continue(facts) => facts,
        ControlFlow::Break(result) => return Ok(result),
    };
    if let (Command::Start(_), Some(preflight)) = (&cli.command, &preflight) {
        let terminal_disposed = if matches!(
            preflight.facts.lifecycle.as_str(),
            "completed" | "cancelled"
        ) {
            let response = invocation.execute(
                Direct::Session(SessionCommand::Reset(protocol::SessionResetV1 {
                    mode: protocol::SessionDeletionModeV1::Eligible,
                    dry_run: true,
                    preconditions: identity(&fences)?,
                })),
                fences.workspace_id.clone(),
                None,
            )?;
            let output = match response {
                protocol_boundary::DirectResponseV1::Success(output) => output,
                protocol_boundary::DirectResponseV1::Failure(error) => {
                    return preflight_error(&cli.command, error);
                }
            };
            Some(
                output.as_value()["result"]["current_terminal_disposition"]
                    .as_bool()
                    .ok_or_else(|| {
                        LocalFailure::response_invalid(
                            "start preflight omitted the terminal disposition",
                        )
                    })?,
            )
        } else {
            None
        };
        resolve_existing_start(&mut cli, preflight, terminal_disposed)?;
    }
    let command = typed_command(&cli, &fences)?;
    let key = if cli.command.is_mutation() {
        let value = match cli.idempotency_key.as_ref() {
            Some(value) => value.clone(),
            None if !cli.json && io::stdin().is_terminal() && io::stdout().is_terminal() => {
                let value = Uuid::new_v4().to_string();
                write_text_line(
                    &mut io::stderr().lock(),
                    format_args!("idempotency key: {value}"),
                )?;
                value
            }
            None => {
                return Err(LocalFailure::request_invalid(
                    "automation mutations require --idempotency-key",
                ));
            }
        };
        Some(protocol_boundary::IdempotencyKeyV1::new(value).map_err(invalid_v2_value)?)
    } else {
        None
    };
    let response = invocation.execute(command, fences.workspace_id.clone(), key.clone())?;
    // A fenced plain start never enters the interactive interview, so a
    // terminal user cannot turn it into a deletion.
    let interactive_start = matches!(
        &cli.command,
        Command::Start(StartArgs {
            dry_run: false,
            on_existing: None,
            ..
        })
    ) && cli.if_session_id.is_none()
        && !cli.json
        && !cli.quiet
        && io::stdin().is_terminal()
        && io::stdout().is_terminal();
    if interactive_start
        && let protocol_boundary::DirectResponseV1::Failure(error) = &response
        && error.as_value()["code"].as_str() == Some("SESSION_START_DECISION_REQUIRED")
    {
        let disposed = error.as_value()["details"]["current_terminal_disposition"].as_bool();
        let status = invocation.execute(
            Direct::Read {
                command: ReadCommand::Status {
                    tier: GraphStatusTierV2::Standard,
                    history_before: None,
                },
                expected_session_id: None,
            },
            fences.workspace_id.clone(),
            None,
        )?;
        let output = match status {
            protocol_boundary::DirectResponseV1::Success(output) => output,
            protocol_boundary::DirectResponseV1::Failure(error) => {
                return preflight_error(&cli.command, error);
            }
        };
        let preflight = StatusPreflight::from_direct(&output)?;
        fill_facts(&cli.command, &mut fences, &preflight);
        match resolve_existing_start(&mut cli, &preflight, disposed)? {
            ExistingStartResolution::Continue => {
                write_text_line(
                    &mut io::stderr().lock(),
                    format_args!(
                        "No new session was created. Continuing existing {} session {} at revision {}.",
                        preflight.facts.lifecycle,
                        preflight.facts.session_id,
                        preflight.facts.session_revision.get(),
                    ),
                )?;
                let response = invocation.execute(
                    Direct::Read {
                        command: if matches!(
                            preflight.facts.lifecycle.as_str(),
                            "completed" | "cancelled"
                        ) {
                            ReadCommand::Observe
                        } else {
                            ReadCommand::Next
                        },
                        expected_session_id: fences.session_id,
                    },
                    fences.workspace_id,
                    None,
                )?;
                return Ok(direct_response_result(&cli.command, response));
            }
            ExistingStartResolution::Replace => {
                let command = typed_command(&cli, &fences)?;
                let response = invocation.execute(command, fences.workspace_id, key)?;
                return Ok(direct_response_result(&cli.command, response));
            }
        }
    }
    Ok(direct_response_result(&cli.command, response))
}

fn required<T>(value: Option<T>, flag: &str) -> Result<T, LocalFailure> {
    value.ok_or_else(|| LocalFailure::request_invalid(format!("{flag} is required")))
}

fn identity(
    fences: &ExplicitPreconditions,
) -> Result<protocol::SessionIdentityPreconditionsWireV1, LocalFailure> {
    Ok(protocol::SessionIdentityPreconditionsWireV1 {
        expected_session_id: required(fences.session_id.clone(), "--if-session-id")?,
        expected_session_revision: required(fences.session_revision, "--if-session-revision")?,
    })
}

fn mutation(
    fences: &ExplicitPreconditions,
) -> Result<protocol::SessionMutationPreconditionsWireV1, LocalFailure> {
    let identity = identity(fences)?;
    Ok(protocol::SessionMutationPreconditionsWireV1 {
        expected_session_id: identity.expected_session_id,
        expected_session_revision: identity.expected_session_revision,
        expected_attempt_id: required(fences.attempt_id.clone(), "--if-attempt")?,
    })
}

fn item(
    fences: &ExplicitPreconditions,
) -> Result<protocol::ItemMutationPreconditionsWireV1, LocalFailure> {
    Ok(protocol::ItemMutationPreconditionsWireV1 {
        expected_session_id: required(fences.session_id.clone(), "--if-session-id")?,
        expected_attempt_id: required(fences.attempt_id.clone(), "--if-attempt")?,
        expected_item_revision: required(fences.item_revision, "--if-item-revision")?,
    })
}

fn criteria(values: &[String]) -> Result<Vec<protocol::GoalCriterionWireV2>, LocalFailure> {
    parse_criteria(values)?;
    values
        .iter()
        .map(|value| {
            let (id, statement) = value.split_once('=').ok_or_else(|| {
                LocalFailure::request_invalid("criterion must use <criterion-id>=<statement>")
            })?;
            Ok(protocol::GoalCriterionWireV2 {
                criterion_id: CriterionId::new(id.to_owned()).map_err(invalid_v2_value)?,
                statement: statement.to_owned(),
            })
        })
        .collect()
}

fn tier(verbose: bool, compact: bool) -> GraphStatusTierV2 {
    if compact {
        GraphStatusTierV2::Compact
    } else if verbose {
        GraphStatusTierV2::Verbose
    } else {
        GraphStatusTierV2::Standard
    }
}

pub(super) fn typed_command(
    cli: &Cli,
    fences: &ExplicitPreconditions,
) -> Result<Direct, LocalFailure> {
    let read = |command| Direct::Read {
        command,
        expected_session_id: fences.session_id.clone(),
    };
    Ok(match &cli.command {
        Command::Init { repair } => Direct::WorkspaceInit { repair: *repair },
        Command::Doctor { deep } => Direct::WorkspaceDoctor { deep: *deep },
        Command::Workspace {
            command: WorkspaceCommand::Show,
        } => Direct::WorkspaceShow,
        Command::Workspace {
            command: WorkspaceCommand::Repair,
        } => Direct::WorkspaceRepair,
        Command::Workspace {
            command: WorkspaceCommand::Remove { force },
        } => Direct::WorkspaceRemove {
            confirmed: true,
            force: *force,
        },
        Command::Reset(args) if args.all => Direct::WorkspaceResetAll { confirmed: true },
        Command::Start(args) => {
            let source = match (&args.preset, &args.procedure) {
                (Some(preset), None) => protocol::SessionStartSourceV1::Preset {
                    preset: preset.clone(),
                },
                (None, Some(procedure)) => protocol::SessionStartSourceV1::Procedure {
                    procedure: procedure.clone(),
                },
                _ => {
                    return Err(LocalFailure::request_invalid(
                        "start requires one procedure source",
                    ));
                }
            };
            let start = protocol::SessionStartV1 {
                source,
                expected_procedure_digest: args
                    .expect_procedure_digest
                    .as_ref()
                    .map(|value| Sha256Digest::new(value.clone()).map_err(invalid_v2_value))
                    .transpose()?,
                task_title: args.task.clone(),
                dry_run: args.dry_run,
            };
            let mode = if args.auto_archive_terminal {
                Some(protocol::SessionDeletionModeV1::Eligible)
            } else if let Some(disposition) = &args.terminal_disposition {
                let (disposition, actor) = match disposition {
                    StartTerminalDisposition::HandedOff {
                        summary,
                        reference,
                        actor,
                    } => (
                        protocol::TerminalDispositionInputV1::HandedOff {
                            summary: summary.clone(),
                            reference: reference.clone(),
                        },
                        actor.clone(),
                    ),
                    StartTerminalDisposition::NotRequired { reason, actor } => (
                        protocol::TerminalDispositionInputV1::NotRequired {
                            reason: reason.clone(),
                        },
                        actor.clone(),
                    ),
                };
                Some(protocol::SessionDeletionModeV1::DisposeAndArchive { disposition, actor })
            } else {
                match args.on_existing {
                    Some(ExistingSessionPolicy::Preserve) => {
                        Some(protocol::SessionDeletionModeV1::Supersede {
                            reason: required(args.supersede_reason.clone(), "--supersede-reason")?,
                            actor: args.actor.clone(),
                        })
                    }
                    Some(ExistingSessionPolicy::Delete) => {
                        Some(protocol::SessionDeletionModeV1::Delete {
                            confirmed: args.delete_confirmed
                                || cli.yes
                                || args.progress_summary.is_none(),
                            progress_summary: args.progress_summary.clone(),
                        })
                    }
                    None => None,
                }
            };
            Direct::Start(match mode {
                Some(mode) => StartCommand::Replace(protocol::SessionStartReplaceV1 {
                    start,
                    mode,
                    preconditions: identity(fences)?,
                }),
                None => StartCommand::Start(protocol::SessionStartCommandV1 {
                    start,
                    preconditions: match fences.session_id {
                        Some(_) => Some(identity(fences)?),
                        None => None,
                    },
                }),
            })
        }
        Command::Status(args) => read(ReadCommand::Status {
            tier: tier(args.read.verbose, args.compact),
            history_before: args.history_before.map(TraceSequenceV2::new),
        }),
        Command::Next(_) => read(ReadCommand::Next),
        Command::Observe(_) => read(ReadCommand::Observe),
        Command::Evidence {
            command: EvidenceCommand::Read(args),
        } => read(ReadCommand::Evidence {
            source: GraphNodeId::new(args.source.clone()).map_err(invalid_v2_value)?,
            item_id: ItemId::new(args.item.clone()).map_err(invalid_v2_value)?,
            expected_consumer_attempt_id: fences.attempt_id.clone(),
            token: args
                .page_token
                .as_deref()
                .map(protocol::EvidencePageTokenV1::decode)
                .transpose()
                .map_err(invalid_v2_value)?,
        }),
        Command::Delegation {
            command: DelegationCommand::Package,
        } => read(ReadCommand::DelegationPackage),
        Command::Delegation {
            command: DelegationCommand::Receipt { session, attempt },
        } => read(ReadCommand::DelegationReceipt {
            session_id: SessionId::new(session.clone()).map_err(invalid_v2_value)?,
            attempt_id: AttemptId::new(attempt.clone()).map_err(invalid_v2_value)?,
        }),
        Command::Delegation {
            command: DelegationCommand::Accept(args),
        } => Direct::DelegationAccept(protocol::DelegationAcceptV1 {
            input: required(args.input.clone(), "--stdin")?,
            preconditions: mutation(fences)?,
        }),
        Command::Complete => Direct::Cursor(Cursor::Complete(protocol::SessionCompleteV1 {
            preconditions: mutation(fences)?,
        })),
        Command::Skip { reason } => Direct::Cursor(Cursor::Skip(protocol::SessionSkipV1 {
            reason: reason.clone(),
            preconditions: mutation(fences)?,
        })),
        Command::Retry { reason } => Direct::Cursor(Cursor::Retry(protocol::SessionRetryV1 {
            reason: reason.clone(),
            preconditions: mutation(fences)?,
        })),
        Command::Block { reason } => Direct::Cursor(Cursor::Block(protocol::SessionBlockV1 {
            reason: reason.clone(),
            preconditions: mutation(fences)?,
        })),
        Command::Unblock { blocker_id, all } => {
            Direct::Cursor(Cursor::Unblock(protocol::SessionUnblockV1 {
                blocker_id: blocker_id
                    .as_ref()
                    .map(|value| BlockerId::new(value.clone()).map_err(invalid_v2_value))
                    .transpose()?,
                all: *all,
                preconditions: mutation(fences)?,
            }))
        }
        Command::Cancel { reason } => Direct::Cursor(Cursor::Cancel(protocol::SessionCancelV1 {
            reason: reason.clone(),
            preconditions: mutation(fences)?,
        })),
        Command::Reset(args) => Direct::Session(SessionCommand::Reset(protocol::SessionResetV1 {
            mode: match &args.progress_summary {
                Some(progress_summary) => protocol::SessionDeletionModeV1::Force {
                    progress_summary: progress_summary.clone(),
                },
                None => protocol::SessionDeletionModeV1::Eligible,
            },
            dry_run: args.dry_run,
            preconditions: identity(fences)?,
        })),
        Command::Archive { command: None } => {
            Direct::Session(SessionCommand::Archive(protocol::SessionArchiveV1 {
                preconditions: identity(fences)?,
            }))
        }
        Command::Archive {
            command: Some(ArchiveCommand::List),
        } => Direct::Read {
            command: ReadCommand::ArchiveList,
            expected_session_id: None,
        },
        Command::Archive {
            command:
                Some(ArchiveCommand::Show {
                    session_id,
                    verbose,
                    history_before,
                }),
        } => read(ReadCommand::ArchiveShow {
            session_id: SessionId::new(session_id.clone()).map_err(invalid_v2_value)?,
            tier: tier(*verbose, false),
            history_before: history_before.map(TraceSequenceV2::new),
        }),
        Command::Archive {
            command:
                Some(ArchiveCommand::Purge {
                    session_id,
                    expected_session_revision,
                }),
        } => Direct::Session(SessionCommand::Purge(protocol::SessionArchivePurgeV1 {
            session_id: SessionId::new(session_id.clone()).map_err(invalid_v2_value)?,
            expected_session_revision: Revision::new(required(
                *expected_session_revision,
                "--if-session-revision",
            )?),
            confirmed: cli.yes,
        })),
        Command::Check { item_id } => Direct::Item(ItemCommand::Check(protocol::ItemCheckV1 {
            item_id: ItemId::new(item_id.clone()).map_err(invalid_v2_value)?,
            preconditions: item(fences)?,
        })),
        Command::Uncheck { item_id } => {
            Direct::Item(ItemCommand::Uncheck(protocol::ItemUncheckV1 {
                item_id: ItemId::new(item_id.clone()).map_err(invalid_v2_value)?,
                preconditions: item(fences)?,
            }))
        }
        Command::Set(args) => Direct::Item(ItemCommand::Set(protocol::ItemSetV1 {
            item_id: ItemId::new(args.item_id.clone()).map_err(invalid_v2_value)?,
            value: required(args.value.clone(), "VALUE or --stdin")?,
            preconditions: item(fences)?,
        })),
        Command::Add { item_id, value } => Direct::Item(ItemCommand::Add(protocol::ItemAddV1 {
            item_id: ItemId::new(item_id.clone()).map_err(invalid_v2_value)?,
            value: value.clone(),
            preconditions: item(fences)?,
        })),
        Command::Remove {
            item_id,
            value,
            ignore_missing,
        } => Direct::Item(ItemCommand::Remove(protocol::ItemRemoveV1 {
            item_id: ItemId::new(item_id.clone()).map_err(invalid_v2_value)?,
            value: value.clone(),
            ignore_missing: *ignore_missing,
            preconditions: item(fences)?,
        })),
        Command::Attach(args) => Direct::Item(ItemCommand::Attach(protocol::ItemAttachV1 {
            item_id: ItemId::new(args.item_id.clone()).map_err(invalid_v2_value)?,
            source: match (&args.path, &args.reference) {
                (Some(path), None) => protocol::ItemAttachSourceV1::Path {
                    path: path.clone(),
                    media_type: args.media_type.clone(),
                },
                (None, Some(reference)) => protocol::ItemAttachSourceV1::OpaqueReference {
                    reference: reference.clone(),
                    digest: Sha256Digest::new(required(args.digest.clone(), "--digest")?)
                        .map_err(invalid_v2_value)?,
                    size_bytes: required(args.size, "--size")?,
                    media_type: required(args.media_type.clone(), "--media-type")?,
                },
                _ => {
                    return Err(LocalFailure::request_invalid(
                        "attach requires one artifact source",
                    ));
                }
            },
            preconditions: item(fences)?,
        })),
        Command::Clear { item_id } => Direct::Item(ItemCommand::Clear(protocol::ItemClearV1 {
            item_id: ItemId::new(item_id.clone()).map_err(invalid_v2_value)?,
            preconditions: item(fences)?,
        })),
        Command::Record(args) => Direct::RecordMany(protocol::ItemRecordManyV1 {
            operations: required(args.input.as_ref(), "--stdin")?.operations.clone(),
            preconditions: mutation(fences)?,
        }),
        Command::Begin(args) => Direct::Procedure(
            protocol::ProcedureV2MutationCommandV1::SessionBegin(protocol::SessionBeginV1 {
                initial_goal: args
                    .goal
                    .as_ref()
                    .map(|goal| {
                        Ok::<_, LocalFailure>(protocol::InitialGoalWireV2 {
                            goal: goal.clone(),
                            criteria: criteria(&args.criterion)?,
                            actor: args.actor.clone(),
                        })
                    })
                    .transpose()?,
                preconditions: identity(fences)?,
            }),
        ),
        Command::Disposition { command } => {
            let (disposition, actor) = match command {
                DispositionCommand::HandedOff {
                    summary,
                    reference,
                    actor,
                } => (
                    protocol::TerminalDispositionInputV1::HandedOff {
                        summary: summary.clone(),
                        reference: reference.clone(),
                    },
                    actor.clone(),
                ),
                DispositionCommand::NotRequired { reason, actor } => (
                    protocol::TerminalDispositionInputV1::NotRequired {
                        reason: reason.clone(),
                    },
                    actor.clone(),
                ),
            };
            Direct::Procedure(
                protocol::ProcedureV2MutationCommandV1::SessionTerminalDisposition(
                    protocol::SessionTerminalDispositionV1 {
                        disposition,
                        actor,
                        preconditions: identity(fences)?,
                    },
                ),
            )
        }
        Command::Decide(args) => Direct::Procedure(
            protocol::ProcedureV2MutationCommandV1::SessionDecide(protocol::SessionDecideV2 {
                option_id: OptionId::new(args.option.clone()).map_err(invalid_v2_value)?,
                reason: args.reason.clone(),
                actor: args.actor.clone(),
                preconditions: mutation(fences)?,
                expected_goal_revision: fences.goal_revision.map(|revision| revision.get()),
            }),
        ),
        Command::Rework(args) => {
            let identity = identity(fences)?;
            Direct::Procedure(protocol::ProcedureV2MutationCommandV1::SessionRework(
                protocol::SessionReworkV2 {
                    target_graph_node_id: GraphNodeId::new(args.to.clone())
                        .map_err(invalid_v2_value)?,
                    reason: args.reason.clone(),
                    actor: args.actor.clone(),
                    preconditions: protocol::ReworkPreconditionsWireV2 {
                        expected_session_id: identity.expected_session_id,
                        expected_session_revision: identity.expected_session_revision,
                        expected_attempt_id: fences.attempt_id.clone(),
                    },
                },
            ))
        }
        Command::Goal {
            command: GoalCommand::Define(args),
        } => Direct::Procedure(protocol::ProcedureV2MutationCommandV1::GoalDefine(
            protocol::GoalDefineV2 {
                goal: args.goal.clone(),
                criteria: criteria(&args.criterion)?,
                actor: args.actor.clone(),
                preconditions: identity(fences)?,
            },
        )),
        Command::Goal {
            command: GoalCommand::Revise(args),
        } => {
            let identity = identity(fences)?;
            Direct::Procedure(protocol::ProcedureV2MutationCommandV1::GoalRevise(
                protocol::GoalReviseV2 {
                    goal: args.goal.clone(),
                    criteria: criteria(&args.criterion)?,
                    target_graph_node_id: GraphNodeId::new(args.rework_to.clone())
                        .map_err(invalid_v2_value)?,
                    reason: args.reason.clone(),
                    actor: args.actor.clone(),
                    reactivate: args.reactivate,
                    preconditions: protocol::GoalRevisionPreconditionsWireV2 {
                        expected_session_id: identity.expected_session_id,
                        expected_session_revision: identity.expected_session_revision,
                        expected_attempt_id: fences.attempt_id.clone(),
                        expected_goal_revision: required(
                            fences.goal_revision,
                            "--if-goal-revision",
                        )?
                        .get(),
                    },
                },
            ))
        }
        Command::Goal {
            command: GoalCommand::AssessCriterion(args),
        } => Direct::Procedure(protocol::ProcedureV2MutationCommandV1::GoalAssessCriterion(
            protocol::GoalAssessCriterionV2 {
                criterion_id: CriterionId::new(args.criterion_id.clone())
                    .map_err(invalid_v2_value)?,
                status: args.status.clone(),
                reason: args.reason.clone(),
                actor: args.actor.clone(),
                evidence: args
                    .evidence
                    .iter()
                    .map(|value| GraphNodeId::new(value.clone()).map_err(invalid_v2_value))
                    .collect::<Result<_, _>>()?,
                items: args
                    .item
                    .iter()
                    .map(|value| ItemId::new(value.clone()).map_err(invalid_v2_value))
                    .collect::<Result<_, _>>()?,
                preconditions: mutation(fences)?,
                expected_goal_revision: required(fences.goal_revision, "--if-goal-revision")?.get(),
            },
        )),
        Command::Outcome {
            command: OutcomeCommand::Lookup,
        } => Direct::OutcomeLookup {
            key: podway_engine::direct_api::IdempotencyKeyV1::new(required(
                cli.idempotency_key.clone(),
                "--idempotency-key",
            )?)
            .map_err(invalid_v2_value)?,
        },
        _ => {
            return Err(LocalFailure::request_invalid(
                "command is not a direct Engine route",
            ));
        }
    })
}

#[cfg(test)]
mod admit003_tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn invocation_keeps_distinct_request_ids_and_one_deadline_across_preflight_and_mutation() {
        struct OwnedRoot(PathBuf);
        impl Drop for OwnedRoot {
            fn drop(&mut self) {
                let _ = fs::remove_dir_all(&self.0);
            }
        }
        let temporary =
            OwnedRoot(env::temp_dir().join(format!("podway-admit003-{}", Uuid::new_v4())));
        fs::create_dir(&temporary.0).unwrap();
        assert!(
            std::process::Command::new("git")
                .args(["init", "--quiet"])
                .arg(&temporary.0)
                .status()
                .unwrap()
                .success()
        );
        let target = workspace_target(Some(temporary.0.clone())).unwrap();
        let mut invocation = Invocation::new(
            &target,
            5000,
            Instant::now(),
            protocol::ContractGenerationV1::Generation1,
        )
        .unwrap();
        let key = |value: &str| Some(protocol_boundary::IdempotencyKeyV1::new(value).unwrap());
        assert!(matches!(
            invocation
                .execute(Direct::WorkspaceInit { repair: false }, None, key("init"))
                .unwrap(),
            protocol_boundary::DirectResponseV1::Success(_)
        ));
        let cli = Cli::try_parse_from([
            "podway",
            "start",
            "--preset",
            "small-change-v2",
            "--task",
            "request identity",
        ])
        .unwrap();
        assert!(matches!(
            invocation
                .execute(
                    typed_command(&cli, &ExplicitPreconditions::default()).unwrap(),
                    None,
                    key("start")
                )
                .unwrap(),
            protocol_boundary::DirectResponseV1::Success(_)
        ));
        invocation.request_ids.clear();
        let deadline = invocation.deadline;
        let observed_at = invocation.observed_at.clone();
        let podway_engine::direct_execution::DirectPreflightFactsResponseV1::Facts(facts) =
            invocation.preflight_facts(None, None).unwrap()
        else {
            panic!("preflight failed")
        };
        let response = invocation
            .execute(
                Direct::Procedure(protocol::ProcedureV2MutationCommandV1::SessionBegin(
                    protocol::SessionBeginV1 {
                        initial_goal: None,
                        preconditions: protocol::SessionIdentityPreconditionsWireV1 {
                            expected_session_id: facts.session_id,
                            expected_session_revision: facts.session_revision,
                        },
                    },
                )),
                Some(facts.workspace_id),
                key("begin"),
            )
            .unwrap();
        let protocol_boundary::DirectResponseV1::Success(output) = response else {
            panic!("begin failed: {response:?}")
        };
        assert_eq!(invocation.request_ids.len(), 2);
        assert_ne!(invocation.request_ids[0], invocation.request_ids[1]);
        assert_eq!(
            output.as_value()["request_id"],
            invocation.request_ids[1].as_str()
        );
        assert_eq!(invocation.deadline, deadline);
        assert_eq!(invocation.observed_at, observed_at);
        let fresh = Invocation::new(
            &target,
            5000,
            Instant::now(),
            protocol::ContractGenerationV1::Generation1,
        )
        .unwrap();
        assert!(fresh.request_ids.is_empty());
    }
}
