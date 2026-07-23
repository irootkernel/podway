# Goals and Non-Goals

## Priority order

The priorities are ordered. A lower priority MUST NOT weaken a higher priority.

1. Prevent omitted procedure steps.
2. Make the current task state immediately understandable.
3. Make concurrent writes deterministic.
4. Recover safely from daemon and client failure.
5. Keep the mental model small.
6. Keep task state local to the worktree.
7. Provide stable automation interfaces.
8. Avoid features whose main value is long-term review, audit, or analytics.

## Functional goals

Podway MUST:

- run an ordered procedure with one active stage attempt;
- support built-in presets and custom YAML procedures;
- expose required and optional typed stage items;
- prevent completion when required items or blockers remain;
- support complete, skip, retry, return, reopen, cancel, and reset;
- represent reached downstream work as `redo` after return;
- preserve previous attempts only for the lifetime of the current session;
- serialize mutations through a daemon-controlled worktree FIFO queue;
- provide idempotent mutation handling;
- provide read consistency indicators when jobs are pending;
- fail closed outside a valid Git worktree;
- keep the authoritative state database inside the worktree;
- serve stable versioned JSON for every public operation;
- install and run `podwayd` as a macOS user LaunchAgent;
- ship complete help and shell completion.

## Reliability goals

Podway MUST guarantee:

- at most one executing mutation per worktree;
- atomic state transitions;
- no double application after client retry or lost response;
- recovery of acknowledged queued jobs after daemon restart, while the worktree remains at its registered location;
- stale cursor and stale item updates fail rather than overwrite newer state;
- failed or cancelled jobs do not modify session state;
- state remains valid after process termination at every transaction boundary;
- corrupt or unsupported state fails closed;
- no state is retained outside the worktree except minimal daemon registry metadata.

## Usability goals

Podway SHOULD:

- make the common workflow short enough for frequent use;
- show concise stage instructions and exact missing items;
- give structured command suggestions rather than vague prose;
- avoid requiring users to manage run IDs;
- make destructive actions explicit;
- support synchronous CLI use by default and detached jobs when needed;
- allow scripts and AI agents to use the same public contract as humans;
- keep procedure authoring understandable without programming knowledge.

## Non-goals

Podway is not and MUST NOT become, without a new accepted architecture decision:

- a long-term evidence repository;
- a review database;
- a post-mortem, audit, compliance, or analytics system;
- a general history explorer;
- a task-management board or issue tracker;
- a multi-task workspace scheduler;
- an arbitrary finite-state-machine framework;
- a parallel-stage DAG or BPMN engine;
- a CI/CD system;
- a test runner, build runner, shell runner, or command executor;
- a Git client or Git mutation tool;
- an AI agent runtime;
- a remote collaboration service;
- a multi-user server;
- a secret manager;
- an artifact archive;
- a cryptographic identity system;
- a security boundary against processes running as the same user;
- an automatic judge of semantic correctness;
- a product-specific adapter package.

## Explicitly excluded behaviors

Procedure and workspace files MUST NOT:

- execute shell commands;
- load code or plugins;
- reference remote schemas or includes;
- define arbitrary expressions;
- configure network endpoints;
- mutate Git;
- access paths outside the worktree;
- contain credentials intended for daemon authentication.

The daemon MUST NOT:

- listen on TCP;
- make network requests;
- run configured commands;
- invoke `git` for mutations;
- copy artifact bytes into Podway storage;
- silently begin or replace a task session;
- infer stage completion from another process's exit status;
- persist task data in its global registry or logs.

## Deferred, not required

The following capabilities are outside the supported product and are not release
goals:

- Linux or Windows packaging and service integration;
- Intel macOS, translated, universal, fat, or cross-built distribution;
- remote synchronization;
- team or server mode;
- multiple simultaneous sessions in one worktree;
- parallel stage groups or joins;
- long-term session export and import;
- artifact content storage;
- cryptographic actor signatures;
- product-specific adapters;
- automatic command capture;
- UI beyond CLI and shell completion.
