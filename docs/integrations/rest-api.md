# REST API

Full schema: [`specs/openapi.yaml`](../../specs/openapi.yaml) (OpenAPI 3.1).

## Run

```bash
memoryfs serve --bind 127.0.0.1:7777 --data-dir ~/projects/foo/.memory
```

## Endpoint summary

| Method | Path | Purpose |
|--------|------|---------|
| `GET`    | `/v1/health` | Liveness check |
| `POST`   | `/v1/files` | Stage a write |
| `GET`    | `/v1/files/{path}` | Read by path |
| `GET`    | `/v1/files` | List with prefix |
| `POST`   | `/v1/commits` | Commit staged changes |
| `GET`    | `/v1/commits` | List commits |
| `GET`    | `/v1/commits/{hash}` | Read one |
| `POST`   | `/v1/commits/{hash}/revert` | Revert to a prior snapshot |
| `POST`   | `/v1/recall` | Semantic search via the retrieval engine |
| `POST`   | `/v1/context` | Multi-signal context bundle (vector + BM25 + RRF) |
| `POST`   | `/v1/memories/{id}/supersede` | Append-only replacement |
| `GET/POST` | `/v1/entities` | Entity graph CRUD |
| `POST`   | `/v1/entities/{id}/link` | Add a relation |
| `GET`    | `/v1/entities/{id}/neighbors` | BFS traversal |

## Auth

Every request requires `Authorization: Bearer <token>`. Tokens are JWT (HS256 by default), validated by `jsonwebtoken`. The token's subject (`sub`) and workspace (`ws`) claims feed `acl::check`.

For local dev, generate a token:

```bash
memoryfs admin issue-token --subject user:alice --workspace ws_dev --ttl 86400
```

## Errors

```json
{ "code": "acl_denied", "message": "user:alice has no allow rule for read on decisions/internal/**" }
```

The `code` is a stable string; `message` is human-readable. HTTP status maps to the error class:

| Class | Status | Code prefix |
|-------|--------|-------------|
| Validation | 400 | `validation_*` |
| Auth | 401 | `auth_*` |
| ACL | 403 | `acl_*` |
| Not found | 404 | `not_found` |
| Conflict | 409 | `conflict_*` |
| Server | 500 | `internal_*` |

## Notes

- All paths in `/v1/files/{path}` are URL-encoded — `decisions%2Fdb.md` for `decisions/db.md`.
- `/v1/recall` and `/v1/context` accept the same query params (`vector_weight`, `bm25_weight`, `top_k`, `scope_filter`).
- The server is single-tenant per `--data-dir`. For multi-workspace deployments, run multiple instances behind a router that picks the right data dir per workspace.
