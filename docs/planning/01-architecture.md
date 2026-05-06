# 01 — Архитектура MemoryFS

## 1. Архитектурные принципы

1. **Truth before retrieval.** Источник истины — Markdown-файлы. Индексы — производные.
2. **Append-only, never overwrite.** Изменение факта = новая запись + supersede.
3. **Provenance as data, not metadata.** Источник, run_id, commit, confidence — обязательные поля, валидируются схемой.
4. **Explicit commit boundaries.** Ни один агент не пишет в workspace без явного commit-события.
5. **Indexes are disposable.** Любой индекс должен полностью реконструироваться из workspace + event log.
6. **Permissions enforced at API, not at filesystem.** ОС-permissions — defense in depth, не основной механизм.
7. **Deterministic final read.** Retrieval даёт кандидатов; ответ строится на детерминированном чтении файлов.
8. **Plug-in by contract.** Embedder, LLM-extractor, graph-store, vector-store — за интерфейсами.

## 2. C4: Уровень 1 — Context

```mermaid
flowchart LR
    USER[Human user]
    AGENT[AI agents<br/>Claude Code / Cursor / Qwen Code / custom]
    OPS[Ops / SRE]

    SYS((MemoryFS))

    LLM[(External LLM API<br/>or local LLM endpoint)]
    EMB[(Embedding model<br/>local or cloud)]

    USER -->|CLI / MCP| SYS
    AGENT -->|MCP / REST| SYS
    OPS -->|Admin CLI / metrics| SYS

    SYS -->|extract / classify| LLM
    SYS -->|embed| EMB
```

## 3. C4: Уровень 2 — Containers

```mermaid
flowchart TD
    subgraph Client
        CLI[memoryfs CLI]
        MCPC[MCP client in agent]
    end

    subgraph Core[MemoryFS Core - Rust]
        API[HTTP/REST API<br/>Axum]
        MCP[MCP Server]
        WSE[Workspace Engine<br/>files + commits + ACL]
        EVL[Event Log<br/>append-only WAL]
        POL[Policy Engine<br/>permissions + redaction]
    end

    subgraph Workers[Workers - Python]
        EXTR[Extraction Worker<br/>LLM-driven]
        IDXR[Indexer Worker<br/>chunk + embed + BM25]
        GRAPH[Graph Worker<br/>entity linking]
    end

    subgraph Storage
        FS[(Workspace FS<br/>Markdown + objects)]
        VDB[(Vector DB<br/>Qdrant/pgvector)]
        FTS[(Full-text<br/>Tantivy)]
        GDB[(Edges/Graph<br/>SQLite/Postgres)]
        META[(Metadata DB<br/>SQLite/Postgres)]
    end

    CLI --> API
    MCPC --> MCP
    MCP --> API

    API --> WSE
    API --> EVL
    API --> POL

    WSE --> FS
    WSE --> META

    EVL --> EXTR
    EVL --> IDXR
    EVL --> GRAPH

    EXTR --> WSE
    IDXR --> VDB
    IDXR --> FTS
    GRAPH --> GDB

    API -.->|read for retrieve| VDB
    API -.->|read for retrieve| FTS
    API -.->|read for retrieve| GDB
    API -.->|deterministic read| FS
```

## 4. C4: Уровень 3 — Component (Workspace Engine)

```mermaid
flowchart TD
    API[API Layer]

    subgraph WSE[Workspace Engine]
        ROUTER[Path router]
        ACL[ACL guard]
        FRONT[Frontmatter parser/validator]
        OBJ[Object store<br/>content-addressable]
        IDX[Inode index<br/>path -> object hash]
        COMMIT[Commit graph<br/>parent / message / author]
        DIFF[Diff engine]
        REVERT[Revert engine]
    end

    API --> ROUTER
    ROUTER --> ACL
    ACL --> FRONT
    FRONT --> OBJ
    OBJ --> IDX
    IDX --> COMMIT
    COMMIT --> DIFF
    COMMIT --> REVERT
```

## 5. Поток: Запись памяти (write path)

```mermaid
sequenceDiagram
    autonumber
    participant A as Agent
    participant API as API/MCP
    participant POL as Policy
    participant EVL as Event Log
    participant EX as Extraction Worker
    participant WS as Workspace Engine
    participant IDX as Indexer

    A->>API: POST /events (conversation chunk)
    API->>POL: pre-redact (secrets/PII)
    POL-->>API: redacted payload
    API->>EVL: append event
    API-->>A: event_id

    EVL-->>EX: new event
    EX->>EX: LLM extract memories
    EX->>API: POST /memory/propose
    API->>POL: classify scope
    alt sensitive scope
        POL-->>API: requires review
        API->>WS: write to /inbox/proposals/
    else auto-commit allowed
        POL-->>API: ok
        API->>WS: write memory file + commit
        WS-->>API: commit_hash
        WS-->>EVL: append commit event
        EVL-->>IDX: reindex affected files
    end
```

## 6. Поток: Чтение / Recall (read path)

```mermaid
sequenceDiagram
    autonumber
    participant A as Agent
    participant API as API/MCP
    participant POL as Policy
    participant V as Vector index
    participant B as BM25 index
    participant G as Graph index
    participant WS as Workspace Engine

    A->>API: GET /context?query=...&scope=user:stek0v
    API->>POL: filter by ACL
    par parallel retrieval
        API->>V: top-k semantic
        API->>B: top-k keyword
        API->>G: entity-related nodes
    end
    V-->>API: candidates
    B-->>API: candidates
    G-->>API: candidates
    API->>API: rank fusion (RRF + scope boost)
    API->>WS: deterministic read of top-N files
    WS-->>API: file contents + frontmatter
    API->>API: build context with provenance
    API-->>A: {answer_context, citations, scores}
```

## 7. Поток: Commit / Revert

```mermaid
sequenceDiagram
    autonumber
    participant A as Agent
    participant API as API
    participant WS as Workspace Engine
    participant IDX as Indexer

    A->>API: POST /workspace/commit (message, files)
    API->>WS: validate frontmatter for each file
    WS->>WS: compute object hashes
    WS->>WS: build commit object {parent, tree, author, msg}
    WS->>WS: persist commit + update HEAD
    WS-->>API: commit_hash
    API->>IDX: signal reindex (event)
    API-->>A: commit_hash

    Note over A,WS: Later — revert
    A->>API: POST /workspace/revert {commit_hash}
    API->>WS: build inverse changes
    WS->>WS: new commit reverting target
    WS-->>API: new commit_hash
    API->>IDX: reindex affected
```

## 8. Поток: Reindex (rebuild from truth)

```mermaid
sequenceDiagram
    autonumber
    participant OPS as Ops
    participant API as Admin API
    participant WS as Workspace
    participant IDX as Indexer
    participant V as Vector
    participant B as BM25
    participant G as Graph

    OPS->>API: POST /admin/reindex {scope}
    API->>IDX: enumerate files in scope
    loop per file
        IDX->>WS: read file + frontmatter
        IDX->>IDX: chunk by heading-aware
        IDX->>V: upsert embeddings
        IDX->>B: upsert tokens
        IDX->>G: upsert entities/edges
    end
    IDX-->>API: report counts + drift
    API-->>OPS: done
```

## 9. Структура директорий workspace

```text
/workspaces/<workspace_id>/
├── .memoryfs/
│   ├── config.yaml              # workspace config
│   ├── HEAD                     # current commit hash
│   ├── refs/                    # named refs (main, branches)
│   ├── objects/                 # content-addressable blobs
│   ├── commits/                 # commit objects
│   ├── policy.yaml              # memory policies
│   ├── schema/                  # JSON schemas per type
│   └── audit.log                # append-only audit
│
├── memory/
│   ├── users/<user_id>/
│   │   ├── profile.md
│   │   ├── preferences.md
│   │   └── history/<ULID>.md
│   ├── agents/<agent_id>/
│   │   ├── identity.md
│   │   └── learnings/<ULID>.md
│   ├── sessions/<session_id>/
│   ├── projects/<project_id>/
│   └── org/
│
├── conversations/
│   └── <YYYY>/<MM>/<DD>/<conv_id>.md
│
├── runs/
│   └── <run_id>/
│       ├── prompt.md
│       ├── tool_calls.md
│       ├── stdout.md
│       ├── stderr.md
│       ├── result.md
│       ├── metadata.md
│       ├── memory_patch.md
│       └── artifacts/
│
├── decisions/
│   └── adr-<NNNN>-<slug>.md
│
├── entities/
│   ├── people/<entity_id>.md
│   ├── projects/<entity_id>.md
│   ├── tools/<entity_id>.md
│   └── concepts/<entity_id>.md
│
├── inbox/
│   ├── proposals/               # awaiting review
│   └── conflicts/               # superseding to confirm
│
└── archive/                     # superseded / deprecated
```

## 10. Слои хранения

| Слой | Технология | Назначение | Перестраиваемый? |
| ------ | ----------- | ------------ | ------------------ |
| Markdown FS | filesystem + объекты по хешу | Источник истины, content-addressable | — (canonical) |
| Metadata DB | SQLite (single-node) / Postgres (server) | Inode index, ACL cache, commit graph | Да, из FS |
| Vector index | Qdrant (рекоменд.) или pgvector | Семантический recall | Да |
| Full-text index | Tantivy (embedded) или Meilisearch | BM25 / keyword | Да |
| Graph store | Postgres-таблица `edges` (MVP) → Kuzu | Entity-linking, multi-hop | Да |
| Event log | append-only file + offset-index | Очередь событий для воркеров | — (canonical для очереди) |
| Audit log | append-only file | Compliance / debugging | — (canonical для аудита) |

**Правило**: если данные есть только в индексе и нет в `workspace + event log + audit log` —
это баг, индекс **не** источник истины ни для чего.

## 11. Безопасность — слои защиты

```mermaid
flowchart LR
    INP[Input from agent/user] --> SAN[1. Sanitizer<br/>strip system instructions]
    SAN --> RED[2. Redactor<br/>regex + entropy + denylist]
    RED --> POL[3. Policy engine<br/>scope rules]
    POL --> EXT[4. LLM extractor<br/>contract-bound output]
    EXT --> SCAN[5. Post-scan<br/>secrets after extraction]
    SCAN --> REV[6. Review queue<br/>for sensitive scopes]
    REV --> ACL[7. ACL guard<br/>at write]
    ACL --> WS[Workspace]
    WS --> AUDIT[8. Audit log]
```

Каждый слой имеет независимые тесты (см. `06-testing-strategy.md`).

## 12. Permissions модель

- **Subjects**: `user:<id>`, `agent:<id>`, `group:<id>`, `role:<name>`, `owner`, `anonymous`.
- **Resources**: путь в workspace (с поддержкой glob: `/memory/users/stek0v/**`).
- **Actions**: `read`, `write`, `commit`, `propose`, `review`, `revert`, `delete`, `admin`.
- **Decision rule**: `deny` побеждает `allow`; default — `deny`.
- **ACL хранится** в `.memoryfs/policy.yaml` + опционально per-file overrides в frontmatter.

Пример:

```yaml
# .memoryfs/policy.yaml
default_deny: true
rules:
  - subject: "user:stek0v"
    resource: "/memory/users/stek0v/**"
    actions: ["read", "write", "commit", "revert"]
  - subject: "agent:valeria"
    resource: "/memory/users/stek0v/**"
    actions: ["read", "propose"]
  - subject: "group:reviewers"
    resource: "/inbox/proposals/**"
    actions: ["read", "review", "commit"]
```

## 13. Deployment — топологии

### 13.1 Local single-user (MVP target)

```mermaid
flowchart LR
    USER[User CLI] --> CORE[memoryfs core<br/>localhost]
    AGENT[Agent via MCP] --> CORE
    CORE --> FS[(local FS)]
    CORE --> SQLITE[(SQLite metadata)]
    CORE --> QDRANT[(Qdrant local container)]
    CORE --> WORKER[Python worker<br/>local process]
    WORKER --> LLMLOCAL[(local LLM endpoint)]
```

### 13.2 Team server (Phase 5+)

```mermaid
flowchart TD
    AGENTS[Agents / users] --> LB[Reverse proxy / TLS]
    LB --> CORE[memoryfs core<br/>multi-instance]
    CORE --> PG[(Postgres metadata)]
    CORE --> NFS[(Shared FS / S3-compat objects)]
    CORE --> QDRANT[(Qdrant cluster)]
    CORE --> KAFKA[(Event log: NATS / Kafka)]
    KAFKA --> WORKERS[Worker pool]
```

## 14. Контракты между компонентами

### 14.1 API ↔ Workspace Engine

```rust
trait WorkspaceEngine {
    fn read(&self, path: &Path, ctx: &AuthCtx) -> Result<File>;
    fn write(&self, path: &Path, content: &[u8], fm: Frontmatter, ctx: &AuthCtx) -> Result<ObjectHash>;
    fn commit(&self, msg: &str, paths: &[Path], ctx: &AuthCtx) -> Result<CommitHash>;
    fn revert(&self, commit: CommitHash, ctx: &AuthCtx) -> Result<CommitHash>;
    fn diff(&self, from: CommitHash, to: CommitHash) -> Result<Diff>;
    fn log(&self, path: Option<&Path>, limit: usize) -> Result<Vec<CommitMeta>>;
    fn list(&self, path: &Path, ctx: &AuthCtx) -> Result<Vec<DirEntry>>;
}
```

### 14.2 Indexer ↔ stores

```python
class Indexer:
    def index_file(self, file: WorkspaceFile, commit: str) -> IndexResult: ...
    def remove_file(self, path: str) -> None: ...
    def reindex_scope(self, scope_glob: str) -> ReindexReport: ...
```

### 14.3 Extraction worker

```python
class ExtractionWorker:
    def extract(self, event: ConversationEvent, ctx: ExtractionContext) -> List[MemoryProposal]: ...
```

`MemoryProposal` всегда включает `confidence`, `source_span`, `entities`, `scope`, `type`.
Воркер **никогда** не пишет в workspace напрямую — только через `POST /memory/propose`.

## 15. Observability

Обязательно с Phase 1:

- **Metrics** (Prometheus-формат): RPS, p50/p95/p99 на read/write/commit/recall; queue depth;
  index drift; redaction hits; review-queue size.
- **Tracing** (OpenTelemetry): trace per request; spans на каждый слой (API → policy → engine → store).
- **Structured logs** (JSON): обязательные поля `trace_id`, `workspace_id`, `subject`, `action`, `resource`, `result`.
- **Audit events** (отдельный лог): все write/commit/revert/review events, immutable.

## 16. Что считаем "production-ready"

Не входит в MVP, но фиксируем DoD для зрелости:

- p95 read < 50ms на workspace до 100k файлов.
- p95 recall < 300ms (вкл. fusion).
- Reindex 100k файлов < 30 минут.
- Zero data loss при kill -9 на любом этапе (WAL recovery).
- Все sensitive-write события → audit log без потерь.
- Coverage > 80% на core (Rust), > 70% на workers (Python).
- Adversarial-suite: 100% redaction для известных шаблонов секретов.
