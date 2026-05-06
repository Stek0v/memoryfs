//! Типизированные идентификаторы.
//!
//! Все ID в MemoryFS — ULID с типизированным префиксом. Это исключает класс ошибок
//! "перепутали run_id и memory_id". См. `02-data-model.md` §2 и
//! `specs/schemas/v1/base.schema.json#/$defs`.

use std::fmt;

/// Опаковый ULID-носитель. Crockford-Base32, 26 символов, без `I/L/O/U`.
///
/// Конкретный тип никогда не строится напрямую — только через типизированные
/// wrapper'ы (`MemoryId`, `RunId`, и т.п.), которые добавляют префикс и
/// валидируют его.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Ulid(String);

impl Default for Ulid {
    fn default() -> Self {
        Self::new()
    }
}

impl Ulid {
    /// Сгенерировать новый ULID. Использует `ulid` crate.
    pub fn new() -> Self {
        Self(ulid::Ulid::new().to_string())
    }

    /// Распарсить из строки. Валидирует длину и алфавит.
    pub fn parse(s: &str) -> crate::Result<Self> {
        if s.len() != 26 {
            return Err(crate::MemoryFsError::Validation(format!(
                "ULID must be 26 chars, got {}",
                s.len()
            )));
        }
        if s.chars().any(|c| !is_crockford_base32(c)) {
            return Err(crate::MemoryFsError::Validation(
                "ULID contains forbidden Crockford-Base32 characters (I, L, O, U disallowed)"
                    .into(),
            ));
        }
        Ok(Self(s.to_string()))
    }

    /// Внутреннее представление.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

fn is_crockford_base32(c: char) -> bool {
    matches!(
        c,
        '0'..='9'
            | 'A'
            | 'B'
            | 'C'
            | 'D'
            | 'E'
            | 'F'
            | 'G'
            | 'H'
            | 'J'
            | 'K'
            | 'M'
            | 'N'
            | 'P'
            | 'Q'
            | 'R'
            | 'S'
            | 'T'
            | 'V'
            | 'W'
            | 'X'
            | 'Y'
            | 'Z'
    )
}

macro_rules! prefixed_id {
    ($name:ident, $prefix:literal, $doc:literal) => {
        #[doc = $doc]
        #[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
        pub struct $name(String);

        impl $name {
            /// Сгенерировать новый ID с префиксом.
            pub fn new() -> Self {
                Self(format!("{}{}", $prefix, Ulid::new().as_str()))
            }

            /// Распарсить из строки. Проверяет префикс и валидность ULID-части.
            pub fn parse(s: &str) -> crate::Result<Self> {
                if let Some(rest) = s.strip_prefix($prefix) {
                    let _ = Ulid::parse(rest)?;
                    Ok(Self(s.to_string()))
                } else {
                    Err(crate::MemoryFsError::Validation(format!(
                        "expected prefix {:?}, got {:?}",
                        $prefix, s
                    )))
                }
            }

            /// Полный идентификатор (с префиксом).
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }
    };
}

prefixed_id!(MemoryId, "mem_", "Идентификатор памяти.");

impl MemoryId {
    /// Derive a deterministic MemoryId from a file path. The same path always
    /// produces the same ID across processes — required so the indexer can
    /// re-find and replace prior chunks for a file across cycles instead of
    /// piling new chunks under fresh random IDs.
    pub fn from_path(path: &str) -> Self {
        use sha2::{Digest, Sha256};
        let digest = Sha256::digest(path.as_bytes());
        let mut bytes = [0u8; 16];
        bytes.copy_from_slice(&digest[..16]);
        let ulid = ulid::Ulid::from_bytes(bytes);
        Self(format!("mem_{ulid}"))
    }
}
prefixed_id!(ConvId, "conv_", "Идентификатор conversation.");
prefixed_id!(RunId, "run_", "Идентификатор agent run.");
prefixed_id!(EntityId, "ent_", "Идентификатор entity (узла графа).");
prefixed_id!(DecisionId, "dec_", "Идентификатор ADR-decision.");
prefixed_id!(
    ProposalId,
    "prp_",
    "Идентификатор pending proposal в inbox."
);
prefixed_id!(WorkspaceId, "ws_", "Идентификатор workspace.");
prefixed_id!(EventId, "evt_", "Идентификатор audit event.");

/// Хэш коммита — sha256 в hex (lowercase, 64 chars).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CommitHash(String);

impl CommitHash {
    /// Распарсить из строки. Проверяет длину и hex.
    pub fn parse(s: &str) -> crate::Result<Self> {
        if s.len() != 64 || !s.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f')) {
            return Err(crate::MemoryFsError::Validation(format!(
                "commit hash must be 64 lowercase hex chars, got {s:?}"
            )));
        }
        Ok(Self(s.to_string()))
    }

    /// Hex-представление.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ulid_rejects_forbidden_chars() {
        for bad in ["L", "I", "O", "U"] {
            let s: String = bad.repeat(26);
            assert!(Ulid::parse(&s).is_err(), "should reject all-{bad}");
        }
    }

    #[test]
    fn ulid_rejects_wrong_length() {
        assert!(Ulid::parse("ABC").is_err());
        assert!(Ulid::parse(&"A".repeat(27)).is_err());
    }

    #[test]
    fn memory_id_requires_prefix() {
        assert!(MemoryId::parse("01HZK4M7N5P8Q9R3T6V8W2X5Y0").is_err());
        assert!(MemoryId::parse("mem_01HZK4M7N5P8Q9R3T6V8W2X5Y0").is_ok());
        assert!(MemoryId::parse("run_01HZK4M5J8K1M4N7P0Q3R6S9T2").is_err());
    }

    #[test]
    fn memory_id_from_path_is_deterministic() {
        let a = MemoryId::from_path("memories/foo.md");
        let b = MemoryId::from_path("memories/foo.md");
        let c = MemoryId::from_path("memories/bar.md");
        assert_eq!(a, b);
        assert_ne!(a, c);
        // Result must round-trip through the public parser.
        MemoryId::parse(a.as_str()).expect("from_path output must be parseable");
    }

    #[test]
    fn commit_hash_validates() {
        assert!(CommitHash::parse(&"a".repeat(64)).is_ok());
        assert!(CommitHash::parse(&"A".repeat(64)).is_err()); // uppercase
        assert!(CommitHash::parse(&"a".repeat(63)).is_err()); // wrong length
        assert!(CommitHash::parse(&"g".repeat(64)).is_err()); // non-hex
    }
}
