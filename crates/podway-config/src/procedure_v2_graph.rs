//! Bounded graph analysis over a validated Procedure v2 graph: successor adjacency, reachability,
//! BFS levels, strongly connected components, dominance, terminal reachability, and the
//! assessment-free-to-terminal fixpoint (dossier sections 7.2, 11.3, and 11.4).
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
use std::collections::{BTreeMap, VecDeque};

use podway_core::{
    ActionOutcomeV2, GraphNodeId, GraphPlacementV2, ProcedureGraphV2, TransitionEffectV2,
};

use crate::{ParsedNodeDefinition, ParsedProcedureV2};

/// Returns the graph nodes from which every terminal path crosses a session-goal assessment.
///
/// Goal revision uses this stricter subset of ordinary manual-rework targets. The analysis is the
/// same bounded least fixpoint used by vet and lint, exposed here so runtime guidance cannot drift
/// from authoring diagnostics.
pub fn goal_revision_safe_targets_v2(procedure: &ParsedProcedureV2) -> Vec<GraphNodeId> {
    let graph = GraphIndex::new(procedure.graph());
    let assessment_definitions = procedure
        .node_definitions()
        .iter()
        .filter_map(|definition| match definition {
            ParsedNodeDefinition::Decision(definition) if definition.assessment().is_some() => {
                Some(definition.id())
            }
            ParsedNodeDefinition::Action(_) | ParsedNodeDefinition::Decision(_) => None,
        })
        .collect::<Vec<_>>();
    let unsafe_nodes = graph.assessment_free_to_terminal(|index| {
        matches!(
            graph.placement(index),
            Some(GraphPlacementV2::Decision(placement))
                if assessment_definitions.contains(&placement.definition())
        )
    });
    procedure
        .graph()
        .placements()
        .iter()
        .enumerate()
        .filter(|(index, _)| !unsafe_nodes.contains(*index))
        .map(|(_, placement)| placement.id().clone())
        .collect()
}

const NODE_SET_WORDS: usize =
    podway_core::MAX_GRAPH_NODES_PER_PROCEDURE_V2.div_ceil(u64::BITS as usize);

/// A fixed-size set of graph node indices covering the complete Procedure v2 graph bound.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct NodeSet([u64; NODE_SET_WORDS]);

impl NodeSet {
    const fn empty() -> Self {
        Self([0; NODE_SET_WORDS])
    }

    /// Adds a node. An out-of-range index is ignored, keeping the set total over `usize`.
    fn insert(&mut self, node: usize) {
        if node < podway_core::MAX_GRAPH_NODES_PER_PROCEDURE_V2 {
            self.0[node / u64::BITS as usize] |= 1_u64 << (node % u64::BITS as usize);
        }
    }

    fn intersect(&mut self, other: Self) {
        for (word, other) in self.0.iter_mut().zip(other.0) {
            *word &= other;
        }
    }

    pub(crate) fn contains(self, node: usize) -> bool {
        node < podway_core::MAX_GRAPH_NODES_PER_PROCEDURE_V2
            && (self.0[node / u64::BITS as usize] & (1_u64 << (node % u64::BITS as usize))) != 0
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
    predecessors: Vec<Vec<usize>>,
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
        let successors: Vec<Vec<Successor>> = placements
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
        let mut predecessors = vec![Vec::new(); placements.len()];
        for (source, edges) in successors.iter().enumerate() {
            for edge in edges {
                if let Some(targets) = predecessors.get_mut(edge.target()) {
                    targets.push(source);
                }
            }
        }
        let entry = index_by_id
            .get(graph.entry().as_str())
            .copied()
            .unwrap_or(0);
        Self {
            placements,
            index_by_id,
            successors,
            predecessors,
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

    /// Every node with a finite path to a terminal action, following both advance and rework
    /// transitions. A reverse breadth-first walk is exact even in cyclic graphs.
    pub(crate) fn terminal_reachable_nodes(&self) -> NodeSet {
        let mut reaches_terminal = NodeSet::empty();
        let mut frontier = VecDeque::new();
        for node in 0..self.node_count() {
            if self.is_terminal(node) {
                reaches_terminal.insert(node);
                frontier.push_back(node);
            }
        }
        while let Some(current) = frontier.pop_front() {
            for predecessor in self
                .predecessors
                .get(current)
                .map_or(&[][..], Vec::as_slice)
            {
                if !reaches_terminal.contains(*predecessor) {
                    reaches_terminal.insert(*predecessor);
                    frontier.push_back(*predecessor);
                }
            }
        }
        reaches_terminal
    }

    /// Dominators for every reachable node in the complete graph, including declared rework
    /// edges. Unreachable nodes receive the empty set because vet reports them independently.
    pub(crate) fn dominators(&self) -> Vec<NodeSet> {
        let count = self.node_count();
        let reachable = self.reachable_from(self.entry);
        let all_reachable = {
            let mut nodes = NodeSet::empty();
            for node in 0..count {
                if reachable.contains(node) {
                    nodes.insert(node);
                }
            }
            nodes
        };
        let mut dominators = vec![NodeSet::empty(); count];
        for (node, set) in dominators.iter_mut().enumerate() {
            if node == self.entry {
                set.insert(node);
            } else if reachable.contains(node) {
                *set = all_reachable;
            }
        }

        let mut traversal = self.depth_first_finish_order(self.entry, true);
        traversal.reverse();
        for _ in 0..=count {
            let mut changed = false;
            for node in traversal.iter().copied() {
                if node == self.entry || !reachable.contains(node) {
                    continue;
                }
                let mut incoming = self
                    .predecessors
                    .get(node)
                    .into_iter()
                    .flatten()
                    .copied()
                    .filter(|predecessor| reachable.contains(*predecessor));
                let Some(first) = incoming.next() else {
                    continue;
                };
                let mut next = dominators[first];
                for predecessor in incoming {
                    next.intersect(dominators[predecessor]);
                }
                next.insert(node);
                if next != dominators[node] {
                    dominators[node] = next;
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }
        dominators
    }

    /// True when `candidate` dominates `node`; every reachable node dominates itself.
    pub(crate) fn dominates(&self, dominators: &[NodeSet], candidate: usize, node: usize) -> bool {
        dominators
            .get(node)
            .is_some_and(|set| set.contains(candidate))
    }

    /// True when `candidate` dominates `node` and the two placements are distinct.
    pub(crate) fn strictly_dominates(
        &self,
        dominators: &[NodeSet],
        candidate: usize,
        node: usize,
    ) -> bool {
        candidate != node && self.dominates(dominators, candidate, node)
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

    /// The strongly connected components of the graph, each sorted in author order and the
    /// components themselves ordered by their author-earliest member.
    ///
    /// Components rather than simple cycles: enumerating every simple cycle is exponential in the
    /// graph size, while the component decomposition is linear, bounded, and the unit a reviewer
    /// actually reasons about — "this region loops" rather than "these 4,000 paths loop".
    pub(crate) fn strongly_connected_components(&self) -> Vec<Vec<usize>> {
        self.components(true)
    }

    /// Number of cyclic regions in the complete transition graph, including a singleton SCC with
    /// a self-loop. Preview reports regions rather than simple cycles: the latter can grow
    /// exponentially while an SCC is the bounded unit a reviewer can usefully inspect.
    pub(crate) fn cyclic_component_count(&self) -> usize {
        self.strongly_connected_components()
            .into_iter()
            .filter(|component| {
                component.len() > 1
                    || component.first().is_some_and(|node| {
                        self.successors(*node)
                            .iter()
                            .any(|edge| edge.target() == *node)
                    })
            })
            .count()
    }

    /// Cyclic regions in the graph after declared rework edges are removed. The v2 cycle rule is
    /// exactly that this advance-only subgraph is acyclic; checking full-graph components would
    /// miss an advance-only subcycle inside a larger component that also contains rework.
    pub(crate) fn advance_only_cycles(&self) -> Vec<Vec<usize>> {
        self.components(false)
            .into_iter()
            .filter(|component| {
                component.len() > 1
                    || component.first().is_some_and(|node| {
                        self.successors(*node).iter().any(|edge| {
                            edge.target() == *node
                                && edge.effect() != Some(TransitionEffectV2::Rework)
                        })
                    })
            })
            .collect()
    }

    fn components(&self, include_rework: bool) -> Vec<Vec<usize>> {
        let count = self.node_count();
        let mut visited = vec![false; count];
        let mut finish_order = Vec::with_capacity(count);
        for node in 0..count {
            if visited[node] {
                continue;
            }
            finish_order.extend(self.depth_first_finish_order_with_visited(
                node,
                include_rework,
                &mut visited,
            ));
        }

        let mut reverse = vec![Vec::new(); count];
        for source in 0..count {
            for edge in self.successors(source) {
                if include_rework || edge.effect() != Some(TransitionEffectV2::Rework) {
                    reverse[edge.target()].push(source);
                }
            }
        }

        let mut assigned = vec![false; count];
        let mut components = Vec::new();
        for start in finish_order.into_iter().rev() {
            if assigned[start] {
                continue;
            }
            assigned[start] = true;
            let mut stack = vec![start];
            let mut component = Vec::new();
            while let Some(node) = stack.pop() {
                component.push(node);
                for predecessor in reverse[node].iter().rev().copied() {
                    if !assigned[predecessor] {
                        assigned[predecessor] = true;
                        stack.push(predecessor);
                    }
                }
            }
            components.push(component);
        }
        for component in &mut components {
            component.sort_unstable();
        }
        components.sort_by_key(|component| component.first().copied().unwrap_or(usize::MAX));
        components
    }

    fn depth_first_finish_order(&self, start: usize, include_rework: bool) -> Vec<usize> {
        let mut visited = vec![false; self.node_count()];
        self.depth_first_finish_order_with_visited(start, include_rework, &mut visited)
    }

    fn depth_first_finish_order_with_visited(
        &self,
        start: usize,
        include_rework: bool,
        visited: &mut [bool],
    ) -> Vec<usize> {
        if start >= self.node_count() || visited[start] {
            return Vec::new();
        }
        visited[start] = true;
        let mut finish_order = Vec::new();
        let mut stack = vec![(start, 0usize)];
        while let Some((node, next_edge)) = stack.last_mut() {
            let edges = self.successors(*node);
            let mut target = None;
            while *next_edge < edges.len() {
                let edge = edges[*next_edge];
                *next_edge += 1;
                if include_rework || edge.effect() != Some(TransitionEffectV2::Rework) {
                    target = Some(edge.target());
                    break;
                }
            }
            if let Some(target) = target {
                if !visited[target] {
                    visited[target] = true;
                    stack.push((target, 0));
                }
            } else {
                finish_order.push(*node);
                stack.pop();
            }
        }
        finish_order
    }

    /// The least fixpoint `Unsafe = { n : ¬assessment(n) ∧ (terminal(n) ∨ ∃ s ∈ succ(n). s ∈ Unsafe) }`.
    ///
    /// A node is *unsafe* exactly when some path from it reaches a terminal action without passing
    /// a session-goal assessment; its complement is section 7.2's revision-safe target set, where
    /// "a path from the target includes the target itself", so an assessment placement is safe
    /// through its own placement. A cycle that never reaches a terminal contributes no path and
    /// therefore never enters the set — the fixpoint is least, not greatest.
    ///
    /// Computed as one reverse traversal from non-assessment terminal nodes. Assessment nodes are
    /// barriers, so every discovered predecessor has one terminal path that avoids assessment.
    pub(crate) fn assessment_free_to_terminal(
        &self,
        is_assessment: impl Fn(usize) -> bool,
    ) -> NodeSet {
        let mut unsafe_nodes = NodeSet::empty();
        let mut frontier = VecDeque::new();
        for node in 0..self.node_count() {
            if self.is_terminal(node) && !is_assessment(node) {
                unsafe_nodes.insert(node);
                frontier.push_back(node);
            }
        }
        while let Some(node) = frontier.pop_front() {
            for predecessor in &self.predecessors[node] {
                if !unsafe_nodes.contains(*predecessor) && !is_assessment(*predecessor) {
                    unsafe_nodes.insert(*predecessor);
                    frontier.push_back(*predecessor);
                }
            }
        }
        unsafe_nodes
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use podway_core::{
        ActionPlacementV2, DecisionPlacementV2, DecisionRouteEntryV2, DecisionRouteMapV2,
        DecisionRouteV2, GraphNodeId, NodeDefinitionId, OptionId,
    };

    #[test]
    fn goal_revision_targets_require_an_assessment_on_every_terminal_path() {
        let source = br#"{
          "schema":"podway.procedure/v2",
          "id":"revision-safety",
          "version":"1",
          "name":"Revision safety",
          "purpose":"Separate manual rework from safe goal revision.",
          "goal_tracking":true,
          "node_definitions":{
            "assess":{"type":"decision","title":"Assess","objective":"Assess goal.","prompt":"Outcome?","options":[{"id":"achieved","label":"Achieved"},{"id":"not-achieved","label":"Not achieved"},{"id":"superseded","label":"Superseded"}],"reason":{"required":true},"assessment":{"target":"session_goal","outcomes":{"achieved":"achieved","not-achieved":"not_achieved","superseded":"superseded"}}},
            "work":{"type":"action","title":"Work","intent":"Do work."},
            "finish":{"type":"action","title":"Finish","intent":"Finish."}
          },
          "graph":{
            "entry":"assessment",
            "nodes":[
              {"id":"assessment","use":"assess","routes":{"achieved":{"to":"work","effect":"advance"},"not-achieved":{"to":"work","effect":"advance"},"superseded":{"to":"work","effect":"advance"}}},
              {"id":"work","use":"work","next":"finish"},
              {"id":"finish","use":"finish","terminal":true}
            ]
          },
          "manual_rework":{"allowed_targets":["assessment","work"]}
        }"#;
        let crate::ParsedProcedure::V2(parsed) =
            crate::parse_procedure_document(source, crate::ProcedureDocumentFormat::Json)
                .expect("fixture must parse");

        let safe = goal_revision_safe_targets_v2(&parsed);
        assert_eq!(
            safe.iter().map(GraphNodeId::as_str).collect::<Vec<_>>(),
            vec!["assessment"]
        );
    }

    #[test]
    fn v2gsc002_full_length_chain_uses_bounded_iterative_analysis() {
        let ids: Vec<GraphNodeId> = (0..podway_core::MAX_GRAPH_NODES_PER_PROCEDURE_V2)
            .map(|node| GraphNodeId::new(format!("n-{node}")).expect("test id"))
            .collect();
        let definition = NodeDefinitionId::new("work").expect("test definition id");
        let placements: Vec<GraphPlacementV2> = ids
            .iter()
            .enumerate()
            .map(|(node, id)| {
                let outcome = ids
                    .get(node + 1)
                    .cloned()
                    .map_or_else(ActionOutcomeV2::terminal, ActionOutcomeV2::next);
                GraphPlacementV2::Action(ActionPlacementV2::new(
                    id.clone(),
                    definition.clone(),
                    None,
                    None,
                    outcome,
                ))
            })
            .collect();
        let graph = ProcedureGraphV2::new(ids[0].clone(), placements, None).expect("full graph");
        let index = GraphIndex::new(&graph);
        let last = ids.len() - 1;

        assert_eq!(index.distances_from(0)[last], Some(last as u32));
        assert!(index.terminal_reachable_nodes().contains(0));
        assert_eq!(index.strongly_connected_components().len(), ids.len());
        assert!(index.dominates(&index.dominators(), 0, last));
        assert!(index.assessment_free_to_terminal(|_| false).contains(0));
    }

    #[test]
    fn v2gsc004_production_index_crosses_three_words_for_cycles_and_dominators() {
        const NODE_COUNT: usize = 131;
        let ids: Vec<GraphNodeId> = (0..NODE_COUNT)
            .map(|node| GraphNodeId::new(format!("n-{node}")).expect("test id"))
            .collect();
        let action = NodeDefinitionId::new("work").unwrap();
        let decision = NodeDefinitionId::new("review").unwrap();
        let routes = |entries: Vec<(&str, usize, TransitionEffectV2)>| {
            DecisionRouteMapV2::new(
                entries
                    .into_iter()
                    .map(|(option, target, effect)| {
                        DecisionRouteEntryV2::new(
                            OptionId::new(option).unwrap(),
                            DecisionRouteV2::new(ids[target].clone(), effect),
                        )
                    })
                    .collect(),
            )
            .unwrap()
        };
        let placements = (0..NODE_COUNT)
            .map(|node| match node {
                125 => GraphPlacementV2::Decision(DecisionPlacementV2::new(
                    ids[node].clone(),
                    decision.clone(),
                    None,
                    routes(vec![
                        ("left", 126, TransitionEffectV2::Advance),
                        ("right", 127, TransitionEffectV2::Advance),
                    ]),
                )),
                128 => GraphPlacementV2::Decision(DecisionPlacementV2::new(
                    ids[node].clone(),
                    decision.clone(),
                    None,
                    routes(vec![
                        ("cycle", 129, TransitionEffectV2::Advance),
                        ("finish", 130, TransitionEffectV2::Advance),
                        ("retry", 125, TransitionEffectV2::Rework),
                    ]),
                )),
                126 | 127 => GraphPlacementV2::Action(ActionPlacementV2::new(
                    ids[node].clone(),
                    action.clone(),
                    None,
                    None,
                    ActionOutcomeV2::next(ids[128].clone()),
                )),
                129 => GraphPlacementV2::Action(ActionPlacementV2::new(
                    ids[node].clone(),
                    action.clone(),
                    None,
                    None,
                    ActionOutcomeV2::next(ids[128].clone()),
                )),
                130 => GraphPlacementV2::Action(ActionPlacementV2::new(
                    ids[node].clone(),
                    action.clone(),
                    None,
                    None,
                    ActionOutcomeV2::terminal(),
                )),
                _ => GraphPlacementV2::Action(ActionPlacementV2::new(
                    ids[node].clone(),
                    action.clone(),
                    None,
                    None,
                    ActionOutcomeV2::next(ids[node + 1].clone()),
                )),
            })
            .collect();
        let graph = ProcedureGraphV2::new(ids[0].clone(), placements, None).unwrap();
        let index = GraphIndex::new(&graph);

        assert!(
            index
                .strongly_connected_components()
                .iter()
                .any(|component| component == &[125, 126, 127, 128, 129])
        );
        assert_eq!(index.advance_only_cycles(), vec![vec![128, 129]]);
        let dominators = index.dominators();
        assert!(index.dominates(&dominators, 125, 128));
        assert!(!index.dominates(&dominators, 126, 128));
        assert!(!index.dominates(&dominators, 127, 128));
        assert!(index.terminal_reachable_nodes().contains(0));
    }

    #[test]
    fn v2grf001_assessment_free_reachability_is_existential_across_branches() {
        let ids =
            ["branch", "direct", "assessment", "guarded"].map(|id| GraphNodeId::new(id).unwrap());
        let action = NodeDefinitionId::new("work").unwrap();
        let decision = NodeDefinitionId::new("review").unwrap();
        let route_map = |entries: Vec<(&str, usize)>| {
            DecisionRouteMapV2::new(
                entries
                    .into_iter()
                    .map(|(option, target)| {
                        DecisionRouteEntryV2::new(
                            OptionId::new(option).unwrap(),
                            DecisionRouteV2::new(ids[target].clone(), TransitionEffectV2::Advance),
                        )
                    })
                    .collect(),
            )
            .unwrap()
        };
        let graph = ProcedureGraphV2::new(
            ids[0].clone(),
            vec![
                GraphPlacementV2::Decision(DecisionPlacementV2::new(
                    ids[0].clone(),
                    decision.clone(),
                    None,
                    route_map(vec![("unsafe", 1), ("assessed", 2)]),
                )),
                GraphPlacementV2::Action(ActionPlacementV2::new(
                    ids[1].clone(),
                    action.clone(),
                    None,
                    None,
                    ActionOutcomeV2::terminal(),
                )),
                GraphPlacementV2::Decision(DecisionPlacementV2::new(
                    ids[2].clone(),
                    decision,
                    None,
                    route_map(vec![("continue", 3)]),
                )),
                GraphPlacementV2::Action(ActionPlacementV2::new(
                    ids[3].clone(),
                    action,
                    None,
                    None,
                    ActionOutcomeV2::terminal(),
                )),
            ],
            None,
        )
        .unwrap();
        let index = GraphIndex::new(&graph);
        let unsafe_nodes = index.assessment_free_to_terminal(|node| node == 2);

        assert!(unsafe_nodes.contains(0));
        assert!(unsafe_nodes.contains(1));
        assert!(!unsafe_nodes.contains(2));
        assert!(unsafe_nodes.contains(3));
    }

    #[test]
    fn v2gsc004_graph_analysis_crosses_the_first_node_set_word() {
        const NODE_COUNT: usize = 70;
        let ids: Vec<GraphNodeId> = (0..NODE_COUNT)
            .map(|node| GraphNodeId::new(format!("n-{node}")).expect("test id"))
            .collect();
        let definition = NodeDefinitionId::new("work").expect("test definition id");
        let placements: Vec<GraphPlacementV2> = ids
            .iter()
            .enumerate()
            .map(|(node, id)| {
                let outcome = ids
                    .get(node + 1)
                    .cloned()
                    .map_or_else(ActionOutcomeV2::terminal, ActionOutcomeV2::next);
                GraphPlacementV2::Action(ActionPlacementV2::new(
                    id.clone(),
                    definition.clone(),
                    None,
                    None,
                    outcome,
                ))
            })
            .collect();
        let index_by_id = placements
            .iter()
            .enumerate()
            .map(|(index, placement)| (placement.id().as_str(), index))
            .collect();
        let mut successors = vec![Vec::new(); NODE_COUNT];
        for (source, edges) in successors.iter_mut().enumerate().take(63) {
            edges.push(Successor {
                target: source + 1,
                effect: Some(TransitionEffectV2::Advance),
            });
        }
        successors[63].push(Successor {
            target: 64,
            effect: Some(TransitionEffectV2::Advance),
        });
        successors[64].extend([
            Successor {
                target: 63,
                effect: Some(TransitionEffectV2::Rework),
            },
            Successor {
                target: 65,
                effect: Some(TransitionEffectV2::Advance),
            },
        ]);
        successors[65].extend([
            Successor {
                target: 66,
                effect: Some(TransitionEffectV2::Advance),
            },
            Successor {
                target: 67,
                effect: Some(TransitionEffectV2::Advance),
            },
        ]);
        successors[66].push(Successor {
            target: 68,
            effect: Some(TransitionEffectV2::Advance),
        });
        successors[67].push(Successor {
            target: 68,
            effect: Some(TransitionEffectV2::Advance),
        });
        successors[68].push(Successor {
            target: 69,
            effect: Some(TransitionEffectV2::Advance),
        });
        let mut predecessors = vec![Vec::new(); NODE_COUNT];
        for (source, edges) in successors.iter().enumerate() {
            for edge in edges {
                predecessors[edge.target()].push(source);
            }
        }
        let graph = GraphIndex {
            placements: &placements,
            index_by_id,
            successors,
            predecessors,
            entry: 0,
        };

        assert!(graph.reachable_from(0).contains(69));
        assert!(
            graph
                .strongly_connected_components()
                .iter()
                .any(|component| component == &[63, 64])
        );
        let dominators = graph.dominators();
        assert!(graph.dominates(&dominators, 64, 68));
        assert!(graph.dominates(&dominators, 65, 68));
        assert!(!graph.dominates(&dominators, 66, 68));
        assert!(!graph.dominates(&dominators, 67, 68));
        assert!(graph.terminal_reachable_nodes().contains(64));
        assert!(graph.advance_only_cycles().is_empty());

        let assessment_free = graph.assessment_free_to_terminal(|node| node == 64);
        assert!(assessment_free.contains(65));
        assert!(!assessment_free.contains(64));
        assert!(!assessment_free.contains(63));

        let mut advance_successors = graph.successors.clone();
        advance_successors[64][0].effect = Some(TransitionEffectV2::Advance);
        let mut advance_predecessors = vec![Vec::new(); NODE_COUNT];
        for (source, edges) in advance_successors.iter().enumerate() {
            for edge in edges {
                advance_predecessors[edge.target()].push(source);
            }
        }
        let advance_graph = GraphIndex {
            placements: &placements,
            index_by_id: graph.index_by_id.clone(),
            successors: advance_successors,
            predecessors: advance_predecessors,
            entry: 0,
        };
        assert!(
            advance_graph
                .advance_only_cycles()
                .iter()
                .any(|component| component == &[63, 64])
        );

        let mut isolated_cycle_successors = vec![Vec::new(); NODE_COUNT];
        isolated_cycle_successors[0].push(Successor {
            target: 1,
            effect: Some(TransitionEffectV2::Advance),
        });
        isolated_cycle_successors[1].push(Successor {
            target: 0,
            effect: Some(TransitionEffectV2::Advance),
        });
        let mut isolated_cycle_predecessors = vec![Vec::new(); NODE_COUNT];
        for (source, edges) in isolated_cycle_successors.iter().enumerate() {
            for edge in edges {
                isolated_cycle_predecessors[edge.target()].push(source);
            }
        }
        let isolated_cycle_graph = GraphIndex {
            placements: &placements,
            index_by_id: graph.index_by_id.clone(),
            successors: isolated_cycle_successors,
            predecessors: isolated_cycle_predecessors,
            entry: 0,
        };
        let isolated_assessment_free = isolated_cycle_graph.assessment_free_to_terminal(|_| false);
        assert!(!isolated_assessment_free.contains(0));
        assert!(!isolated_assessment_free.contains(1));
        assert!(isolated_assessment_free.contains(69));
    }

    /// Runs a check against a synthetic three-node index. The placements provide terminal
    /// identity; adjacency is supplied independently so all 512 directed graphs can be exhausted
    /// without constructing 512 authoring documents.
    fn with_index(test_graph: u16, rework_back_edges: bool, test: impl FnOnce(&GraphIndex<'_>)) {
        let ids: Vec<GraphNodeId> = (0..3)
            .map(|node| GraphNodeId::new(format!("n-{node}")).expect("test id"))
            .collect();
        let definition = NodeDefinitionId::new("work").expect("test definition id");
        let placements: Vec<GraphPlacementV2> = ids
            .iter()
            .enumerate()
            .map(|(node, id)| {
                let outcome = if node == 2 {
                    ActionOutcomeV2::terminal()
                } else {
                    ActionOutcomeV2::next(ids[node].clone())
                };
                GraphPlacementV2::Action(ActionPlacementV2::new(
                    id.clone(),
                    definition.clone(),
                    None,
                    None,
                    outcome,
                ))
            })
            .collect();
        let index_by_id = placements
            .iter()
            .enumerate()
            .map(|(index, placement)| (placement.id().as_str(), index))
            .collect();
        let successors: Vec<Vec<Successor>> = (0..3)
            .map(|source| {
                (0..3)
                    .filter(|target| test_graph & (1 << (source * 3 + target)) != 0)
                    .map(|target| Successor {
                        target,
                        effect: Some(if rework_back_edges && target <= source {
                            TransitionEffectV2::Rework
                        } else {
                            TransitionEffectV2::Advance
                        }),
                    })
                    .collect()
            })
            .collect();
        let mut predecessors = vec![Vec::new(); 3];
        for (source, edges) in successors.iter().enumerate() {
            for edge in edges {
                predecessors[edge.target()].push(source);
            }
        }
        let graph = GraphIndex {
            placements: &placements,
            index_by_id,
            successors,
            predecessors,
            entry: 0,
        };
        test(&graph);
    }

    fn reference_reachable(
        graph: &GraphIndex<'_>,
        start: usize,
        blocked: Option<usize>,
    ) -> NodeSet {
        let mut result = NodeSet::empty();
        if blocked == Some(start) {
            return result;
        }
        let mut frontier = VecDeque::from([start]);
        result.insert(start);
        while let Some(node) = frontier.pop_front() {
            for edge in graph.successors(node) {
                if blocked == Some(edge.target()) || result.contains(edge.target()) {
                    continue;
                }
                result.insert(edge.target());
                frontier.push_back(edge.target());
            }
        }
        result
    }

    fn reference_has_cycle(graph: &GraphIndex<'_>) -> bool {
        fn visit(graph: &GraphIndex<'_>, node: usize, marks: &mut [u8]) -> bool {
            marks[node] = 1;
            for edge in graph.successors(node) {
                if marks[edge.target()] == 1
                    || (marks[edge.target()] == 0 && visit(graph, edge.target(), marks))
                {
                    return true;
                }
            }
            marks[node] = 2;
            false
        }

        let mut marks = vec![0; graph.node_count()];
        (0..graph.node_count()).any(|node| marks[node] == 0 && visit(graph, node, &mut marks))
    }

    fn visit_valid_traces(
        graph: &GraphIndex<'_>,
        remaining_edges: usize,
        trace: &mut Vec<usize>,
        visit: &mut impl FnMut(&[usize]),
    ) {
        visit(trace);
        if remaining_edges == 0 {
            return;
        }
        let current = *trace.last().expect("a valid trace starts at entry");
        for edge in graph.successors(current) {
            trace.push(edge.target());
            visit_valid_traces(graph, remaining_edges - 1, trace, visit);
            trace.pop();
        }
    }

    #[derive(Clone, Copy, Debug)]
    enum ModeledReentry {
        Retry,
        DeclaredRework,
        ManualRework,
        GoalRework,
    }

    #[derive(Clone, Copy, Debug)]
    struct ModeledAttempt {
        sequence: usize,
        node: usize,
        valid: bool,
    }

    fn valid_attempts(trace: &[ModeledAttempt]) -> Vec<ModeledAttempt> {
        trace
            .iter()
            .copied()
            .filter(|attempt| attempt.valid)
            .collect()
    }

    fn assert_current_trace_invariants(graph: &GraphIndex<'_>, trace: &[ModeledAttempt]) {
        let valid = valid_attempts(trace);
        assert_eq!(
            valid.first().map(|attempt| attempt.node),
            Some(graph.entry()),
            "the current valid trace must start at entry: {trace:?}"
        );
        for pair in valid.windows(2) {
            assert!(
                graph
                    .successors(pair[0].node)
                    .iter()
                    .any(|edge| edge.target() == pair[1].node),
                "the current valid trace must remain a graph path: {trace:?}"
            );
        }

        let active = valid.last().expect("a current trace has an active attempt");
        for candidate in 0..graph.node_count() {
            let dominates = candidate == active.node
                || !reference_reachable(graph, graph.entry(), Some(candidate))
                    .contains(active.node);
            if dominates {
                assert_eq!(
                    valid
                        .iter()
                        .filter(|attempt| attempt.node == candidate)
                        .count(),
                    1,
                    "dominator {candidate} must have exactly one valid attempt for active node {}: {trace:?}",
                    active.node
                );
            }
        }
    }

    fn model_reentry(
        graph: &GraphIndex<'_>,
        trace: &[ModeledAttempt],
        target_position: usize,
        mode: ModeledReentry,
    ) -> Vec<ModeledAttempt> {
        let old_valid = valid_attempts(trace);
        let target = old_valid[target_position];
        let mut next = trace.to_vec();
        for attempt in &mut next {
            if attempt.valid && attempt.sequence >= target.sequence {
                attempt.valid = false;
            }
        }
        next.push(ModeledAttempt {
            sequence: trace.len(),
            node: target.node,
            valid: true,
        });

        for old in &old_valid[..target_position] {
            assert!(
                next.iter()
                    .any(|attempt| attempt.sequence == old.sequence && attempt.valid),
                "{mode:?} must preserve the valid prefix"
            );
        }
        for old in &old_valid[target_position..] {
            assert!(
                next.iter()
                    .any(|attempt| attempt.sequence == old.sequence && !attempt.valid),
                "{mode:?} must stale the complete suffix"
            );
        }
        let expected_nodes = old_valid[..=target_position]
            .iter()
            .map(|attempt| attempt.node)
            .collect::<Vec<_>>();
        assert_eq!(
            valid_attempts(&next)
                .iter()
                .map(|attempt| attempt.node)
                .collect::<Vec<_>>(),
            expected_nodes,
            "{mode:?} must replace the target attempt without changing the surviving path"
        );
        assert!(
            target_position == 0
                || graph
                    .successors(old_valid[target_position - 1].node)
                    .iter()
                    .any(|edge| edge.target() == target.node),
            "the fresh target must re-extend the preserved path"
        );
        next
    }

    fn exercise_reentry_traces(
        graph: &GraphIndex<'_>,
        trace: Vec<ModeledAttempt>,
        remaining_transitions: usize,
        exercised: &mut [bool; 4],
    ) {
        assert_current_trace_invariants(graph, &trace);
        if remaining_transitions == 0 {
            return;
        }

        let valid = valid_attempts(&trace);
        let active = valid.last().expect("generated trace has an active attempt");

        let retry = model_reentry(graph, &trace, valid.len() - 1, ModeledReentry::Retry);
        exercised[0] = true;
        exercise_reentry_traces(graph, retry, remaining_transitions - 1, exercised);

        for target_position in 0..valid.len() {
            for (mode, coverage) in [
                (ModeledReentry::ManualRework, 2),
                (ModeledReentry::GoalRework, 3),
            ] {
                let next = model_reentry(graph, &trace, target_position, mode);
                exercised[coverage] = true;
                exercise_reentry_traces(graph, next, remaining_transitions - 1, exercised);
            }
        }

        for edge in graph.successors(active.node) {
            if edge.effect() == Some(TransitionEffectV2::Rework) {
                if let Some(target_position) = valid
                    .iter()
                    .position(|attempt| attempt.node == edge.target())
                {
                    let target_dominates_active = edge.target() == active.node
                        || !reference_reachable(graph, graph.entry(), Some(edge.target()))
                            .contains(active.node);
                    if !target_dominates_active {
                        continue;
                    }
                    let next = model_reentry(
                        graph,
                        &trace,
                        target_position,
                        ModeledReentry::DeclaredRework,
                    );
                    exercised[1] = true;
                    exercise_reentry_traces(graph, next, remaining_transitions - 1, exercised);
                }
            } else if !valid.iter().any(|attempt| attempt.node == edge.target()) {
                let mut next = trace.clone();
                next.push(ModeledAttempt {
                    sequence: trace.len(),
                    node: edge.target(),
                    valid: true,
                });
                exercise_reentry_traces(graph, next, remaining_transitions - 1, exercised);
            }
        }
    }

    #[test]
    fn v2grf001_rework_preserves_dominance_over_generated_valid_execution_traces() {
        let mut exercised = [false; 4];
        for encoded in 0..(1_u16 << 9) {
            with_index(encoded, true, |graph| {
                exercise_reentry_traces(
                    graph,
                    vec![ModeledAttempt {
                        sequence: 0,
                        node: graph.entry(),
                        valid: true,
                    }],
                    4,
                    &mut exercised,
                );
            });
        }
        assert_eq!(
            exercised, [true; 4],
            "retry, declared rework, manual rework, and goal rework must all be generated"
        );
    }

    #[test]
    fn v2grf001_graph_analyses_match_independent_exhaustive_three_node_models() {
        for encoded in 0..(1_u16 << 9) {
            with_index(encoded, false, |graph| {
                let reachable = reference_reachable(graph, graph.entry(), None);
                assert_eq!(
                    graph.reachable_from(graph.entry()),
                    reachable,
                    "graph {encoded:#x}"
                );

                let terminal_reachable = graph.terminal_reachable_nodes();
                for node in 0..graph.node_count() {
                    let expected = reference_reachable(graph, node, None).contains(2);
                    assert_eq!(
                        terminal_reachable.contains(node),
                        expected,
                        "terminal reachability for node {node}, graph {encoded:#x}"
                    );
                }

                let dominators = graph.dominators();
                for node in 0..graph.node_count() {
                    for candidate in 0..graph.node_count() {
                        let expected = reachable.contains(node)
                            && (candidate == node
                                || !reference_reachable(graph, graph.entry(), Some(candidate))
                                    .contains(node));
                        assert_eq!(
                            graph.dominates(&dominators, candidate, node),
                            expected,
                            "dominance {candidate} -> {node}, graph {encoded:#x}"
                        );
                    }
                }

                let mut trace = vec![graph.entry()];
                visit_valid_traces(graph, 5, &mut trace, &mut |trace| {
                    let active = *trace.last().expect("generated trace is non-empty");
                    for candidate in 0..graph.node_count() {
                        if graph.dominates(&dominators, candidate, active) {
                            assert!(
                                trace.contains(&candidate),
                                "dominator {candidate} missing from trace {trace:?}, graph {encoded:#x}"
                            );
                        }
                    }
                });

                assert_eq!(
                    !graph.advance_only_cycles().is_empty(),
                    reference_has_cycle(graph),
                    "cycle property for graph {encoded:#x}"
                );
            });
        }
    }
}
