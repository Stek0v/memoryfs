//! Entity graph — in-memory store for entities and edges.
//!
//! Entities are typed nodes (person, project, tool, concept, site, org).
//! Edges connect any pair of nodes with a typed relation and weight.
//! Dedupe by canonical name + kind within a workspace.
//! See `02-data-model.md` §8, §11 and `specs/schemas/v1/entity.schema.json`.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::error::{MemoryFsError, Result};
use crate::ids::EntityId;

/// Entity kinds — controlled vocabulary from the schema.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EntityKind {
    /// A human.
    Person,
    /// A software or business project.
    Project,
    /// A tool, library, or framework.
    Tool,
    /// An abstract concept or topic.
    Concept,
    /// A website or service.
    Site,
    /// An organization or team.
    Org,
}

impl EntityKind {
    /// Parse from string.
    pub fn parse(s: &str) -> Result<Self> {
        match s {
            "person" => Ok(Self::Person),
            "project" => Ok(Self::Project),
            "tool" => Ok(Self::Tool),
            "concept" => Ok(Self::Concept),
            "site" => Ok(Self::Site),
            "org" => Ok(Self::Org),
            _ => Err(MemoryFsError::Validation(format!(
                "unknown entity kind: {s:?}"
            ))),
        }
    }
}

impl std::fmt::Display for EntityKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Person => f.write_str("person"),
            Self::Project => f.write_str("project"),
            Self::Tool => f.write_str("tool"),
            Self::Concept => f.write_str("concept"),
            Self::Site => f.write_str("site"),
            Self::Org => f.write_str("org"),
        }
    }
}

/// Edge relation types — controlled vocabulary from the OpenAPI spec.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Relation {
    /// Subject prefers target.
    Prefers,
    /// Subject avoids target.
    Avoids,
    /// Subject uses target.
    Uses,
    /// Subject knows target.
    Knows,
    /// Subject owns target.
    Owns,
    /// Subject is member of target.
    MemberOf,
    /// Subject wrote target.
    Wrote,
    /// Subject reviewed target.
    Reviewed,
    /// Subject is derived from target.
    DerivedFrom,
    /// Subject mentions target.
    Mentions,
    /// Subject references target.
    References,
    /// Subject supersedes target.
    Supersedes,
    /// Subject conflicts with target.
    ConflictsWith,
    /// Generic relation.
    RelatesTo,
    /// Subject produced target.
    Produced,
    /// Subject consumed target.
    Consumed,
}

impl Relation {
    /// Parse from string (case-insensitive).
    pub fn parse(s: &str) -> Result<Self> {
        match s.to_uppercase().as_str() {
            "PREFERS" => Ok(Self::Prefers),
            "AVOIDS" => Ok(Self::Avoids),
            "USES" => Ok(Self::Uses),
            "KNOWS" => Ok(Self::Knows),
            "OWNS" => Ok(Self::Owns),
            "MEMBER_OF" => Ok(Self::MemberOf),
            "WROTE" => Ok(Self::Wrote),
            "REVIEWED" => Ok(Self::Reviewed),
            "DERIVED_FROM" => Ok(Self::DerivedFrom),
            "MENTIONS" => Ok(Self::Mentions),
            "REFERENCES" => Ok(Self::References),
            "SUPERSEDES" => Ok(Self::Supersedes),
            "CONFLICTS_WITH" => Ok(Self::ConflictsWith),
            "RELATES_TO" => Ok(Self::RelatesTo),
            "PRODUCED" => Ok(Self::Produced),
            "CONSUMED" => Ok(Self::Consumed),
            _ => Err(MemoryFsError::Validation(format!(
                "unknown relation: {s:?}"
            ))),
        }
    }
}

/// An external reference (e.g. GitHub username, domain).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExternalRef {
    /// System name (e.g. "github", "email", "domain").
    pub system: String,
    /// Identifier within that system.
    pub id: String,
}

/// An entity node in the graph.
#[derive(Debug, Clone, Serialize)]
pub struct Entity {
    /// Unique entity ID.
    pub id: EntityId,
    /// Workspace this entity belongs to.
    pub workspace_id: String,
    /// Entity type.
    pub kind: EntityKind,
    /// Canonical display name.
    pub canonical_name: String,
    /// Alternative names for fuzzy matching and dedupe.
    pub aliases: Vec<String>,
    /// Free-form typed attributes.
    pub attributes: serde_json::Value,
    /// External system references.
    pub external_refs: Vec<ExternalRef>,
    /// File path in the workspace (e.g. `entities/project/ent_01J...md`).
    pub file_path: String,
    /// ID of the entity this was merged into, if any.
    pub merged_into: Option<EntityId>,
    /// When the entity was created.
    pub created_at: DateTime<Utc>,
    /// When the entity was last updated.
    pub updated_at: DateTime<Utc>,
}

/// A directed edge between two nodes.
#[derive(Debug, Clone, Serialize)]
pub struct Edge {
    /// Source node ID.
    pub src: String,
    /// Destination node ID.
    pub dst: String,
    /// Relation type.
    pub relation: Relation,
    /// Edge weight (0.0–1.0).
    pub weight: f32,
    /// Commit hash that created this edge.
    pub provenance_commit: Option<String>,
    /// When the edge was created.
    pub created_at: DateTime<Utc>,
}

/// In-memory entity graph with dedupe and neighbor traversal.
pub struct EntityGraph {
    entities: HashMap<String, Entity>,
    edges: Vec<Edge>,
    /// Index: lowercase canonical_name + kind → entity ID (for dedupe).
    name_index: HashMap<(String, EntityKind), String>,
    /// Index: lowercase alias → entity ID (for search/dedupe).
    alias_index: HashMap<String, Vec<String>>,
}

impl EntityGraph {
    /// Create an empty graph.
    pub fn new() -> Self {
        Self {
            entities: HashMap::new(),
            edges: Vec::new(),
            name_index: HashMap::new(),
            alias_index: HashMap::new(),
        }
    }

    /// Number of entities.
    pub fn entity_count(&self) -> usize {
        self.entities.len()
    }

    /// Number of edges.
    pub fn edge_count(&self) -> usize {
        self.edges.len()
    }

    /// All entities as a sorted vec (for backup/serialization).
    pub fn all_entities(&self) -> Vec<&Entity> {
        let mut ents: Vec<&Entity> = self.entities.values().collect();
        ents.sort_by_key(|a| a.id.to_string());
        ents
    }

    /// All edges (for backup/serialization).
    pub fn all_edges(&self) -> &[Edge] {
        &self.edges
    }

    /// Insert an entity. Returns error if an entity with the same canonical name
    /// and kind already exists in the workspace (dedupe).
    pub fn create_entity(
        &mut self,
        workspace_id: &str,
        kind: EntityKind,
        canonical_name: &str,
        aliases: Vec<String>,
        attributes: serde_json::Value,
        external_refs: Vec<ExternalRef>,
    ) -> Result<&Entity> {
        let name_key = (canonical_name.to_lowercase(), kind.clone());
        if let Some(existing_id) = self.name_index.get(&name_key) {
            return Err(MemoryFsError::Conflict(format!(
                "entity with name {:?} and kind {} already exists: {existing_id}",
                canonical_name, kind
            )));
        }

        let id = EntityId::new();
        let now = Utc::now();
        let file_path = format!("entities/{}/{}.md", kind, id);
        let id_str = id.to_string();

        let entity = Entity {
            id,
            workspace_id: workspace_id.to_string(),
            kind: kind.clone(),
            canonical_name: canonical_name.to_string(),
            aliases: aliases.clone(),
            attributes,
            external_refs,
            file_path,
            merged_into: None,
            created_at: now,
            updated_at: now,
        };

        self.name_index.insert(name_key, id_str.clone());

        for alias in &aliases {
            self.alias_index
                .entry(alias.to_lowercase())
                .or_default()
                .push(id_str.clone());
        }
        self.alias_index
            .entry(canonical_name.to_lowercase())
            .or_default()
            .push(id_str.clone());

        self.entities.insert(id_str.clone(), entity);
        Ok(self.entities.get(&id_str).unwrap())
    }

    /// Get an entity by ID.
    pub fn get(&self, id: &str) -> Result<&Entity> {
        self.entities
            .get(id)
            .ok_or_else(|| MemoryFsError::NotFound(format!("entity {id}")))
    }

    /// Search entities by query string (matches canonical name and aliases, case-insensitive).
    pub fn search(&self, query: &str, kinds: Option<&[EntityKind]>, limit: usize) -> Vec<&Entity> {
        let q = query.to_lowercase();
        let mut seen = std::collections::HashSet::new();
        let mut results = Vec::new();

        // Exact canonical name matches first.
        for ((_name, kind), id) in &self.name_index {
            if let Some(filter_kinds) = kinds {
                if !filter_kinds.contains(kind) {
                    continue;
                }
            }
            if let Some(entity) = self.entities.get(id) {
                if entity.canonical_name.to_lowercase().contains(&q) && seen.insert(id.clone()) {
                    results.push(entity);
                }
            }
        }

        // Then alias matches.
        for (alias, ids) in &self.alias_index {
            if alias.contains(&q) {
                for id in ids {
                    if seen.insert(id.clone()) {
                        if let Some(entity) = self.entities.get(id) {
                            if let Some(filter_kinds) = kinds {
                                if !filter_kinds.contains(&entity.kind) {
                                    continue;
                                }
                            }
                            results.push(entity);
                        }
                    }
                }
            }
        }

        results.truncate(limit);
        results
    }

    /// Merge `source_id` into `target_id`. Aliases from source are added to target.
    /// Source entity is marked with `merged_into`.
    pub fn merge(&mut self, source_id: &str, target_id: &str) -> Result<()> {
        if source_id == target_id {
            return Err(MemoryFsError::Validation(
                "cannot merge entity into itself".into(),
            ));
        }

        let source = self
            .entities
            .get(source_id)
            .ok_or_else(|| MemoryFsError::NotFound(format!("entity {source_id}")))?;

        if source.merged_into.is_some() {
            return Err(MemoryFsError::Conflict(format!(
                "entity {source_id} is already merged"
            )));
        }

        let _ = self
            .entities
            .get(target_id)
            .ok_or_else(|| MemoryFsError::NotFound(format!("entity {target_id}")))?;

        let source_aliases: Vec<String> = {
            let s = self.entities.get(source_id).unwrap();
            let mut a = s.aliases.clone();
            a.push(s.canonical_name.clone());
            a
        };
        let source_target_id = EntityId::parse(target_id)?;

        // Move aliases to target.
        {
            let target = self.entities.get_mut(target_id).unwrap();
            for alias in &source_aliases {
                if !target.aliases.contains(alias)
                    && alias.to_lowercase() != target.canonical_name.to_lowercase()
                {
                    target.aliases.push(alias.clone());
                    self.alias_index
                        .entry(alias.to_lowercase())
                        .or_default()
                        .push(target_id.to_string());
                }
            }
            target.updated_at = Utc::now();
        }

        // Mark source as merged.
        {
            let source = self.entities.get_mut(source_id).unwrap();
            source.merged_into = Some(source_target_id);
            source.updated_at = Utc::now();
        }

        // Repoint edges from source to target.
        for edge in &mut self.edges {
            if edge.src == source_id {
                edge.src = target_id.to_string();
            }
            if edge.dst == source_id {
                edge.dst = target_id.to_string();
            }
        }

        Ok(())
    }

    /// Create a directed edge between two entities.
    pub fn link(
        &mut self,
        src: &str,
        dst: &str,
        relation: Relation,
        weight: f32,
        provenance_commit: Option<String>,
    ) -> Result<&Edge> {
        if !self.entities.contains_key(src) {
            return Err(MemoryFsError::NotFound(format!("entity {src}")));
        }
        if !self.entities.contains_key(dst) {
            return Err(MemoryFsError::NotFound(format!("entity {dst}")));
        }

        let edge = Edge {
            src: src.to_string(),
            dst: dst.to_string(),
            relation,
            weight: weight.clamp(0.0, 1.0),
            provenance_commit,
            created_at: Utc::now(),
        };

        self.edges.push(edge);
        Ok(self.edges.last().unwrap())
    }

    /// Remove all edges matching src, dst, and relation.
    pub fn unlink(&mut self, src: &str, dst: &str, relation: &Relation) -> usize {
        let before = self.edges.len();
        self.edges
            .retain(|e| !(e.src == src && e.dst == dst && e.relation == *relation));
        before - self.edges.len()
    }

    /// Get neighbors of an entity up to `depth` hops, optionally filtered by relation.
    pub fn neighbors(
        &self,
        entity_id: &str,
        depth: usize,
        relations: Option<&[Relation]>,
    ) -> Result<(Vec<&Entity>, Vec<&Edge>)> {
        if !self.entities.contains_key(entity_id) {
            return Err(MemoryFsError::NotFound(format!("entity {entity_id}")));
        }

        let mut visited = std::collections::HashSet::new();
        visited.insert(entity_id.to_string());
        let mut frontier = vec![entity_id.to_string()];
        let mut result_edges = Vec::new();

        for _ in 0..depth {
            let mut next_frontier = Vec::new();
            for node_id in &frontier {
                for edge in &self.edges {
                    if let Some(rels) = relations {
                        if !rels.contains(&edge.relation) {
                            continue;
                        }
                    }

                    let neighbor = if edge.src == *node_id {
                        Some(&edge.dst)
                    } else if edge.dst == *node_id {
                        Some(&edge.src)
                    } else {
                        None
                    };

                    if let Some(nid) = neighbor {
                        result_edges.push(edge);
                        if visited.insert(nid.clone()) {
                            next_frontier.push(nid.clone());
                        }
                    }
                }
            }
            frontier = next_frontier;
            if frontier.is_empty() {
                break;
            }
        }

        // Exclude the root entity from results.
        visited.remove(entity_id);
        let nodes: Vec<&Entity> = visited
            .iter()
            .filter_map(|id| self.entities.get(id))
            .collect();

        Ok((nodes, result_edges))
    }

    /// Find an entity by canonical name and kind (exact dedupe lookup).
    pub fn find_by_name(&self, canonical_name: &str, kind: &EntityKind) -> Option<&Entity> {
        let key = (canonical_name.to_lowercase(), kind.clone());
        self.name_index
            .get(&key)
            .and_then(|id| self.entities.get(id))
    }
}

impl Default for EntityGraph {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_graph() -> EntityGraph {
        EntityGraph::new()
    }

    #[test]
    fn create_entity() {
        let mut g = make_graph();
        let e = g
            .create_entity(
                "ws_test",
                EntityKind::Person,
                "Alice",
                vec![],
                serde_json::json!({}),
                vec![],
            )
            .unwrap();
        assert_eq!(e.canonical_name, "Alice");
        assert_eq!(e.kind, EntityKind::Person);
        assert!(e.id.to_string().starts_with("ent_"));
        assert_eq!(g.entity_count(), 1);
    }

    #[test]
    fn dedupe_by_name_and_kind() {
        let mut g = make_graph();
        g.create_entity(
            "ws_test",
            EntityKind::Tool,
            "Qdrant",
            vec![],
            serde_json::json!({}),
            vec![],
        )
        .unwrap();
        let err = g
            .create_entity(
                "ws_test",
                EntityKind::Tool,
                "qdrant",
                vec![],
                serde_json::json!({}),
                vec![],
            )
            .unwrap_err();
        assert_eq!(err.api_code(), "CONFLICT");
    }

    #[test]
    fn same_name_different_kind_ok() {
        let mut g = make_graph();
        g.create_entity(
            "ws_test",
            EntityKind::Tool,
            "Rust",
            vec![],
            serde_json::json!({}),
            vec![],
        )
        .unwrap();
        g.create_entity(
            "ws_test",
            EntityKind::Concept,
            "Rust",
            vec![],
            serde_json::json!({}),
            vec![],
        )
        .unwrap();
        assert_eq!(g.entity_count(), 2);
    }

    #[test]
    fn get_entity() {
        let mut g = make_graph();
        let id = g
            .create_entity(
                "ws_test",
                EntityKind::Project,
                "MemoryFS",
                vec![],
                serde_json::json!({}),
                vec![],
            )
            .unwrap()
            .id
            .to_string();
        let e = g.get(&id).unwrap();
        assert_eq!(e.canonical_name, "MemoryFS");
    }

    #[test]
    fn get_missing_entity() {
        let g = make_graph();
        let err = g.get("ent_nonexistent").unwrap_err();
        assert_eq!(err.api_code(), "NOT_FOUND");
    }

    #[test]
    fn search_by_canonical_name() {
        let mut g = make_graph();
        g.create_entity(
            "ws_test",
            EntityKind::Person,
            "Alice Smith",
            vec![],
            serde_json::json!({}),
            vec![],
        )
        .unwrap();
        g.create_entity(
            "ws_test",
            EntityKind::Person,
            "Bob Jones",
            vec![],
            serde_json::json!({}),
            vec![],
        )
        .unwrap();
        let results = g.search("alice", None, 10);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].canonical_name, "Alice Smith");
    }

    #[test]
    fn search_by_alias() {
        let mut g = make_graph();
        g.create_entity(
            "ws_test",
            EntityKind::Tool,
            "Qdrant",
            vec!["qdrant-db".into(), "vector-store".into()],
            serde_json::json!({}),
            vec![],
        )
        .unwrap();
        let results = g.search("vector-store", None, 10);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].canonical_name, "Qdrant");
    }

    #[test]
    fn search_with_kind_filter() {
        let mut g = make_graph();
        g.create_entity(
            "ws_test",
            EntityKind::Tool,
            "Rust",
            vec![],
            serde_json::json!({}),
            vec![],
        )
        .unwrap();
        g.create_entity(
            "ws_test",
            EntityKind::Concept,
            "Rust",
            vec![],
            serde_json::json!({}),
            vec![],
        )
        .unwrap();
        let results = g.search("rust", Some(&[EntityKind::Tool]), 10);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].kind, EntityKind::Tool);
    }

    #[test]
    fn link_entities() {
        let mut g = make_graph();
        let a = g
            .create_entity(
                "ws_test",
                EntityKind::Person,
                "Alice",
                vec![],
                serde_json::json!({}),
                vec![],
            )
            .unwrap()
            .id
            .to_string();
        let b = g
            .create_entity(
                "ws_test",
                EntityKind::Tool,
                "Qdrant",
                vec![],
                serde_json::json!({}),
                vec![],
            )
            .unwrap()
            .id
            .to_string();

        let edge = g.link(&a, &b, Relation::Uses, 1.0, None).unwrap();
        assert_eq!(edge.relation, Relation::Uses);
        assert_eq!(g.edge_count(), 1);
    }

    #[test]
    fn link_missing_entity() {
        let mut g = make_graph();
        let a = g
            .create_entity(
                "ws_test",
                EntityKind::Person,
                "Alice",
                vec![],
                serde_json::json!({}),
                vec![],
            )
            .unwrap()
            .id
            .to_string();
        let err = g
            .link(&a, "ent_nonexistent", Relation::Knows, 1.0, None)
            .unwrap_err();
        assert_eq!(err.api_code(), "NOT_FOUND");
    }

    #[test]
    fn unlink_edges() {
        let mut g = make_graph();
        let a = g
            .create_entity(
                "ws_test",
                EntityKind::Person,
                "A",
                vec![],
                serde_json::json!({}),
                vec![],
            )
            .unwrap()
            .id
            .to_string();
        let b = g
            .create_entity(
                "ws_test",
                EntityKind::Person,
                "B",
                vec![],
                serde_json::json!({}),
                vec![],
            )
            .unwrap()
            .id
            .to_string();
        g.link(&a, &b, Relation::Knows, 1.0, None).unwrap();
        g.link(&a, &b, Relation::Mentions, 0.5, None).unwrap();

        let removed = g.unlink(&a, &b, &Relation::Knows);
        assert_eq!(removed, 1);
        assert_eq!(g.edge_count(), 1);
    }

    #[test]
    fn neighbors_depth_1() {
        let mut g = make_graph();
        let a = g
            .create_entity(
                "ws_test",
                EntityKind::Person,
                "A",
                vec![],
                serde_json::json!({}),
                vec![],
            )
            .unwrap()
            .id
            .to_string();
        let b = g
            .create_entity(
                "ws_test",
                EntityKind::Person,
                "B",
                vec![],
                serde_json::json!({}),
                vec![],
            )
            .unwrap()
            .id
            .to_string();
        let c = g
            .create_entity(
                "ws_test",
                EntityKind::Person,
                "C",
                vec![],
                serde_json::json!({}),
                vec![],
            )
            .unwrap()
            .id
            .to_string();
        g.link(&a, &b, Relation::Knows, 1.0, None).unwrap();
        g.link(&b, &c, Relation::Knows, 1.0, None).unwrap();

        let (nodes, edges) = g.neighbors(&a, 1, None).unwrap();
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].canonical_name, "B");
        assert_eq!(edges.len(), 1);
    }

    #[test]
    fn neighbors_depth_2() {
        let mut g = make_graph();
        let a = g
            .create_entity(
                "ws_test",
                EntityKind::Person,
                "A",
                vec![],
                serde_json::json!({}),
                vec![],
            )
            .unwrap()
            .id
            .to_string();
        let b = g
            .create_entity(
                "ws_test",
                EntityKind::Person,
                "B",
                vec![],
                serde_json::json!({}),
                vec![],
            )
            .unwrap()
            .id
            .to_string();
        let c = g
            .create_entity(
                "ws_test",
                EntityKind::Person,
                "C",
                vec![],
                serde_json::json!({}),
                vec![],
            )
            .unwrap()
            .id
            .to_string();
        g.link(&a, &b, Relation::Knows, 1.0, None).unwrap();
        g.link(&b, &c, Relation::Knows, 1.0, None).unwrap();

        let (nodes, _edges) = g.neighbors(&a, 2, None).unwrap();
        assert_eq!(nodes.len(), 2);
    }

    #[test]
    fn neighbors_with_relation_filter() {
        let mut g = make_graph();
        let a = g
            .create_entity(
                "ws_test",
                EntityKind::Person,
                "A",
                vec![],
                serde_json::json!({}),
                vec![],
            )
            .unwrap()
            .id
            .to_string();
        let b = g
            .create_entity(
                "ws_test",
                EntityKind::Tool,
                "B",
                vec![],
                serde_json::json!({}),
                vec![],
            )
            .unwrap()
            .id
            .to_string();
        let c = g
            .create_entity(
                "ws_test",
                EntityKind::Tool,
                "C",
                vec![],
                serde_json::json!({}),
                vec![],
            )
            .unwrap()
            .id
            .to_string();
        g.link(&a, &b, Relation::Uses, 1.0, None).unwrap();
        g.link(&a, &c, Relation::Knows, 1.0, None).unwrap();

        let (nodes, _) = g.neighbors(&a, 1, Some(&[Relation::Uses])).unwrap();
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].canonical_name, "B");
    }

    #[test]
    fn neighbors_missing_entity() {
        let g = make_graph();
        let err = g.neighbors("ent_nope", 1, None).unwrap_err();
        assert_eq!(err.api_code(), "NOT_FOUND");
    }

    #[test]
    fn merge_entities() {
        let mut g = make_graph();
        let a = g
            .create_entity(
                "ws_test",
                EntityKind::Person,
                "Alice",
                vec!["ali".into()],
                serde_json::json!({}),
                vec![],
            )
            .unwrap()
            .id
            .to_string();
        let b = g
            .create_entity(
                "ws_test",
                EntityKind::Person,
                "Alice Smith",
                vec!["alice-s".into()],
                serde_json::json!({}),
                vec![],
            )
            .unwrap()
            .id
            .to_string();
        g.link(&a, &b, Relation::Knows, 1.0, None).unwrap();

        g.merge(&a, &b).unwrap();

        let source = g.get(&a).unwrap();
        assert!(source.merged_into.is_some());

        let target = g.get(&b).unwrap();
        assert!(target.aliases.contains(&"Alice".to_string()));
        assert!(target.aliases.contains(&"ali".to_string()));

        // Edge should be repointed.
        assert_eq!(g.edges[0].src, b);
        assert_eq!(g.edges[0].dst, b);
    }

    #[test]
    fn merge_into_self_rejected() {
        let mut g = make_graph();
        let a = g
            .create_entity(
                "ws_test",
                EntityKind::Person,
                "X",
                vec![],
                serde_json::json!({}),
                vec![],
            )
            .unwrap()
            .id
            .to_string();
        let err = g.merge(&a, &a).unwrap_err();
        assert_eq!(err.api_code(), "VALIDATION");
    }

    #[test]
    fn merge_already_merged() {
        let mut g = make_graph();
        let a = g
            .create_entity(
                "ws_test",
                EntityKind::Person,
                "A",
                vec![],
                serde_json::json!({}),
                vec![],
            )
            .unwrap()
            .id
            .to_string();
        let b = g
            .create_entity(
                "ws_test",
                EntityKind::Person,
                "B",
                vec![],
                serde_json::json!({}),
                vec![],
            )
            .unwrap()
            .id
            .to_string();
        let c = g
            .create_entity(
                "ws_test",
                EntityKind::Person,
                "C",
                vec![],
                serde_json::json!({}),
                vec![],
            )
            .unwrap()
            .id
            .to_string();
        g.merge(&a, &b).unwrap();
        let err = g.merge(&a, &c).unwrap_err();
        assert_eq!(err.api_code(), "CONFLICT");
    }

    #[test]
    fn find_by_name() {
        let mut g = make_graph();
        g.create_entity(
            "ws_test",
            EntityKind::Tool,
            "Qdrant",
            vec![],
            serde_json::json!({}),
            vec![],
        )
        .unwrap();
        assert!(g.find_by_name("qdrant", &EntityKind::Tool).is_some());
        assert!(g.find_by_name("qdrant", &EntityKind::Person).is_none());
    }

    #[test]
    fn weight_clamped() {
        let mut g = make_graph();
        let a = g
            .create_entity(
                "ws_test",
                EntityKind::Person,
                "A",
                vec![],
                serde_json::json!({}),
                vec![],
            )
            .unwrap()
            .id
            .to_string();
        let b = g
            .create_entity(
                "ws_test",
                EntityKind::Person,
                "B",
                vec![],
                serde_json::json!({}),
                vec![],
            )
            .unwrap()
            .id
            .to_string();
        let edge = g.link(&a, &b, Relation::Knows, 5.0, None).unwrap();
        assert_eq!(edge.weight, 1.0);
    }

    #[test]
    fn entity_kind_parse_all() {
        assert_eq!(EntityKind::parse("person").unwrap(), EntityKind::Person);
        assert_eq!(EntityKind::parse("project").unwrap(), EntityKind::Project);
        assert_eq!(EntityKind::parse("tool").unwrap(), EntityKind::Tool);
        assert_eq!(EntityKind::parse("concept").unwrap(), EntityKind::Concept);
        assert_eq!(EntityKind::parse("site").unwrap(), EntityKind::Site);
        assert_eq!(EntityKind::parse("org").unwrap(), EntityKind::Org);
        assert!(EntityKind::parse("unknown").is_err());
    }

    #[test]
    fn relation_parse_all() {
        let names = [
            "PREFERS",
            "AVOIDS",
            "USES",
            "KNOWS",
            "OWNS",
            "MEMBER_OF",
            "WROTE",
            "REVIEWED",
            "DERIVED_FROM",
            "MENTIONS",
            "REFERENCES",
            "SUPERSEDES",
            "CONFLICTS_WITH",
            "RELATES_TO",
            "PRODUCED",
            "CONSUMED",
        ];
        for name in names {
            assert!(Relation::parse(name).is_ok(), "failed to parse {name}");
        }
        assert!(Relation::parse("INVALID").is_err());
    }

    #[test]
    fn relation_case_insensitive() {
        assert_eq!(Relation::parse("uses").unwrap(), Relation::Uses);
        assert_eq!(Relation::parse("Uses").unwrap(), Relation::Uses);
    }

    #[test]
    fn entity_serialization() {
        let mut g = make_graph();
        let e = g
            .create_entity(
                "ws_test",
                EntityKind::Tool,
                "Test",
                vec![],
                serde_json::json!({"version": "1.0"}),
                vec![],
            )
            .unwrap();
        let json = serde_json::to_value(e).unwrap();
        assert_eq!(json["canonical_name"], "Test");
        assert_eq!(json["kind"], "tool");
        assert_eq!(json["attributes"]["version"], "1.0");
    }

    #[test]
    fn edge_serialization() {
        let mut g = make_graph();
        let a = g
            .create_entity(
                "ws_test",
                EntityKind::Person,
                "A",
                vec![],
                serde_json::json!({}),
                vec![],
            )
            .unwrap()
            .id
            .to_string();
        let b = g
            .create_entity(
                "ws_test",
                EntityKind::Person,
                "B",
                vec![],
                serde_json::json!({}),
                vec![],
            )
            .unwrap()
            .id
            .to_string();
        let edge = g.link(&a, &b, Relation::Knows, 0.8, None).unwrap();
        let json = serde_json::to_value(edge).unwrap();
        assert_eq!(json["relation"], "KNOWS");
        assert!((json["weight"].as_f64().unwrap() - 0.8).abs() < 0.01);
    }

    #[test]
    fn external_refs() {
        let mut g = make_graph();
        let refs = vec![
            ExternalRef {
                system: "github".into(),
                id: "alice".into(),
            },
            ExternalRef {
                system: "email".into(),
                id: "alice@example.com".into(),
            },
        ];
        let e = g
            .create_entity(
                "ws_test",
                EntityKind::Person,
                "Alice",
                vec![],
                serde_json::json!({}),
                refs,
            )
            .unwrap();
        assert_eq!(e.external_refs.len(), 2);
        assert_eq!(e.external_refs[0].system, "github");
    }

    #[test]
    fn default_graph_empty() {
        let g = EntityGraph::default();
        assert_eq!(g.entity_count(), 0);
        assert_eq!(g.edge_count(), 0);
    }
}
