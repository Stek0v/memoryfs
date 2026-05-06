//! Entity extraction — NER + linking to existing entities.
//!
//! After memory extraction produces `ValidatedProposal`s, this module scans
//! entity references and links them to existing graph entities or creates new
//! ones. Dedupe relies on case-insensitive fuzzy matching via the entity
//! graph's `search()` and `find_by_name()` methods.
//!
//! See `04-tasks-dod.md` Phase 5 (task 5.2).

use crate::error::Result;
use crate::extraction::ValidatedProposal;
use crate::graph::{EntityGraph, EntityKind};
use crate::llm::{LlmClient, Message, Role};

/// Result of linking one entity reference to the graph.
#[derive(Debug, Clone)]
pub struct LinkedEntity {
    /// Entity ID in the graph (either existing or newly created).
    pub entity_id: String,
    /// Whether the entity already existed.
    pub was_existing: bool,
    /// Role from the original EntityRef.
    pub role: String,
    /// Confidence of the link (1.0 for exact match, lower for fuzzy).
    pub confidence: f32,
}

/// Result of entity extraction for a single proposal.
#[derive(Debug)]
pub struct ExtractionResult {
    /// Linked entities from the proposal's entity refs.
    pub linked: Vec<LinkedEntity>,
    /// Names mentioned in the body that were detected by NER.
    pub detected_names: Vec<DetectedName>,
}

/// A name detected by the NER pass (LLM-based or heuristic).
#[derive(Debug, Clone)]
pub struct DetectedName {
    /// The raw surface form from the text.
    pub surface_form: String,
    /// Inferred entity kind, if determinable.
    pub kind: Option<EntityKind>,
    /// Entity ID if linked to an existing entity.
    pub linked_entity_id: Option<String>,
}

/// Minimum fuzzy match score to consider a name "the same entity".
const LINK_THRESHOLD: f32 = 0.8;

/// Link entity references from a proposal to the entity graph.
/// For each entity ref, tries to find an existing entity by name; if not found,
/// creates a new one. Returns the list of linked entities.
pub fn link_entities(
    proposal: &ValidatedProposal,
    graph: &mut EntityGraph,
    workspace_id: &str,
) -> Result<Vec<LinkedEntity>> {
    let mut linked = Vec::new();

    for entity_ref in &proposal.entities {
        if entity_ref.id.is_empty() {
            continue;
        }

        let result = link_single_entity(&entity_ref.id, &entity_ref.role, graph, workspace_id)?;
        linked.push(result);
    }

    Ok(linked)
}

fn link_single_entity(
    name: &str,
    role: &str,
    graph: &mut EntityGraph,
    workspace_id: &str,
) -> Result<LinkedEntity> {
    let kind = infer_kind_from_role(role);

    if let Some(entity) = graph.find_by_name(name, &kind) {
        return Ok(LinkedEntity {
            entity_id: entity.id.to_string(),
            was_existing: true,
            role: role.to_string(),
            confidence: 1.0,
        });
    }

    let mut search_results = graph.search(name, Some(std::slice::from_ref(&kind)), 5);
    if search_results.is_empty() {
        if let Some(first_word) = name.split_whitespace().next() {
            search_results = graph.search(first_word, Some(std::slice::from_ref(&kind)), 5);
        }
    }
    for candidate in &search_results {
        let score = fuzzy_name_score(name, &candidate.canonical_name);
        if score >= LINK_THRESHOLD {
            return Ok(LinkedEntity {
                entity_id: candidate.id.to_string(),
                was_existing: true,
                role: role.to_string(),
                confidence: score,
            });
        }
    }

    let entity = graph.create_entity(
        workspace_id,
        kind,
        name,
        vec![],
        serde_json::json!({}),
        vec![],
    )?;
    Ok(LinkedEntity {
        entity_id: entity.id.to_string(),
        was_existing: false,
        role: role.to_string(),
        confidence: 1.0,
    })
}

/// Infer entity kind from the role in the entity ref.
fn infer_kind_from_role(role: &str) -> EntityKind {
    match role {
        "subject" => EntityKind::Person,
        "object" => EntityKind::Concept,
        "context" => EntityKind::Project,
        _ => EntityKind::Concept,
    }
}

/// Simple fuzzy name matching: normalized Levenshtein similarity.
fn fuzzy_name_score(a: &str, b: &str) -> f32 {
    let a = a.to_lowercase();
    let b = b.to_lowercase();

    if a == b {
        return 1.0;
    }

    let max_len = a.len().max(b.len());
    if max_len == 0 {
        return 1.0;
    }

    let dist = levenshtein(&a, &b);
    1.0 - (dist as f32 / max_len as f32)
}

/// Levenshtein edit distance.
fn levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let m = a.len();
    let n = b.len();

    let mut dp = vec![vec![0usize; n + 1]; m + 1];
    for (i, row) in dp.iter_mut().enumerate().take(m + 1) {
        row[0] = i;
    }
    for (j, cell) in dp[0].iter_mut().enumerate().take(n + 1) {
        *cell = j;
    }

    for i in 1..=m {
        for j in 1..=n {
            let cost = if a[i - 1] == b[j - 1] { 0 } else { 1 };
            dp[i][j] = (dp[i - 1][j] + 1)
                .min(dp[i][j - 1] + 1)
                .min(dp[i - 1][j - 1] + cost);
        }
    }

    dp[m][n]
}

/// NER prompt schema for LLM-based entity detection.
const NER_SCHEMA: &str = r#"{"type":"object","properties":{"entities":{"type":"array","items":{"type":"object","properties":{"name":{"type":"string"},"kind":{"type":"string","enum":["person","project","tool","concept","site","org"]}},"required":["name","kind"]}}},"required":["entities"]}"#;

/// Detect entity names from text using the LLM.
pub async fn detect_entities_llm(text: &str, llm: &dyn LlmClient) -> Result<Vec<DetectedName>> {
    let schema: serde_json::Value =
        serde_json::from_str(NER_SCHEMA).expect("NER schema is valid JSON");

    let messages = vec![
        Message {
            role: Role::System,
            content: "Extract named entities (people, projects, tools, concepts, sites, organizations) from the text. Return JSON with an 'entities' array.".to_string(),
        },
        Message {
            role: Role::User,
            content: text.to_string(),
        },
    ];

    let response = llm.chat(&messages, Some(&schema)).await?;
    parse_ner_response(&response)
}

/// Parse the NER response from the LLM.
fn parse_ner_response(response: &str) -> Result<Vec<DetectedName>> {
    let trimmed = response.trim();
    let json: serde_json::Value = serde_json::from_str(trimmed).map_err(|e| {
        crate::error::MemoryFsError::Internal(anyhow::anyhow!("NER response parse error: {e}"))
    })?;

    let entities = json["entities"].as_array().cloned().unwrap_or_default();

    let mut detected = Vec::new();
    for ent in entities {
        let name = ent["name"].as_str().unwrap_or_default().to_string();
        if name.is_empty() {
            continue;
        }
        let kind = ent["kind"].as_str().and_then(|k| EntityKind::parse(k).ok());
        detected.push(DetectedName {
            surface_form: name,
            kind,
            linked_entity_id: None,
        });
    }

    Ok(detected)
}

/// Link detected NER names to existing graph entities.
pub fn link_detected_names(names: &mut [DetectedName], graph: &EntityGraph) {
    for name in names.iter_mut() {
        let kind = name.kind.clone().unwrap_or(EntityKind::Concept);
        if let Some(entity) = graph.find_by_name(&name.surface_form, &kind) {
            name.linked_entity_id = Some(entity.id.to_string());
            continue;
        }

        let results = graph.search(&name.surface_form, Some(&[kind]), 1);
        if let Some(best) = results.first() {
            let score = fuzzy_name_score(&name.surface_form, &best.canonical_name);
            if score >= LINK_THRESHOLD {
                name.linked_entity_id = Some(best.id.to_string());
            }
        }
    }
}

/// Full entity extraction pipeline: link entity refs + NER detection + linking.
pub async fn extract_and_link(
    proposal: &ValidatedProposal,
    graph: &mut EntityGraph,
    workspace_id: &str,
    llm: &dyn LlmClient,
) -> Result<ExtractionResult> {
    let linked = link_entities(proposal, graph, workspace_id)?;

    let mut detected = detect_entities_llm(&proposal.body, llm).await?;
    link_detected_names(&mut detected, graph);

    Ok(ExtractionResult {
        linked,
        detected_names: detected,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extraction::EntityRef;

    #[test]
    fn fuzzy_exact_match() {
        assert_eq!(fuzzy_name_score("Alice", "alice"), 1.0);
    }

    #[test]
    fn fuzzy_close_match() {
        let score = fuzzy_name_score("Alice", "Alise");
        assert!(score > 0.7, "score={score}");
    }

    #[test]
    fn fuzzy_distant_match() {
        let score = fuzzy_name_score("Alice", "Bob");
        assert!(score < 0.5, "score={score}");
    }

    #[test]
    fn fuzzy_empty_strings() {
        assert_eq!(fuzzy_name_score("", ""), 1.0);
    }

    #[test]
    fn levenshtein_basic() {
        assert_eq!(levenshtein("kitten", "sitting"), 3);
        assert_eq!(levenshtein("", "abc"), 3);
        assert_eq!(levenshtein("abc", "abc"), 0);
    }

    #[test]
    fn infer_kind_defaults() {
        assert_eq!(infer_kind_from_role("subject"), EntityKind::Person);
        assert_eq!(infer_kind_from_role("object"), EntityKind::Concept);
        assert_eq!(infer_kind_from_role("context"), EntityKind::Project);
        assert_eq!(infer_kind_from_role("other"), EntityKind::Concept);
    }

    #[test]
    fn link_entities_creates_new() {
        let mut graph = EntityGraph::new();
        let proposal = make_proposal(vec![EntityRef {
            id: "Alice".into(),
            role: "subject".into(),
        }]);

        let linked = link_entities(&proposal, &mut graph, "ws_test").unwrap();
        assert_eq!(linked.len(), 1);
        assert!(!linked[0].was_existing);
        assert_eq!(linked[0].role, "subject");
        assert_eq!(graph.entity_count(), 1);
    }

    #[test]
    fn link_entities_finds_existing() {
        let mut graph = EntityGraph::new();
        graph
            .create_entity(
                "ws_test",
                EntityKind::Person,
                "Alice",
                vec![],
                serde_json::json!({}),
                vec![],
            )
            .unwrap();

        let proposal = make_proposal(vec![EntityRef {
            id: "Alice".into(),
            role: "subject".into(),
        }]);

        let linked = link_entities(&proposal, &mut graph, "ws_test").unwrap();
        assert_eq!(linked.len(), 1);
        assert!(linked[0].was_existing);
        assert_eq!(linked[0].confidence, 1.0);
        assert_eq!(graph.entity_count(), 1);
    }

    #[test]
    fn link_entities_fuzzy_match() {
        let mut graph = EntityGraph::new();
        graph
            .create_entity(
                "ws_test",
                EntityKind::Person,
                "Alice Johnson",
                vec![],
                serde_json::json!({}),
                vec![],
            )
            .unwrap();

        let proposal = make_proposal(vec![EntityRef {
            id: "Alice Jonhson".into(),
            role: "subject".into(),
        }]);

        let linked = link_entities(&proposal, &mut graph, "ws_test").unwrap();
        assert_eq!(linked.len(), 1);
        assert!(linked[0].was_existing);
        assert!(linked[0].confidence >= LINK_THRESHOLD);
        assert_eq!(graph.entity_count(), 1);
    }

    #[test]
    fn link_entities_skips_empty_id() {
        let mut graph = EntityGraph::new();
        let proposal = make_proposal(vec![EntityRef {
            id: "".into(),
            role: "subject".into(),
        }]);

        let linked = link_entities(&proposal, &mut graph, "ws_test").unwrap();
        assert!(linked.is_empty());
        assert_eq!(graph.entity_count(), 0);
    }

    #[test]
    fn link_detected_names_finds_existing() {
        let mut graph = EntityGraph::new();
        graph
            .create_entity(
                "ws_test",
                EntityKind::Tool,
                "Rust",
                vec![],
                serde_json::json!({}),
                vec![],
            )
            .unwrap();

        let mut names = vec![DetectedName {
            surface_form: "Rust".into(),
            kind: Some(EntityKind::Tool),
            linked_entity_id: None,
        }];

        link_detected_names(&mut names, &graph);
        assert!(names[0].linked_entity_id.is_some());
    }

    #[test]
    fn link_detected_names_no_match() {
        let graph = EntityGraph::new();
        let mut names = vec![DetectedName {
            surface_form: "UnknownTool".into(),
            kind: Some(EntityKind::Tool),
            linked_entity_id: None,
        }];

        link_detected_names(&mut names, &graph);
        assert!(names[0].linked_entity_id.is_none());
    }

    #[test]
    fn parse_ner_response_valid() {
        let json =
            r#"{"entities":[{"name":"Alice","kind":"person"},{"name":"Rust","kind":"tool"}]}"#;
        let result = parse_ner_response(json).unwrap();
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].surface_form, "Alice");
        assert_eq!(result[0].kind, Some(EntityKind::Person));
        assert_eq!(result[1].surface_form, "Rust");
        assert_eq!(result[1].kind, Some(EntityKind::Tool));
    }

    #[test]
    fn parse_ner_response_empty() {
        let json = r#"{"entities":[]}"#;
        let result = parse_ner_response(json).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn parse_ner_response_skips_empty_names() {
        let json = r#"{"entities":[{"name":"","kind":"person"},{"name":"Bob","kind":"person"}]}"#;
        let result = parse_ner_response(json).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].surface_form, "Bob");
    }

    #[test]
    fn parse_ner_response_unknown_kind() {
        let json = r#"{"entities":[{"name":"X","kind":"unknown_kind"}]}"#;
        let result = parse_ner_response(json).unwrap();
        assert_eq!(result.len(), 1);
        assert!(result[0].kind.is_none());
    }

    fn make_proposal(entities: Vec<EntityRef>) -> ValidatedProposal {
        use crate::extraction::{MemoryType, Scope, Sensitivity};

        ValidatedProposal {
            proposal_id: "prp_test".into(),
            memory_id: "mem_test".into(),
            memory_type: MemoryType::Fact,
            scope: Scope::User,
            scope_id: "user:alice".into(),
            sensitivity: Sensitivity::Normal,
            confidence: 0.9,
            title: "Test".into(),
            body: "Alice prefers Rust over Go.".into(),
            tags: vec![],
            entities,
            supersedes_hint: None,
            extracted_at: "2025-01-01T00:00:00Z".into(),
        }
    }
}
