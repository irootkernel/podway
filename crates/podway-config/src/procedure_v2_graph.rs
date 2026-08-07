//! Bounded graph analysis over a validated Procedure v2 graph: successor adjacency, reachability,
//! BFS levels, strongly connected components, and the assessment-free-to-terminal fixpoint
//! (dossier sections 7.2, 11.3, and 11.4).
//!
//! Two properties make this module a shared foundation rather than a lint helper:
//!
//! - **It reads only a closed-reference-validated model.** Every `next`, route, and evidence target
//!   already resolves, so [`GraphIndex::new`] can build a dense `usize` adjacency once and every
//!   later query is an array walk. A reference that somehow does not resolve is dropped rather than
//!   panicking: an analysis pass that aborts the process would be a worse answer than a slightly
//!   smaller graph.
//! - **Every result is author-ordered.** Placements keep their authored order as their index, and
//!   successors keep their authored order (an action's single `next`, then a decision's routes in
//!   route-table order). Breadth-first walks therefore visit nodes in a fixed order and the
//!   component decomposition is byte-stable across runs, allocators, and platforms.
//!
//! Dominance is deliberately absent. Section 11.3's dominance-based checks belong to vet
//! (V2GRF-001); implementing them here would be dead code under the workspace's deny-warnings lint
//! policy, and a dead analysis is an analysis nobody proves correct.

use std::collections::{BTreeMap, VecDeque};

use podway_core::{ActionOutcomeV2, GraphPlacementV2, ProcedureGraphV2, TransitionEffectV2};

/// The maximum number of placements a Procedure v2 graph can hold (`ProcedureGraphV2::new`).
///
/// [`NodeSet`] is a single `u64` because of this bound, not by coincidence: the domain caps the
/// graph at 64 nodes, so one machine word holds any node set exactly.
const MAX_GRAPH_NODES: usize = 64;

/// A set of graph node indices, held as one 64-bit word.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct NodeSet(u64);

impl NodeSet {
    const fn empty() -> Self {
        Self(0)
    }

    /// Adds a node. An out-of-range index is ignored rather than wrapping the shift, which keeps
    /// the set total over `usize` without a panicking path.
    fn insert(&mut self, node: usize) {
        if node < MAX_GRAPH_NODES {
            self.0 |= 1_u64 << node;
        }
    }

    pub(crate) const fn contains(self, node: usize) -> bool {
        node < MAX_GRAPH_NODES && (self.0 & (1_u64 << node)) != 0
    }
}

/// One outgoing edge of a placement.
///
/// `effect` distinguishes the two edge kinds the authoring model has: `None` is an action's single
/// `next`, and `Some(effect)` is one decision route carrying its declared transition effect. Every
/// route contributes an edge regardless of effect — a rework route is a real transition, and a
/// reachability answer that ignored it would be wrong.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Successor {
    target: usize,
    effect: Option<TransitionEffectV2>,
}

impl Successor {
    pub(crate) const fn target(self) -> usize {
        self.target
    }

    pub(crate) const fn effect(self) -> Option<TransitionEffectV2> {
        self.effect
    }
}

/// A dense, author-ordered index over one validated Procedure v2 graph.
pub(crate) struct GraphIndex<'a> {
    placements: &'a [GraphPlacementV2],
    index_by_id: BTreeMap<&'a str, usize>,
    successors: Vec<Vec<Successor>>,
    entry: usize,
}

impl<'a> GraphIndex<'a> {
    /// Builds the index. Precondition: `graph` comes from a closed-reference-validated model.
    pub(crate) fn new(graph: &'a ProcedureGraphV2) -> Self {
        let placements = graph.placements();
        let index_by_id: BTreeMap<&'a str, usize> = placements
            .iter()
            .enumerate()
            .map(|(index, placement)| (placement.id().as_str(), index))
            .collect();
        let successors = placements
            .iter()
            .map(|placement| match placement {
                GraphPlacementV2::Action(action) => match action.outcome() {
                    ActionOutcomeV2::Next(target) => index_by_id
                        .get(target.as_str())
                        .map(|target| Successor {
                            target: *target,
                            effect: None,
                        })
                        .into_iter()
                        .collect(),
                    ActionOutcomeV2::Terminal => Vec::new(),
                },
                GraphPlacementV2::Decision(decision) => decision
                    .routes()
                    .entries()
                    .iter()
                    .filter_map(|entry| {
                        index_by_id
                            .get(entry.route().to().as_str())
                            .map(|target| Successor {
                                target: *target,
                                effect: Some(entry.route().effect()),
                            })
                    })
                    .collect(),
            })
            .collect();
        let entry = index_by_id
            .get(graph.entry().as_str())
            .copied()
            .unwrap_or(0);
        Self {
            placements,
            index_by_id,
            successors,
            entry,
        }
    }

    pub(crate) fn node_count(&self) -> usize {
        self.placements.len()
    }

    pub(crate) const fn entry(&self) -> usize {
        self.entry
    }

    pub(crate) fn index_of(&self, id: &str) -> Option<usize> {
        self.index_by_id.get(id).copied()
    }

    pub(crate) fn placement(&self, node: usize) -> Option<&'a GraphPlacementV2> {
        self.placements.get(node)
    }

    pub(crate) fn successors(&self, node: usize) -> &[Successor] {
        self.successors.get(node).map_or(&[], Vec::as_slice)
    }

    /// Whether the placement is a terminal action. A decision is never terminal: it always routes.
    pub(crate) fn is_terminal(&self, node: usize) -> bool {
        matches!(
            self.placements.get(node),
            Some(GraphPlacementV2::Action(action)) if action.outcome().is_terminal()
        )
    }

    /// Every node reachable from `node`, including `node` itself at distance zero.
    pub(crate) fn reachable_from(&self, node: usize) -> NodeSet {
        let mut reachable = NodeSet::empty();
        for (index, distance) in self.distances_from(node).into_iter().enumerate() {
            if distance.is_some() {
                reachable.insert(index);
            }
        }
        reachable
    }

    /// Breadth-first levels from `node`: `Some(0)` for `node` itself, `None` for an unreachable
    /// placement. Successors are visited in author order, so the walk is deterministic.
    pub(crate) fn distances_from(&self, node: usize) -> Vec<Option<u32>> {
        let mut distances = vec![None; self.node_count()];
        if node >= self.node_count() {
            return distances;
        }
        distances[node] = Some(0);
        let mut frontier = VecDeque::from([node]);
        while let Some(current) = frontier.pop_front() {
            let next_distance = distances[current].unwrap_or(0).saturating_add(1);
            for successor in self.successors(current) {
                let target = successor.target();
                if distances.get(target).is_some_and(Option::is_none) {
                    distances[target] = Some(next_distance);
                    frontier.push_back(target);
                }
            }
        }
        distances
    }

    /// The strongly connected components of the graph (Tarjan), each sorted in author order and the
    /// components themselves ordered by their author-earliest member.
    ///
    /// Components rather than simple cycles: enumerating every simple cycle is exponential in the
    /// graph size, while the component decomposition is linear, bounded, and the unit a reviewer
    /// actually reasons about — "this region loops" rather than "these 4,000 paths loop".
    pub(crate) fn strongly_connected_components(&self) -> Vec<Vec<usize>> {
        let count = self.node_count();
        let mut state = TarjanState {
            index: vec![None; count],
            lowlink: vec![0; count],
            on_stack: vec![false; count],
            stack: Vec::new(),
            next_index: 0,
            components: Vec::new(),
        };
        for node in 0..count {
            if state.index[node].is_none() {
                self.visit(node, &mut state);
            }
        }
        let mut components = state.components;
        for component in &mut components {
            component.sort_unstable();
        }
        components.sort_by_key(|component| component.first().copied().unwrap_or(usize::MAX));
        components
    }

    fn visit(&self, node: usize, state: &mut TarjanState) {
        state.index[node] = Some(state.next_index);
        state.lowlink[node] = state.next_index;
        state.next_index = state.next_index.saturating_add(1);
        state.stack.push(node);
        state.on_stack[node] = true;

        for successor in self.successors(node) {
            let target = successor.target();
            match state.index.get(target).copied().flatten() {
                None => {
                    self.visit(target, state);
                    state.lowlink[node] = state.lowlink[node].min(state.lowlink[target]);
                }
                Some(target_index) if state.on_stack[target] => {
                    state.lowlink[node] = state.lowlink[node].min(target_index);
                }
                Some(_) => {}
            }
        }

        if state.index[node] == Some(state.lowlink[node]) {
            let mut component = Vec::new();
            while let Some(member) = state.stack.pop() {
                state.on_stack[member] = false;
                component.push(member);
                if member == node {
                    break;
                }
            }
            state.components.push(component);
        }
    }

    /// The least fixpoint `Unsafe = { n : ¬assessment(n) ∧ (terminal(n) ∨ ∃ s ∈ succ(n). s ∈ Unsafe) }`.
    ///
    /// A node is *unsafe* exactly when some path from it reaches a terminal action without passing
    /// a session-goal assessment; its complement is section 7.2's revision-safe target set, where
    /// "a path from the target includes the target itself", so an assessment placement is safe
    /// through its own placement. A cycle that never reaches a terminal contributes no path and
    /// therefore never enters the set — the fixpoint is least, not greatest.
    ///
    /// Computed by monotone iteration to stability. The set only grows and is bounded by the node
    /// count, so at most `node_count + 1` passes run.
    pub(crate) fn assessment_free_to_terminal(
        &self,
        is_assessment: impl Fn(usize) -> bool,
    ) -> NodeSet {
        let count = self.node_count();
        let mut unsafe_nodes = NodeSet::empty();
        for _ in 0..=count {
            let mut changed = false;
            for node in 0..count {
                if unsafe_nodes.contains(node) || is_assessment(node) {
                    continue;
                }
                let escapes = self.is_terminal(node)
                    || self
                        .successors(node)
                        .iter()
                        .any(|successor| unsafe_nodes.contains(successor.target()));
                if escapes {
                    unsafe_nodes.insert(node);
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }
        unsafe_nodes
    }
}

/// Tarjan's working state, kept in one value so the recursive walk borrows a single mutable
/// reference rather than six.
struct TarjanState {
    index: Vec<Option<u32>>,
    lowlink: Vec<u32>,
    on_stack: Vec<bool>,
    stack: Vec<usize>,
    next_index: u32,
    components: Vec<Vec<usize>>,
}
