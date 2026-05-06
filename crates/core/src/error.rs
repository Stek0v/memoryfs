//! Единая иерархия ошибок core. Соответствует кодам ошибок REST/MCP API
//! из `03-api-specification.md` и `specs/openapi.yaml`.

use thiserror::Error;

/// Ошибки workspace engine. Все варианты однозначно мапятся на коды
/// API: `VALIDATION`, `NOT_FOUND`, `CONFLICT`, `SUPERSEDE_CYCLE`, …
#[derive(Debug, Error)]
pub enum MemoryFsError {
    /// Schema- или данным-валидация не прошла.
    #[error("validation: {0}")]
    Validation(String),

    /// Subject не аутентифицирован.
    #[error("unauthorized")]
    Unauthorized,

    /// Субъект аутентифицирован, но не имеет прав.
    #[error("forbidden: {0}")]
    Forbidden(String),

    /// Объект не найден.
    #[error("not found: {0}")]
    NotFound(String),

    /// Конфликт состояния (общий случай).
    #[error("conflict: {0}")]
    Conflict(String),

    /// Цикл в графе supersedes.
    #[error("supersede cycle detected: {0}")]
    SupersedeCycle(String),

    /// Дубликат (idempotency key replayed с другим payload).
    #[error("duplicate: {0}")]
    Duplicate(String),

    /// `If-Match` не совпал с HEAD.
    #[error("precondition failed: parent commit moved")]
    PreconditionFailed,

    /// Operation отвергнута политикой.
    #[error("policy rejected: {0}")]
    PolicyRejected(String),

    /// Sensitive-памятка требует review, нельзя auto-commit.
    #[error("sensitive memory requires review: {0}")]
    SensitiveRequiresReview(String),

    /// Объект заблокирован (например, target_locked_pending_review).
    #[error("locked: {0}")]
    Locked(String),

    /// Превышен rate-limit.
    #[error("rate limited")]
    RateLimited,

    /// Внутренняя ошибка, не маппится напрямую на 4xx.
    #[error("internal: {0}")]
    Internal(#[from] anyhow::Error),

    /// Сервис временно недоступен.
    #[error("unavailable: {0}")]
    Unavailable(String),
}

/// Удобный alias.
pub type Result<T> = std::result::Result<T, MemoryFsError>;

impl MemoryFsError {
    /// Маппинг на код ошибки API. Должно соответствовать enum-у `Error.code`
    /// в `specs/openapi.yaml`.
    pub fn api_code(&self) -> &'static str {
        match self {
            Self::Validation(_) => "VALIDATION",
            Self::Unauthorized => "UNAUTHORIZED",
            Self::Forbidden(_) => "FORBIDDEN",
            Self::NotFound(_) => "NOT_FOUND",
            Self::Conflict(_) => "CONFLICT",
            Self::SupersedeCycle(_) => "SUPERSEDE_CYCLE",
            Self::Duplicate(_) => "DUPLICATE",
            Self::PreconditionFailed => "PRECONDITION_FAILED",
            Self::PolicyRejected(_) => "POLICY_REJECTED",
            Self::SensitiveRequiresReview(_) => "SENSITIVE_REQUIRES_REVIEW",
            Self::Locked(_) => "LOCKED",
            Self::RateLimited => "RATE_LIMITED",
            Self::Internal(_) => "INTERNAL",
            Self::Unavailable(_) => "UNAVAILABLE",
        }
    }

    /// HTTP-статус по умолчанию.
    pub fn http_status(&self) -> u16 {
        match self {
            Self::Validation(_) => 422,
            Self::Unauthorized => 401,
            Self::Forbidden(_) => 403,
            Self::NotFound(_) => 404,
            Self::Conflict(_) => 409,
            Self::SupersedeCycle(_) => 409,
            Self::Duplicate(_) => 409,
            Self::PreconditionFailed => 412,
            Self::PolicyRejected(_) => 422,
            Self::SensitiveRequiresReview(_) => 422,
            Self::Locked(_) => 423,
            Self::RateLimited => 429,
            Self::Internal(_) => 500,
            Self::Unavailable(_) => 503,
        }
    }
}
