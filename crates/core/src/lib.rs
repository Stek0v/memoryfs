//! MemoryFS Core
//!
//! Единый Rust crate — workspace engine, ACL, extraction (через DeepSeek API),
//! embedding (через локальный EmbeddingGemma), vector store (Qdrant → Levara).
//!
//! Согласован с `01-architecture.md`, `02-data-model.md`, `specs/schemas/v1/`
//! и ADR-003 (чистый Rust), ADR-004 (Qdrant vector store), ADR-013 (EmbeddingGemma),
//! ADR-014 (DeepSeek cloud).

#![warn(missing_docs)]
#![allow(dead_code)] // skeleton

pub mod acl;
pub mod api;
pub mod audit;
pub mod backup;
pub mod bm25;
mod chaos;
pub mod chunker;
pub mod commit;
pub mod embedder;
pub mod entity_extraction;
pub mod error;
pub mod event_log;
pub mod extraction;
pub mod extraction_worker;
pub mod graph;
pub mod ids;
pub mod inbox;
pub mod indexer;
pub mod levara;
pub mod llm;
pub mod mcp;
pub mod memory_policy;
pub mod migration;
pub mod observability;
pub mod policy;
pub mod post_scan;
pub mod redaction;
pub mod reindex;
pub mod retrieval;
pub mod runs;
pub mod schema;
pub mod storage;
pub mod supersede;
pub mod vector_store;

pub use error::{MemoryFsError, Result};
pub use ids::{CommitHash, ConvId, DecisionId, EntityId, MemoryId, ProposalId, RunId, WorkspaceId};
