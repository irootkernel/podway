use super::*;
use crate::procedure_v2_diagnostics::diagnostic_hint;
use crate::procedure_v2_document::{
    AuthoringValue as A, condition_value, item_value, node_definition_value,
};
use crate::procedure_v2_format::{drift_position, emit_json, emit_yaml};
use crate::procedure_v2_source::{build_source_index, scan_source};
use crate::{AuthoringContext, FormatRequest, ParsedNodeDefinition, ProcedureDocumentFormat};
use podway_core::{
    ActionOutcomeV2, AuthoringDiagnostic, AuthoringDiagnosticCode, ItemConditionV2,
    MAX_PROCEDURE_DOCUMENT_BYTES, SOURCE_PROJECTION_MAX_CHARACTERS,
};
use sha2::{Digest as _, Sha256};
use std::fmt::Write as _;

fn text(value: &str) -> A {
    A::Text(value.into())
}
fn strings<'a>(values: impl IntoIterator<Item = &'a str>) -> A {
    A::Seq(values.into_iter().map(text).collect())
}
fn field(key: &str, value: A) -> (String, A) {
    (key.into(), value)
}
fn source(value: &EvidenceSourceV3) -> A {
    A::Map(match value {
        EvidenceSourceV3::Local(id) => vec![
            field("scope", text("local")),
            field("node", text(id.as_str())),
        ],
        EvidenceSourceV3::Plan => vec![field("scope", text("plan"))],
        EvidenceSourceV3::ExecutionResult => vec![field("scope", text("execution_result"))],
    })
}
fn definition(value: &NodeDefinitionV3) -> A {
    let shared = match value {
        NodeDefinitionV3::Action(v) => ParsedNodeDefinition::Action(v.clone()),
        NodeDefinitionV3::Decision(v) => ParsedNodeDefinition::Decision(v.metadata().clone()),
    };
    let mut document = node_definition_value(&shared);
    if let (NodeDefinitionV3::Decision(decision), A::Map(fields)) = (value, &mut document)
        && let Some((_, A::Seq(options))) = fields.iter_mut().find(|(key, _)| key == "options")
    {
        for (option, metadata) in options.iter_mut().zip(decision.metadata().options()) {
            if let (A::Map(fields), Some(guards)) = (option, decision.guards(metadata.id())) {
                let predicates = guards
                    .predicates()
                    .iter()
                    .map(|guard| {
                        // Reuse the shared operator/default projection; replace only its source.
                        let condition = ItemConditionV2::new(vec![guard.predicate().clone()])
                            .expect("one existing typed predicate");
                        let A::Seq(mut values) = condition_value(&condition) else {
                            unreachable!()
                        };
                        let A::Map(mut fields) = values.remove(0) else {
                            unreachable!()
                        };
                        fields.remove(0);
                        let A::Map(mut evidence) = source(guard.source()) else {
                            unreachable!()
                        };
                        evidence.push(field("item", text(guard.predicate().item().as_str())));
                        fields.insert(0, field("evidence", A::Map(evidence)));
                        A::Map(fields)
                    })
                    .collect();
                fields.push(field("guards", A::Seq(predicates)));
            }
        }
    }
    document
}
fn placement(value: &PlacementV3) -> A {
    let mut fields = vec![
        field("id", text(value.id().as_str())),
        field("use", text(value.definition().as_str())),
    ];
    if !value.evidence().is_empty() {
        fields.push(field(
            "evidence_from",
            A::Seq(
                value
                    .evidence()
                    .iter()
                    .map(|r| {
                        let A::Map(mut fields) = source(r.source()) else {
                            unreachable!()
                        };
                        fields.push(field("required", A::Flag(r.required())));
                        if !r.items().is_empty() {
                            fields.push(field(
                                "items",
                                strings(r.items().iter().map(|v| v.as_str())),
                            ));
                        }
                        A::Map(fields)
                    })
                    .collect(),
            ),
        ));
    }
    match value.kind() {
        PlacementKindV3::Action {
            outcome,
            skip,
            delegation,
        } => {
            if let Some(d) = delegation {
                fields.push(field(
                    "delegation",
                    A::Map(vec![
                        field("binding_ref", text(d.binding_ref().as_str())),
                        field("role_ref", text(d.role_ref().as_str())),
                        field(
                            "role_snapshot_digest",
                            text(d.role_snapshot_digest().as_str()),
                        ),
                        field("max_rounds", A::Integer(i64::from(d.max_rounds()))),
                        field("after_accept", text(d.after_accept().as_str())),
                    ]),
                ));
            }
            if let Some(s) = skip {
                fields.push(field(
                    "skip",
                    A::Map(vec![
                        field("allowed", A::Flag(s.is_allowed())),
                        field("reason_required", A::Flag(s.reason_required())),
                    ]),
                ));
            }
            match outcome {
                ActionOutcomeV2::Next(to) => fields.push(field("next", text(to.as_str()))),
                ActionOutcomeV2::Terminal => fields.push(field("terminal", A::Flag(true))),
            }
        }
        PlacementKindV3::Decision { routes } => fields.push(field(
            "routes",
            A::Map(
                routes
                    .iter()
                    .map(|(option, route)| {
                        let value = match route {
                            DecisionRouteV3::Advance(to) => vec![
                                field("to", text(to.as_str())),
                                field("effect", text("advance")),
                            ],
                            DecisionRouteV3::Rework(to) => vec![
                                field("to", text(to.as_str())),
                                field("effect", text("rework")),
                            ],
                            DecisionRouteV3::Replan => vec![field("effect", text("replan"))],
                        };
                        (option.as_str().into(), A::Map(value))
                    })
                    .collect(),
            ),
        )),
    }
    A::Map(fields)
}
pub(super) fn authoring(value: &SuccessorDefinition) -> A {
    let header = value.header();
    let mut fields = vec![
        field("schema", text(value.schema())),
        field("id", text(header.id().as_str())),
        field("version", text(header.version())),
        field("name", text(header.name())),
        field("purpose", text(header.purpose())),
    ];
    if let Some(description) = header.description() {
        fields.push(field("description", text(description)));
    }
    if matches!(value,SuccessorDefinition::Base(v) if v.goal_tracking().is_some_and(|v|v.is_enabled()))
    {
        fields.push(field("goal_tracking", A::Flag(true)));
    }
    fields.push(field(
        "node_definitions",
        A::Map(
            value
                .definitions()
                .iter()
                .map(|v| (v.id().as_str().into(), definition(v)))
                .collect(),
        ),
    ));
    fields.push(field(
        "graph",
        A::Map(vec![
            field("entry", text(value.graph().entry().as_str())),
            field(
                "nodes",
                A::Seq(value.graph().nodes().iter().map(placement).collect()),
            ),
        ]),
    ));
    if !value.graph().manual_rework().is_empty() {
        fields.push(field(
            "manual_rework",
            A::Map(vec![field(
                "allowed_targets",
                strings(value.graph().manual_rework().iter().map(|v| v.as_str())),
            )]),
        ));
    }
    match value {
        SuccessorDefinition::Base(base) => {
            let b = base.boundary();
            fields.push(field(
                "execution_group",
                A::Map(vec![
                    field("anchor", text(b.anchor().as_str())),
                    field("success_return", text(b.success_return().as_str())),
                    field(
                        "plan_inputs",
                        strings(b.plan_inputs().iter().map(|v| v.as_str())),
                    ),
                    field(
                        "result_items",
                        A::Seq(b.result_items().iter().map(item_value).collect()),
                    ),
                    field(
                        "replan_decisions",
                        strings(b.replan_decisions().iter().map(|v| v.as_str())),
                    ),
                ]),
            ));
        }
        SuccessorDefinition::Group(group) => fields.push(field(
            "plan_inputs",
            A::Seq(group.plan_inputs().iter().map(item_value).collect()),
        )),
    }
    A::Map(fields)
}
pub(crate) const SUCCESSOR_SOURCE_CHARACTERS_FIELD: &str = "successor source projection characters";
pub(crate) const SUCCESSOR_SOURCE_BYTES_FIELD: &str = "successor source projection bytes";
pub(crate) const SUCCESSOR_FORMATTED_CHARACTERS_FIELD: &str =
    "successor formatted source characters";

pub(super) fn bound(value: &str) -> Result<(), ConfigError> {
    crate::validate_count(
        SUCCESSOR_SOURCE_CHARACTERS_FIELD,
        value.chars().count(),
        1,
        SOURCE_PROJECTION_MAX_CHARACTERS,
    )?;
    crate::validate_count(
        SUCCESSOR_SOURCE_BYTES_FIELD,
        value.len(),
        1,
        MAX_PROCEDURE_DOCUMENT_BYTES,
    )
}
pub(super) fn digest(bytes: &[u8]) -> Result<Sha256Digest, ConfigError> {
    Sha256Digest::new(format!("sha256:{:x}", Sha256::digest(bytes)))
        .map_err(|_| ConfigError::InvalidDigest)
}

fn bound_formatted(value: &str) -> Result<(), ConfigError> {
    crate::validate_count(
        SUCCESSOR_FORMATTED_CHARACTERS_FIELD,
        value.chars().count(),
        1,
        SOURCE_PROJECTION_MAX_CHARACTERS,
    )
}

/// Canonical model-derived source, without comments or runtime state.
pub fn project_successor_source(
    value: &ValidatedSuccessorDefinition,
    format: ProcedureDocumentFormat,
) -> Result<String, ConfigError> {
    let document = match format {
        ProcedureDocumentFormat::Json => emit_json(&value.authoring),
        ProcedureDocumentFormat::Yaml => emit_yaml(
            &value.authoring,
            scan_source("", &build_source_index(""))
                .map_err(|_| invalid("source", "empty comment source is invalid"))?,
        ),
    };
    bound_formatted(&document)?;
    Ok(document)
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FormattedSuccessorDefinition {
    document: String,
    digest: Sha256Digest,
    changed: bool,
}
impl FormattedSuccessorDefinition {
    pub fn document(&self) -> &str {
        &self.document
    }
    pub fn digest(&self) -> &Sha256Digest {
        &self.digest
    }
    pub const fn changed(&self) -> bool {
        self.changed
    }

    pub fn drift_diagnostic(&self, context: &AuthoringContext<'_>) -> Option<AuthoringDiagnostic> {
        if context.source() == self.document {
            return None;
        }
        let (line, column) = drift_position(context.source(), &self.document);
        let code = AuthoringDiagnosticCode::FormatNotCanonical;
        Some(AuthoringDiagnostic::new(
            code,
            context.source_path(),
            context.span(line, column),
            "$",
            "The source is not in canonical authoring form at this line.",
            diagnostic_hint(code),
        ))
    }
}
/// One bounded admission feeds the same authoring tree, with existing supported YAML comments.
pub fn format_successor_definition(
    request: FormatRequest<'_>,
) -> Result<FormattedSuccessorDefinition, ConfigError> {
    let value = super::validate_successor_definition(super::parse_successor_document(
        request.source.as_bytes(),
        request.format,
    )?)?;
    let document = match request.format {
        ProcedureDocumentFormat::Json => emit_json(&value.authoring),
        ProcedureDocumentFormat::Yaml => {
            let comments = scan_source(request.source, &build_source_index(request.source))
                .map_err(|_| {
                    invalid(
                        "source",
                        "source constructs cannot be represented by canonical formatting",
                    )
                })?;
            emit_yaml(&value.authoring, comments)
        }
    };
    bound_formatted(&document)?;
    Ok(FormattedSuccessorDefinition {
        changed: request.source.as_bytes() != document.as_bytes(),
        document,
        digest: value.digest,
    })
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SuccessorGraphProjection {
    projection: CanonicalJsonV1,
    digest: Sha256Digest,
}
impl SuccessorGraphProjection {
    pub fn projection(&self) -> &str {
        self.projection.as_str()
    }
    pub fn digest(&self) -> &Sha256Digest {
        &self.digest
    }
}
/// An explicit replan edge has no target. A base's anchor edge denotes its declared group port.
pub fn project_successor_graph(
    value: &ValidatedSuccessorDefinition,
) -> Result<SuccessorGraphProjection, ConfigError> {
    let projection = project_retained_successor_graph(value)?;
    bound(projection.projection())?;
    Ok(projection)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SuccessorGraphFormat {
    Mermaid,
    Dot,
    PlantUml,
}

/// Render the same bounded topology exposed by the canonical DAG projection.
pub fn render_successor_graph(
    definition: &ValidatedSuccessorDefinition,
    format: SuccessorGraphFormat,
) -> Result<(String, Sha256Digest), ConfigError> {
    use SuccessorGraphFormat::{Dot, Mermaid, PlantUml};

    let graph = project_successor_graph(definition)?;
    let graph: serde_json::Value = serde_json::from_str(graph.projection())
        .map_err(|_| invalid("graph", "cannot decode the canonical graph projection"))?;
    let nodes = graph["nodes"].as_array().expect("projection has nodes");
    let edges = graph["edges"].as_array().expect("projection has edges");
    let anchor = graph["execution_group"]["anchor"].as_str();
    let has_replan = edges.iter().any(|edge| edge["effect"] == "replan");
    let mut output = match format {
        Mermaid => String::from("flowchart TD\n"),
        Dot => String::from("digraph Podway {\n"),
        PlantUml => String::from("@startuml\nhide empty description\n"),
    };
    let alias = |id: &str| format!("n_{}", id.replace('-', "_"));
    for node in nodes {
        let id = node["id"].as_str().expect("projection node ID");
        let alias = alias(id);
        let decision = node["type"] == "decision";
        let label = if anchor == Some(id) {
            format!("{id} · design anchor")
        } else if node["terminal"] == true {
            format!("{id} · terminal")
        } else {
            id.to_owned()
        };
        match format {
            Mermaid if decision => writeln!(output, "    {alias}{{\"{label}\"}}"),
            Mermaid => writeln!(output, "    {alias}[\"{label}\"]"),
            Dot => {
                let shape = if decision { "diamond" } else { "box" };
                writeln!(output, "    {alias} [label=\"{label}\", shape={shape}];")
            }
            PlantUml => writeln!(output, "state \"{label}\" as {alias}"),
        }
        .expect("writing to String cannot fail");
    }
    if has_replan && anchor.is_none() {
        output.push_str(match format {
            Mermaid => "    design_anchor[[\"Return to base design anchor\"]]\n",
            Dot => "    design_anchor [label=\"Return to base design anchor\", shape=box];\n",
            PlantUml => "state \"Return to base design anchor\" as design_anchor\n",
        });
    }
    for edge in edges {
        let from = alias(edge["from"].as_str().expect("projection edge source"));
        let effect = edge["effect"].as_str().expect("projection edge effect");
        let to = edge["to"]
            .as_str()
            .or(anchor)
            .map_or_else(|| "design_anchor".to_owned(), alias);
        let label = edge["option"].as_str().map_or_else(
            || effect.to_owned(),
            |option| format!("{option} · {effect}"),
        );
        match format {
            Mermaid => writeln!(output, "    {from} -->|{label}| {to}"),
            Dot => writeln!(output, "    {from} -> {to} [label=\"{label}\"];"),
            PlantUml => writeln!(output, "{from} --> {to} : {label}"),
        }
        .expect("writing to String cannot fail");
    }
    match format {
        Mermaid => {}
        Dot => output.push_str("}\n"),
        PlantUml => output.push_str("@enduml\n"),
    }
    bound(&output)?;
    let projection_digest = digest(output.as_bytes())?;
    Ok((output, projection_digest))
}

/// Build the exact canonical DAG of an admitted definition for a bounded paged consumer.
pub fn project_retained_successor_graph(
    value: &ValidatedSuccessorDefinition,
) -> Result<SuccessorGraphProjection, ConfigError> {
    use crate::procedure_v2_graph::GraphIndex;
    use serde_json::json;
    let definition = &value.definition;
    let graph = GraphIndex::successor(definition.graph());
    let dominators = graph.dominators();
    let returned = match definition {
        SuccessorDefinition::Base(v) => graph.index_of(v.boundary().success_return().as_str()),
        _ => None,
    };
    let mut edges = Vec::new();
    let nodes=definition.graph().nodes().iter().enumerate().map(|(index,node)| {
        let (kind,terminal)=match node.kind(){
            PlacementKindV3::Action{outcome,..}=>{
                if let ActionOutcomeV2::Next(to)=outcome {edges.push(json!({"from":node.id().as_str(),"to":to.as_str(),"effect":"advance"}));}
                ("action",outcome.is_terminal())
            },
            PlacementKindV3::Decision{routes}=>{
                let ordered=definition.definitions().iter().find(|v|v.id()==node.definition()).into_iter().flat_map(|v|match v {NodeDefinitionV3::Decision(v)=>v.metadata().options(),_=>&[]}).filter_map(|option|routes.iter().find(|(id,_)|id==option.id()));
                for (option,route) in ordered {
                    let mut edge=json!({"from":node.id().as_str(),"option":option.as_str()});
                    match route {
                        DecisionRouteV3::Advance(to)=>{edge["effect"]=json!("advance");edge["to"]=json!(to.as_str());},
                        DecisionRouteV3::Rework(to)=>{edge["effect"]=json!("rework");edge["to"]=json!(to.as_str());},
                        DecisionRouteV3::Replan=>edge["effect"]=json!("replan"),
                    }
                    edges.push(edge);
                }("decision",false)
            }
        };
        json!({"id":node.id().as_str(),"use":node.definition().as_str(),"type":kind,"entry":node.id()==definition.graph().entry(),"terminal":terminal,
            "manual_rework_target":definition.graph().manual_rework().contains(node.id()),
            "manual_rework_before_first_activation":definition.graph().manual_rework().contains(node.id())&&returned.is_some_and(|r|!graph.dominates(&dominators,r,index))})
    }).collect::<Vec<_>>();
    let mut document = json!({"schema":definition.schema(),"definition_digest":value.digest.as_str(),"entry":definition.graph().entry().as_str(),"nodes":nodes,"edges":edges});
    if let SuccessorDefinition::Base(base) = definition {
        document["execution_group"] = json!({"anchor":base.boundary().anchor().as_str(),"success_return":base.boundary().success_return().as_str()});
    }
    let projection = crate::canonical_json_from_serializable(&document)?;
    let digest = digest(projection.as_bytes())?;
    Ok(SuccessorGraphProjection { projection, digest })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn frame002_projection_limits_preserve_canonical_bytes_and_formatted_scalars() {
        let exact = "a".repeat(MAX_PROCEDURE_DOCUMENT_BYTES);
        assert!(bound(&exact).is_ok());
        assert!(bound(&(exact.clone() + "a")).is_err());
        let multibyte = "한".repeat(SOURCE_PROJECTION_MAX_CHARACTERS);
        assert!(bound_formatted(&multibyte).is_ok());
        assert!(bound_formatted(&(multibyte.clone() + "한")).is_err());
        assert!(bound(&multibyte).is_err());
    }
}
