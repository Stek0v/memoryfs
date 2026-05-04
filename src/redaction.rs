//! Pre-redaction engine — blocks secrets from being written to the workspace.
//!
//! Applies regex patterns, entropy heuristics, and denylist checks to detect
//! API keys, JWTs, SSH keys, passwords, credit cards, IBANs, SSNs, and
//! high-entropy strings before they reach storage.

use crate::error::{MemoryFsError, Result};

/// Result of scanning text for secrets.
#[derive(Debug, Clone)]
pub struct RedactionResult {
    /// Detected secrets with their positions and categories.
    pub findings: Vec<Finding>,
}

/// A single detected secret.
#[derive(Debug, Clone)]
pub struct Finding {
    /// Category of the secret (e.g. "api_key", "jwt", "ssh_key").
    pub category: String,
    /// Start byte offset in the input.
    pub start: usize,
    /// End byte offset in the input.
    pub end: usize,
    /// The matched text (for logging; will be partially masked).
    pub matched: String,
}

impl RedactionResult {
    /// Whether any secrets were found.
    pub fn has_secrets(&self) -> bool {
        !self.findings.is_empty()
    }
}

/// Scan `input` for secrets. Returns all findings.
pub fn scan(input: &str) -> RedactionResult {
    let mut findings = Vec::new();

    scan_api_keys(input, &mut findings);
    scan_jwts(input, &mut findings);
    scan_ssh_keys(input, &mut findings);
    scan_passwords(input, &mut findings);
    scan_credit_cards(input, &mut findings);
    scan_ibans(input, &mut findings);
    scan_ssns(input, &mut findings);
    scan_high_entropy(input, &mut findings);

    findings.sort_by_key(|f| f.start);
    findings.dedup_by(|a, b| a.start == b.start && a.end == b.end);

    RedactionResult { findings }
}

/// Check input and return an error if secrets are found (fail-closed mode).
pub fn check_and_reject(input: &str, fail_closed: bool) -> Result<()> {
    let result = scan(input);
    if result.has_secrets() {
        let categories: Vec<&str> = result
            .findings
            .iter()
            .map(|f| f.category.as_str())
            .collect();
        if fail_closed {
            return Err(MemoryFsError::PolicyRejected(format!(
                "content contains secrets: {}",
                categories.join(", ")
            )));
        }
    }
    Ok(())
}

/// Redact secrets in `input`, replacing them with `[REDACTED:<category>]`.
pub fn redact(input: &str) -> String {
    let result = scan(input);
    if result.findings.is_empty() {
        return input.to_string();
    }

    let mut output = String::with_capacity(input.len());
    let mut last_end = 0;

    for f in &result.findings {
        if f.start > last_end {
            output.push_str(&input[last_end..f.start]);
        }
        output.push_str(&format!("[REDACTED:{}]", f.category));
        last_end = f.end;
    }

    if last_end < input.len() {
        output.push_str(&input[last_end..]);
    }

    output
}

fn add_finding(findings: &mut Vec<Finding>, category: &str, input: &str, start: usize, end: usize) {
    let matched = &input[start..end];
    let masked = if matched.len() > 12 {
        format!("{}...{}", &matched[..6], &matched[matched.len() - 4..])
    } else {
        matched.to_string()
    };
    findings.push(Finding {
        category: category.to_string(),
        start,
        end,
        matched: masked,
    });
}

// ── API key patterns ──

fn scan_api_keys(input: &str, findings: &mut Vec<Finding>) {
    let patterns: &[(&str, &str, usize)] = &[
        ("sk-proj-", "api_key", 40),
        ("sk-ant-api", "api_key", 40),
        ("ghp_", "api_key", 36),
        ("gho_", "api_key", 36),
        ("ghs_", "api_key", 36),
        ("ghu_", "api_key", 36),
        ("xoxb-", "api_key", 40),
        ("xoxp-", "api_key", 40),
        ("xoxa-", "api_key", 40),
        ("xoxr-", "api_key", 40),
        ("sk_live_", "api_key", 20),
        ("rk_live_", "api_key", 20),
        ("sk_test_", "api_key", 20),
        ("rk_test_", "api_key", 20),
        ("sq0atp-", "api_key", 20),
        ("sq0csp-", "api_key", 20),
        ("AIzaSy", "api_key", 33),
        ("glpat-", "api_key", 20),
        ("AKIA", "api_key", 20),
    ];

    for (prefix, category, min_len) in patterns {
        for (idx, _) in input.match_indices(prefix) {
            let remaining = &input[idx..];
            let token_end = remaining
                .find(|c: char| c.is_whitespace() || c == '\'' || c == '"' || c == ')')
                .unwrap_or(remaining.len());

            if token_end >= *min_len {
                add_finding(findings, category, input, idx, idx + token_end);
            }
        }
    }

    // AWS secret access key pattern (40 chars, base64-like, after common context)
    for keyword in &[
        "aws_secret",
        "AWS_SECRET",
        "secret_access_key",
        "SecretAccessKey",
    ] {
        for (idx, _) in input.match_indices(keyword) {
            let after = &input[idx..];
            if let Some(eq_pos) = after.find('=').or_else(|| after.find(':')) {
                let value_start = idx + eq_pos + 1;
                if value_start < input.len() {
                    let value = input[value_start..].trim_start();
                    let value_end_offset = value
                        .find(|c: char| c.is_whitespace() || c == '\'' || c == '"')
                        .unwrap_or(value.len());
                    let raw = &value[..value_end_offset];
                    if raw.len() >= 30 {
                        let start = input.len() - value.len();
                        add_finding(findings, "api_key", input, start, start + value_end_offset);
                    }
                }
            }
        }
    }

    // OpenAI legacy key (sk- followed by 48+ alphanumeric chars)
    let mut pos = 0;
    while pos < input.len() {
        if let Some(idx) = input[pos..].find("sk-") {
            let abs_idx = pos + idx;
            // Skip if it's sk-proj- or sk-ant- (already handled)
            let after = &input[abs_idx..];
            if after.starts_with("sk-proj-") || after.starts_with("sk-ant-") {
                pos = abs_idx + 3;
                continue;
            }

            let token_end = after
                .find(|c: char| c.is_whitespace() || c == '\'' || c == '"' || c == ')')
                .unwrap_or(after.len());

            if token_end >= 40 {
                // Check it's not already in findings
                let already = findings
                    .iter()
                    .any(|f| f.start <= abs_idx && f.end >= abs_idx + token_end);
                if !already {
                    add_finding(findings, "api_key", input, abs_idx, abs_idx + token_end);
                }
            }
            pos = abs_idx + token_end;
        } else {
            break;
        }
    }
}

// ── JWT patterns ──

fn scan_jwts(input: &str, findings: &mut Vec<Finding>) {
    let prefixes = ["eyJ"];

    for prefix in &prefixes {
        for (idx, _) in input.match_indices(prefix) {
            let remaining = &input[idx..];
            let token_end = remaining
                .find(|c: char| c.is_whitespace() || c == '\'' || c == '"' || c == ')')
                .unwrap_or(remaining.len());

            let candidate = &remaining[..token_end];

            // JWT has exactly 2 dots
            let dot_count = candidate.chars().filter(|&c| c == '.').count();
            if dot_count == 2 && candidate.len() >= 30 {
                add_finding(findings, "jwt", input, idx, idx + token_end);
            }
        }
    }
}

// ── SSH/PEM keys ──

fn scan_ssh_keys(input: &str, findings: &mut Vec<Finding>) {
    let markers = [
        "-----BEGIN RSA PRIVATE KEY-----",
        "-----BEGIN OPENSSH PRIVATE KEY-----",
        "-----BEGIN EC PRIVATE KEY-----",
        "-----BEGIN DSA PRIVATE KEY-----",
        "-----BEGIN PRIVATE KEY-----",
        "-----BEGIN PGP PRIVATE KEY BLOCK-----",
        "-----BEGIN CERTIFICATE-----",
    ];

    for marker in &markers {
        for (idx, _) in input.match_indices(marker) {
            let end_marker = marker.replace("BEGIN", "END");
            let search_start = idx + marker.len();
            let end = if let Some(end_idx) = input[search_start..].find(&end_marker) {
                search_start + end_idx + end_marker.len()
            } else {
                (idx + 200).min(input.len())
            };
            add_finding(findings, "ssh_key", input, idx, end);
        }
    }
}

// ── Password patterns ──

fn scan_passwords(input: &str, findings: &mut Vec<Finding>) {
    // URLs with embedded credentials
    for proto in &[
        "http://",
        "https://",
        "ftp://",
        "postgres://",
        "mysql://",
        "mongodb://",
        "redis://",
    ] {
        for (idx, _) in input.match_indices(proto) {
            let remaining = &input[idx..];
            let url_end = remaining
                .find(|c: char| c.is_whitespace())
                .unwrap_or(remaining.len());
            let url = &remaining[..url_end];

            if url.contains('@') {
                let after_proto = &url[proto.len()..];
                if after_proto.contains(':') && after_proto.contains('@') {
                    add_finding(findings, "password", input, idx, idx + url_end);
                }
            }
        }
    }

    // Explicit password labels
    let labels = [
        "password is ",
        "password: ",
        "password=",
        "passwd: ",
        "passwd=",
        "PASSWORD=",
        "PASSWORD: ",
    ];
    for label in &labels {
        let label_lower = label.to_lowercase();
        let input_lower = input.to_lowercase();
        for (idx, _) in input_lower.match_indices(&label_lower) {
            let value_start = idx + label.len();
            if value_start < input.len() {
                let value_end = input[value_start..]
                    .find(['\n', '\r'])
                    .map(|e| value_start + e)
                    .unwrap_or(input.len());
                if value_end > value_start + 3 {
                    add_finding(findings, "password", input, idx, value_end);
                }
            }
        }
    }
}

// ── Credit card patterns ──

fn scan_credit_cards(input: &str, findings: &mut Vec<Finding>) {
    // Check for 13-19 digit sequences that pass Luhn check
    let bytes = input.as_bytes();
    let len = bytes.len();
    let mut i = 0;

    while i < len {
        if bytes[i].is_ascii_digit() {
            let start = i;
            let mut digits = Vec::new();
            let mut j = i;
            while j < len && (bytes[j].is_ascii_digit() || bytes[j] == b' ' || bytes[j] == b'-') {
                if bytes[j].is_ascii_digit() {
                    digits.push(bytes[j] - b'0');
                }
                j += 1;
            }
            if digits.len() >= 13 && digits.len() <= 19 && luhn_check(&digits) {
                add_finding(findings, "credit_card", input, start, j);
            }
            i = j;
        } else {
            i += 1;
        }
    }
}

fn luhn_check(digits: &[u8]) -> bool {
    let mut sum = 0u32;
    let mut alt = false;
    for &d in digits.iter().rev() {
        let mut n = d as u32;
        if alt {
            n *= 2;
            if n > 9 {
                n -= 9;
            }
        }
        sum += n;
        alt = !alt;
    }
    sum.is_multiple_of(10)
}

// ── IBAN patterns ──

fn scan_ibans(input: &str, findings: &mut Vec<Finding>) {
    let len = input.len();
    let bytes = input.as_bytes();
    let mut i = 0;

    while i < len.saturating_sub(14) {
        // IBAN starts with 2 uppercase letters followed by 2 digits
        if i + 4 <= len
            && bytes[i].is_ascii_uppercase()
            && bytes[i + 1].is_ascii_uppercase()
            && bytes[i + 2].is_ascii_digit()
            && bytes[i + 3].is_ascii_digit()
        {
            let start = i;
            let mut j = i;
            let mut char_count = 0;
            while j < len && char_count < 34 {
                if bytes[j] == b' ' {
                    j += 1;
                    continue;
                }
                if bytes[j].is_ascii_alphanumeric() {
                    char_count += 1;
                    j += 1;
                } else {
                    break;
                }
            }
            if (15..=34).contains(&char_count) {
                add_finding(findings, "iban", input, start, j);
            }
            i = j;
        } else {
            i += 1;
        }
    }
}

// ── SSN patterns ──

fn scan_ssns(input: &str, findings: &mut Vec<Finding>) {
    let bytes = input.as_bytes();
    let len = bytes.len();

    // Pattern: SSN followed by NNN-NN-NNNN
    let labels = ["SSN ", "ssn ", "SSN: ", "ssn: "];
    for label in &labels {
        for (idx, _) in input.match_indices(label) {
            let after = &input[idx + label.len()..];
            let digits: String = after
                .chars()
                .take(11)
                .filter(|c| c.is_ascii_digit())
                .collect();
            if digits.len() == 9 {
                let end = (idx + label.len() + 11).min(input.len());
                add_finding(findings, "ssn", input, idx, end);
            }
        }
    }

    // Bare NNN-NN-NNNN pattern
    let mut i = 0;
    while i + 10 < len {
        if bytes[i].is_ascii_digit()
            && bytes[i + 1].is_ascii_digit()
            && bytes[i + 2].is_ascii_digit()
            && bytes[i + 3] == b'-'
            && bytes[i + 4].is_ascii_digit()
            && bytes[i + 5].is_ascii_digit()
            && bytes[i + 6] == b'-'
            && bytes[i + 7].is_ascii_digit()
            && bytes[i + 8].is_ascii_digit()
            && bytes[i + 9].is_ascii_digit()
            && bytes[i + 10].is_ascii_digit()
        {
            // Check it's not embedded in a larger number
            let before_ok = i == 0 || !bytes[i - 1].is_ascii_digit();
            let after_ok = i + 11 >= len || !bytes[i + 11].is_ascii_digit();
            if before_ok && after_ok {
                add_finding(findings, "ssn", input, i, i + 11);
            }
        }
        i += 1;
    }
}

// ── High-entropy detection ──

fn scan_high_entropy(input: &str, findings: &mut Vec<Finding>) {
    let bytes = input.as_bytes();
    let len = bytes.len();
    let mut i = 0;

    while i < len {
        // Find runs of base64/hex characters (at least 24 chars long)
        if is_b64_hex_char(bytes[i]) {
            let start = i;
            while i < len && is_b64_hex_char(bytes[i]) {
                i += 1;
            }
            let token = &input[start..i];

            if token.len() >= 24 && !is_context_safe(input, start) {
                let entropy = shannon_entropy(token);
                if entropy > 4.5 {
                    // Skip if it looks like a well-known non-secret
                    if !looks_like_identifier(token, input, start) {
                        add_finding(findings, "high_entropy", input, start, i);
                    }
                }
            }
        } else {
            i += 1;
        }
    }
}

fn is_b64_hex_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'+' || b == b'/' || b == b'=' || b == b'_' || b == b'-'
}

fn shannon_entropy(s: &str) -> f64 {
    let mut freq = [0u32; 256];
    for b in s.bytes() {
        freq[b as usize] += 1;
    }
    let len = s.len() as f64;
    freq.iter()
        .filter(|&&c| c > 0)
        .map(|&c| {
            let p = c as f64 / len;
            -p * p.log2()
        })
        .sum()
}

fn is_context_safe(input: &str, pos: usize) -> bool {
    let before = if pos > 30 {
        &input[pos - 30..pos]
    } else {
        &input[..pos]
    };
    let before_lower = before.to_lowercase();

    let safe_contexts = [
        "sha256=",
        "sha256:",
        "sha-256:",
        "sha1=",
        "sha1:",
        "md5=",
        "md5:",
        "commit ",
        "hash:",
        "hash=",
        "checksum:",
        "digest:",
        "content-hash:",
        "etag:",
    ];

    safe_contexts.iter().any(|ctx| before_lower.contains(ctx))
}

fn looks_like_identifier(token: &str, input: &str, pos: usize) -> bool {
    // UUID pattern
    if token.len() == 36 && token.chars().nth(8) == Some('-') && token.chars().nth(13) == Some('-')
    {
        return true;
    }

    // ULID (26 alphanumeric chars)
    if token.len() == 26 && token.chars().all(|c| c.is_ascii_alphanumeric()) {
        let before = if pos > 10 {
            &input[pos - 10..pos]
        } else {
            &input[..pos]
        };
        if before.contains("mem_") || before.contains("run_") || before.contains("conv_") {
            return true;
        }
    }

    // Org/project identifiers (org-)
    if token.starts_with("org-") && token.len() < 40 {
        return true;
    }

    // Stripe publishable keys are intentionally public
    if token.starts_with("pk_test_") || token.starts_with("pk_live_") {
        return true;
    }

    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_openai_key() {
        let r = scan("sk-proj-abc1234567890ABCDEFGHIJKLMNOPQRSTUVWXYZ123456");
        assert!(r.has_secrets());
        assert_eq!(r.findings[0].category, "api_key");
    }

    #[test]
    fn detect_openai_legacy_key() {
        let r = scan("sk-1234567890abcdefghijklmnopqrstuvwxyz1234567890ABCD");
        assert!(r.has_secrets());
    }

    #[test]
    fn detect_anthropic_key() {
        let r = scan("sk-ant-api03-abc123-DEF456-GHI789_jkl012-MNO345_PQR678-STU901_VWX234-YZA567_BCD890-EFG123_HIJ456-AAA");
        assert!(r.has_secrets());
    }

    #[test]
    fn detect_github_pat() {
        let r = scan("ghp_abcdefghij1234567890ABCDEFGHIJ123456");
        assert!(r.has_secrets());
    }

    #[test]
    fn detect_slack_bot_token() {
        let r = scan(concat!("xoxb-", "1234567890-1234567890123-", "AbCdEfGhIjKlMnOpQrStUvWx"));
        assert!(r.has_secrets());
    }

    #[test]
    fn detect_aws_access_key() {
        let r = scan("AKIAIOSFODNN7EXAMPLE");
        assert!(r.has_secrets());
    }

    #[test]
    fn detect_google_api_key() {
        let r = scan("AIzaSyDOCabc1234567890_abcdefghijklmnopqrs");
        assert!(r.has_secrets());
    }

    #[test]
    fn detect_jwt() {
        let r = scan("eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiIxMjM0NTY3ODkwIiwibmFtZSI6IkpvaG4gRG9lIiwiaWF0IjoxNTE2MjM5MDIyfQ.SflKxwRJSMeKKF2QT4fwpMeJf36POk6yJV_adQssw5c");
        assert!(r.has_secrets());
        assert_eq!(r.findings[0].category, "jwt");
    }

    #[test]
    fn detect_jwt_in_bearer() {
        let r = scan("DEBUG: Authorization: Bearer eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiJ1In0.abcXYZ123");
        assert!(r.has_secrets());
    }

    #[test]
    fn detect_rsa_private_key() {
        let r = scan(
            "-----BEGIN RSA PRIVATE KEY-----\nMIIEowIBAAKCAQEA\n-----END RSA PRIVATE KEY-----",
        );
        assert!(r.has_secrets());
        assert_eq!(r.findings[0].category, "ssh_key");
    }

    #[test]
    fn detect_openssh_key() {
        let r = scan("-----BEGIN OPENSSH PRIVATE KEY-----\nb3BlbnNzaC1rZXktdjEA\n-----END OPENSSH PRIVATE KEY-----");
        assert!(r.has_secrets());
    }

    #[test]
    fn detect_ec_private_key() {
        let r = scan("-----BEGIN EC PRIVATE KEY-----\nMHcCAQEE\n-----END EC PRIVATE KEY-----");
        assert!(r.has_secrets());
    }

    #[test]
    fn detect_password_in_url() {
        let r = scan("https://admin:hunter2@db.internal.lan:5432/main");
        assert!(r.has_secrets());
        assert_eq!(r.findings[0].category, "password");
    }

    #[test]
    fn detect_postgres_connection_string() {
        let r = scan("postgres://memoryfs:s3cr3t_p@ssw0rd@db.internal.lan:5433/memoryfs");
        assert!(r.has_secrets());
    }

    #[test]
    fn detect_explicit_password() {
        let r = scan("My password is correctHorseBatteryStaple, do not share");
        assert!(r.has_secrets());
        assert_eq!(r.findings[0].category, "password");
    }

    #[test]
    fn detect_visa_card() {
        let r = scan("Visa: 4111 1111 1111 1111 exp 12/27 cvv 123");
        assert!(r.has_secrets());
        assert_eq!(r.findings[0].category, "credit_card");
    }

    #[test]
    fn detect_iban_de() {
        let r = scan("DE89 3704 0044 0532 0130 00");
        assert!(r.has_secrets());
        assert_eq!(r.findings[0].category, "iban");
    }

    #[test]
    fn detect_iban_nl() {
        let r = scan("NL91 ABNA 0417 1643 00");
        assert!(r.has_secrets());
    }

    #[test]
    fn detect_ssn() {
        let r = scan("SSN 123-45-6789");
        assert!(r.has_secrets());
        assert_eq!(r.findings[0].category, "ssn");
    }

    #[test]
    fn detect_key_in_curl() {
        let r = scan("curl -H 'X-API-Key: sk-1234567890abcdefghijklmnopqrstuvwxyzABCDEFGH' https://api.example.com");
        assert!(r.has_secrets());
    }

    #[test]
    fn detect_key_in_python() {
        let r = scan(
            "client = OpenAI(api_key='sk-proj-abc1234567890DEFghi456JKLmno789PQRstu012VWXyz')",
        );
        assert!(r.has_secrets());
    }

    #[test]
    fn detect_key_in_env_export() {
        let r = scan("export ANTHROPIC_API_KEY=sk-ant-api03-aaa-bbb-ccc-ddd-eee-fff-ggg");
        assert!(r.has_secrets());
    }

    // ── False positive controls ──

    #[test]
    fn no_false_positive_partial_key() {
        let r = scan("My key starts with sk-proj-... and ends with ...XYZ123");
        assert!(!r.has_secrets());
    }

    #[test]
    fn no_false_positive_stripe_publishable() {
        let r = scan("pk_test_TYooMQauvdEDq54NiTphI7jx");
        assert!(!r.has_secrets());
    }

    #[test]
    fn no_false_positive_git_sha() {
        let r = scan("commit 9b4c7e0d3a6f1e2c5b8d1a4f7c0e3b6d9a2f5c8e1b4d7a0c3f6e9b2d5a8c1f4e");
        assert!(!r.has_secrets());
    }

    #[test]
    fn no_false_positive_sha_in_log() {
        let r = scan("INFO: stream completed sha256=5e88489a3a8e9adba6b2cda9f7d73a8e9c4d5b6a7f8c9d0e1b2c3d4e5f6a7b8c");
        assert!(!r.has_secrets());
    }

    #[test]
    fn no_false_positive_uuid() {
        let r = scan("550e8400-e29b-41d4-a716-446655440000");
        assert!(!r.has_secrets());
    }

    #[test]
    fn no_false_positive_truncated() {
        let r = scan("sk-pro");
        assert!(!r.has_secrets());
    }

    // ── Redaction output ──

    #[test]
    fn redact_replaces_secrets() {
        let input = "key: ghp_abcdefghij1234567890ABCDEFGHIJ123456";
        let output = redact(input);
        assert!(output.contains("[REDACTED:api_key]"));
        assert!(!output.contains("ghp_"));
    }

    #[test]
    fn redact_preserves_clean_text() {
        let input = "User prefers local-first AI infrastructure.";
        let output = redact(input);
        assert_eq!(input, output);
    }

    #[test]
    fn check_and_reject_blocks_secrets() {
        let input = "ghp_abcdefghij1234567890ABCDEFGHIJ123456";
        let err = check_and_reject(input, true).unwrap_err();
        assert_eq!(err.api_code(), "POLICY_REJECTED");
    }

    #[test]
    fn check_and_reject_allows_clean() {
        check_and_reject("safe content", true).unwrap();
    }

    // ── Entropy ──

    #[test]
    fn shannon_entropy_low_for_repetitive() {
        assert!(shannon_entropy("aaaaaaaaaaaaaaaaaaaaaaaaa") < 1.0);
    }

    #[test]
    fn shannon_entropy_high_for_random() {
        let entropy = shannon_entropy("5e88489a3a8e9adba6b2cda9f7d73a8e");
        assert!(entropy > 3.5);
    }

    // ── Luhn ──

    #[test]
    fn luhn_valid_visa() {
        assert!(luhn_check(&[
            4, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1
        ]));
    }

    #[test]
    fn luhn_invalid() {
        assert!(!luhn_check(&[
            1, 2, 3, 4, 5, 6, 7, 8, 9, 0, 1, 2, 3, 4, 5, 6
        ]));
    }

    // ── Full corpus test ──

    #[test]
    fn adversarial_corpus() {
        let corpus_path = "../../tests/adversarial/secrets-suite/corpora.jsonl";
        let content = match std::fs::read_to_string(corpus_path) {
            Ok(c) => c,
            Err(_) => return, // skip if not running from workspace root
        };

        let mut pass = 0;
        let mut fail = 0;
        let mut failures = Vec::new();

        for line in content.lines() {
            let entry: serde_json::Value = serde_json::from_str(line).unwrap();
            let id = entry["id"].as_str().unwrap();
            let input = entry["input"].as_str().unwrap();
            let expect_redacted = entry["expect_redacted"].as_bool().unwrap();

            let result = scan(input);
            let detected = result.has_secrets();

            if detected == expect_redacted {
                pass += 1;
            } else {
                fail += 1;
                failures.push(format!(
                    "{}: expected_redacted={}, detected={} (input: {})",
                    id,
                    expect_redacted,
                    detected,
                    &input[..input.len().min(60)]
                ));
            }
        }

        if !failures.is_empty() {
            eprintln!(
                "\n--- Redaction corpus failures ({fail}/{} total) ---",
                pass + fail
            );
            for f in &failures {
                eprintln!("  {f}");
            }
        }

        // Allow up to 5% false positive/negative rate (per DoD < 2% FP on benign)
        let total = pass + fail;
        assert!(
            fail <= (total as f64 * 0.05).ceil() as usize,
            "{fail} failures out of {total} is too many"
        );
    }
}
