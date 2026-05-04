//! Heading-aware markdown chunker.
//!
//! Splits a markdown document into [`Chunk`]s, respecting heading hierarchy,
//! YAML frontmatter, fenced code blocks, and configurable size limits with overlap.

/// Configuration for the chunker.
#[derive(Debug, Clone)]
pub struct ChunkConfig {
    /// Maximum chunk size in characters (default: 2000).
    pub max_chunk_size: usize,
    /// Overlap between consecutive chunks in characters (default: 200).
    pub overlap_size: usize,
    /// Document title to prepend as context to each chunk's text.
    /// When set, chunks get a prefix like `"Title > Heading: "` which
    /// makes embeddings of short/generic paragraphs more distinctive.
    pub document_title: Option<String>,
}

impl Default for ChunkConfig {
    fn default() -> Self {
        Self {
            max_chunk_size: 2000,
            overlap_size: 200,
            document_title: None,
        }
    }
}

/// A single chunk extracted from a markdown document.
#[derive(Debug, Clone)]
pub struct Chunk {
    /// The chunk text content.
    pub text: String,
    /// Heading path from root, e.g. `["Architecture", "Storage", "Object Store"]`.
    pub heading_path: Vec<String>,
    /// Heading level of the deepest heading (0 for preamble/no heading).
    pub heading_level: u8,
    /// Byte offset of chunk start in the original input.
    pub byte_start: usize,
    /// Byte offset of chunk end in the original input.
    pub byte_end: usize,
    /// Character offset of chunk start.
    pub char_start: usize,
    /// Character offset of chunk end.
    pub char_end: usize,
}

/// A raw section before splitting for size.
struct Section {
    text: String,
    heading_path: Vec<String>,
    heading_level: u8,
    byte_start: usize,
    byte_end: usize,
}

/// Parse the heading level and title from a line known to start with `#`.
/// Returns `None` if the line doesn't match (more than 6 hashes, or no space after hashes).
fn parse_heading(line: &str) -> Option<(u8, &str)> {
    let hashes = line.bytes().take_while(|&b| b == b'#').count();
    if hashes == 0 || hashes > 6 {
        return None;
    }
    let rest = &line[hashes..];
    if let Some(stripped) = rest.strip_prefix(' ') {
        Some((hashes as u8, stripped.trim()))
    } else {
        None
    }
}

/// Determine if a line starts a fenced code block (```` ``` ```` or `~~~`).
fn is_fence_start(line: &str) -> bool {
    line.starts_with("```") || line.starts_with("~~~")
}

/// Check if a line closes the currently open fence (same fence prefix).
fn closes_fence(line: &str, open_prefix: &str) -> bool {
    line.starts_with(open_prefix)
        && line[open_prefix.len()..]
            .chars()
            .all(|c| c == open_prefix.chars().next().unwrap_or('`') || c.is_whitespace())
}

/// Skip YAML frontmatter at the start of `input`. Returns the byte offset after the
/// closing `---\n`, or 0 if there is no frontmatter.
fn skip_frontmatter(input: &str) -> usize {
    if !input.starts_with("---\n") {
        return 0;
    }
    // Search for closing `---\n` starting after the opening delimiter.
    let after_open = 4; // len("---\n")
    let rest = &input[after_open..];
    // Look for `\n---\n` or `---\n` at the start of a line.
    if let Some(pos) = rest.find("\n---\n") {
        after_open + pos + 5 // skip \n---\n
    } else if rest.starts_with("---\n") {
        // Degenerate: closing fence immediately after opening.
        after_open + 4
    } else {
        0
    }
}

/// Split a section's text at paragraph boundaries (`\n\n`) and emit sub-chunks
/// with byte/char offsets relative to the original input. Overlap is applied
/// between sub-chunks within the same section.
fn split_section(section: Section, config: &ChunkConfig, original: &str, chunks: &mut Vec<Chunk>) {
    let text = section.text;
    let content = text.trim();
    if content.is_empty() {
        return;
    }

    // If the section fits within the limit, emit as-is.
    let char_count = content.chars().count();
    if char_count <= config.max_chunk_size {
        let byte_start = section.byte_start + text.find(content).unwrap_or(0);
        let byte_end = byte_start + content.len();
        let char_start = original[..byte_start].chars().count();
        let char_end = char_start + char_count;
        chunks.push(Chunk {
            text: content.to_string(),
            heading_path: section.heading_path,
            heading_level: section.heading_level,
            byte_start,
            byte_end,
            char_start,
            char_end,
        });
        return;
    }

    // Split at paragraph boundaries.
    // We collect paragraph byte ranges within the trimmed content (relative to section.byte_start + trim_offset).
    let trim_offset = text.find(content).unwrap_or(0);
    let content_byte_start = section.byte_start + trim_offset;

    // Split content at "\n\n" boundaries.
    let paragraphs: Vec<&str> = content.split("\n\n").collect();

    // Build sub-chunks by accumulating paragraphs until we would exceed max_chunk_size.
    let mut sub_chunks: Vec<String> = Vec::new();
    let mut current = String::new();

    for (i, para) in paragraphs.iter().enumerate() {
        let sep = if current.is_empty() { "" } else { "\n\n" };
        let candidate_len = current.chars().count() + sep.len() + para.chars().count();
        if !current.is_empty() && candidate_len > config.max_chunk_size {
            sub_chunks.push(current.clone());
            current = para.to_string();
        } else {
            if !sep.is_empty() {
                current.push_str(sep);
            }
            current.push_str(para);
        }
        if i == paragraphs.len() - 1 && !current.is_empty() {
            sub_chunks.push(current.clone());
        }
    }

    if sub_chunks.is_empty() {
        return;
    }

    // Walk through the content string to compute byte offsets for each sub-chunk.
    // We track our search position within `content`.
    let mut search_from = 0usize; // byte offset within `content`
    let mut prev_sub_text: Option<String> = None;

    for sub in &sub_chunks {
        // Compute the text we'll actually emit (may have overlap prepended).
        let emit_text = if let Some(ref prev) = prev_sub_text {
            // Prepend up to overlap_size chars from the end of the previous sub-chunk.
            let prev_chars: Vec<char> = prev.chars().collect();
            let overlap_chars = prev_chars.len().min(config.overlap_size);
            let overlap_start = prev_chars.len() - overlap_chars;
            let overlap_str: String = prev_chars[overlap_start..].iter().collect();
            format!("{}{}", overlap_str, sub)
        } else {
            sub.clone()
        };

        // Find `sub` in content starting from search_from.
        let sub_byte_offset_in_content = content[search_from..]
            .find(sub.as_str())
            .map(|rel| search_from + rel)
            .unwrap_or(search_from);

        let byte_start = content_byte_start + sub_byte_offset_in_content;
        let byte_end = byte_start + sub.len();
        let char_start = original[..byte_start].chars().count();
        let char_end = char_start + emit_text.chars().count();

        chunks.push(Chunk {
            text: emit_text.clone(),
            heading_path: section.heading_path.clone(),
            heading_level: section.heading_level,
            byte_start,
            byte_end,
            char_start,
            char_end,
        });

        search_from = sub_byte_offset_in_content + sub.len();
        prev_sub_text = Some(sub.clone());
    }
}

/// Extract all sections from the markdown body (after frontmatter).
fn collect_sections(body: &str, body_byte_offset: usize) -> Vec<Section> {
    let mut sections: Vec<Section> = Vec::new();
    // Heading stack: (level, title).
    let mut heading_stack: Vec<(u8, String)> = Vec::new();
    // Current section state.
    let mut current_heading_level: u8 = 0;
    let mut current_lines: Vec<&str> = Vec::new();
    let mut current_byte_start: usize = body_byte_offset;
    // Fenced code block tracking.
    let mut in_code_block = false;
    let mut fence_prefix: &str = "";

    let mut byte_cursor: usize = body_byte_offset;

    for line in body.lines() {
        let line_byte_len = line.len() + 1; // +1 for '\n'

        if in_code_block {
            current_lines.push(line);
            // Check if this line closes the fence.
            if closes_fence(line, fence_prefix) {
                in_code_block = false;
                fence_prefix = "";
            }
            byte_cursor += line_byte_len;
            continue;
        }

        // Detect fence open.
        if is_fence_start(line) {
            in_code_block = true;
            fence_prefix = if line.starts_with("```") {
                "```"
            } else {
                "~~~"
            };
            current_lines.push(line);
            byte_cursor += line_byte_len;
            continue;
        }

        // Check for heading.
        if line.starts_with('#') {
            if let Some((level, title)) = parse_heading(line) {
                // Flush current section.
                let section_text = current_lines.join("\n");
                // Only push non-empty sections (or preamble).
                sections.push(Section {
                    text: section_text,
                    heading_path: heading_stack.iter().map(|(_, t)| t.clone()).collect(),
                    heading_level: current_heading_level,
                    byte_start: current_byte_start,
                    byte_end: byte_cursor,
                });

                // Update heading stack.
                // Pop entries that are at same or deeper level.
                while let Some(&(top_level, _)) = heading_stack.last() {
                    if top_level >= level {
                        heading_stack.pop();
                    } else {
                        break;
                    }
                }
                heading_stack.push((level, title.to_string()));

                // Start new section (include the heading line itself).
                current_heading_level = level;
                current_lines = vec![line];
                current_byte_start = byte_cursor;
                byte_cursor += line_byte_len;
                continue;
            }
        }

        current_lines.push(line);
        byte_cursor += line_byte_len;
    }

    // Flush trailing section.
    let section_text = current_lines.join("\n");
    sections.push(Section {
        text: section_text,
        heading_path: heading_stack.iter().map(|(_, t)| t.clone()).collect(),
        heading_level: current_heading_level,
        byte_start: current_byte_start,
        byte_end: byte_cursor,
    });

    sections
}

/// Build a context prefix from the document title and heading path.
/// Example: `"Normans > Military campaigns: "` or just `"Normans: "`.
fn build_context_prefix(title: &str, heading_path: &[String]) -> String {
    if heading_path.is_empty() {
        return format!("{title}: ");
    }
    let leaf = heading_path.last().unwrap();
    if leaf == title {
        // Heading is the same as the document title — no duplication.
        return format!("{title}: ");
    }
    format!("{title} > {}: ", heading_path.join(" > "))
}

/// Split a markdown document into [`Chunk`]s.
///
/// When `config.document_title` is set, each chunk's `text` is prefixed with
/// `"Title > Heading Path: "` to make embeddings of short/generic paragraphs
/// more distinctive across different documents.
pub fn chunk_markdown(input: &str, config: &ChunkConfig) -> Vec<Chunk> {
    let frontmatter_end = skip_frontmatter(input);
    let body = &input[frontmatter_end..];

    let sections = collect_sections(body, frontmatter_end);

    let mut chunks: Vec<Chunk> = Vec::new();
    for section in sections {
        split_section(section, config, input, &mut chunks);
    }

    if let Some(ref title) = config.document_title {
        for chunk in &mut chunks {
            let prefix = build_context_prefix(title, &chunk.heading_path);
            chunk.text = format!("{prefix}{}", chunk.text);
        }
    }

    chunks
}

#[cfg(test)]
#[allow(missing_docs)]
mod tests {
    use super::*;

    fn cfg(max: usize, overlap: usize) -> ChunkConfig {
        ChunkConfig {
            max_chunk_size: max,
            overlap_size: overlap,
            document_title: None,
        }
    }

    fn cfg_with_title(max: usize, overlap: usize, title: &str) -> ChunkConfig {
        ChunkConfig {
            max_chunk_size: max,
            overlap_size: overlap,
            document_title: Some(title.to_string()),
        }
    }

    fn default_cfg() -> ChunkConfig {
        ChunkConfig::default()
    }

    // ── 1. empty_input ──────────────────────────────────────────────────────
    #[test]
    fn empty_input() {
        let chunks = chunk_markdown("", &default_cfg());
        assert!(chunks.is_empty());
    }

    // ── 2. frontmatter_only ─────────────────────────────────────────────────
    #[test]
    fn frontmatter_only() {
        let input = "---\ntitle: test\n---\n";
        let chunks = chunk_markdown(input, &default_cfg());
        assert!(chunks.is_empty());
    }

    // ── 3. frontmatter_skipped ──────────────────────────────────────────────
    #[test]
    fn frontmatter_skipped() {
        let input = "---\ntitle: test\n---\n# Hello\n\nSome body text.\n";
        let chunks = chunk_markdown(input, &default_cfg());
        assert!(!chunks.is_empty());
        for chunk in &chunks {
            assert!(
                !chunk.text.contains("title: test"),
                "frontmatter leaked into chunk"
            );
        }
    }

    // ── 4. single_paragraph ─────────────────────────────────────────────────
    #[test]
    fn single_paragraph() {
        let input = "Just some plain text with no headings.";
        let chunks = chunk_markdown(input, &default_cfg());
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].heading_level, 0);
        assert!(chunks[0].heading_path.is_empty());
        assert!(chunks[0].text.contains("plain text"));
    }

    // ── 5. h1_sections ──────────────────────────────────────────────────────
    #[test]
    fn h1_sections() {
        let input = "# First\n\nContent one.\n\n# Second\n\nContent two.\n";
        let chunks = chunk_markdown(input, &default_cfg());
        assert_eq!(chunks.len(), 2);
        assert!(chunks[0].text.contains("First"));
        assert!(chunks[1].text.contains("Second"));
    }

    // ── 6. nested_headings ──────────────────────────────────────────────────
    #[test]
    fn nested_headings() {
        let input = "# Architecture\n\n## Storage\n\n### Object Store\n\nContent.\n";
        let chunks = chunk_markdown(input, &default_cfg());
        // Expect sections for each heading (Architecture, Storage, Object Store).
        let object_store = chunks
            .iter()
            .find(|c| c.text.contains("Object Store"))
            .expect("Object Store chunk not found");
        assert_eq!(
            object_store.heading_path,
            vec!["Architecture", "Storage", "Object Store"]
        );
        assert_eq!(object_store.heading_level, 3);

        let storage = chunks
            .iter()
            .find(|c| {
                c.heading_path == vec!["Architecture", "Storage"]
                    && !c.text.contains("Object Store")
            })
            .expect("Storage chunk not found");
        assert_eq!(storage.heading_level, 2);
    }

    // ── 7. heading_path_reset ────────────────────────────────────────────────
    #[test]
    fn heading_path_reset() {
        let input = "# First\n\n## Sub\n\nContent.\n\n# Second\n\nNew section.\n";
        let chunks = chunk_markdown(input, &default_cfg());
        let second = chunks
            .iter()
            .find(|c| c.text.contains("Second"))
            .expect("Second chunk not found");
        assert_eq!(second.heading_path, vec!["Second"]);
        assert_eq!(second.heading_level, 1);
    }

    // ── 8. code_block_hashes_not_headings ───────────────────────────────────
    #[test]
    fn code_block_hashes_not_headings() {
        let input =
            "# Real Heading\n\n```python\n# comment in code\ndef foo(): pass\n```\n\nBody text.\n";
        let chunks = chunk_markdown(input, &default_cfg());
        // There should be exactly one section (the real heading), not two.
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].heading_level, 1);
        assert!(chunks[0].text.contains("comment in code"));
    }

    // ── 9. tilde_code_block ─────────────────────────────────────────────────
    #[test]
    fn tilde_code_block() {
        let input = "# Heading\n\n~~~sh\n# not a heading\necho hi\n~~~\n\nText.\n";
        let chunks = chunk_markdown(input, &default_cfg());
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].heading_level, 1);
        assert!(chunks[0].text.contains("not a heading"));
    }

    // ── 10. long_section_split ──────────────────────────────────────────────
    #[test]
    fn long_section_split() {
        // Build a section with two large paragraphs separated by \n\n.
        let para_a = "a".repeat(1500);
        let para_b = "b".repeat(1500);
        let input = format!("# Title\n\n{para_a}\n\n{para_b}\n");
        let chunks = chunk_markdown(&input, &cfg(2000, 0));
        // Should have been split into at least two sub-chunks (each para ~1500 chars).
        assert!(
            chunks.len() >= 2,
            "expected split; got {} chunks",
            chunks.len()
        );
    }

    // ── 11. overlap_applied ─────────────────────────────────────────────────
    #[test]
    fn overlap_applied() {
        let para_a = "AAAA".repeat(400); // 1600 chars
        let para_b = "BBBB".repeat(400); // 1600 chars
        let input = format!("# Title\n\n{para_a}\n\n{para_b}\n");
        let chunks = chunk_markdown(&input, &cfg(2000, 100));
        assert!(chunks.len() >= 2);
        // The second sub-chunk should start with overlap from para_a (all A's).
        let second = &chunks[1];
        assert!(
            second.text.starts_with('A'),
            "overlap not applied; second chunk starts with: {:?}",
            &second.text[..second.text.len().min(20)]
        );
    }

    // ── 12. byte_ranges_correct ─────────────────────────────────────────────
    #[test]
    fn byte_ranges_correct() {
        let input = "Hello world.\n\nSecond paragraph.\n";
        let chunks = chunk_markdown(input, &default_cfg());
        assert_eq!(chunks.len(), 1);
        let chunk = &chunks[0];
        let extracted = &input[chunk.byte_start..chunk.byte_end];
        assert_eq!(extracted.trim(), chunk.text.trim());
    }

    // ── 13. char_ranges_correct ─────────────────────────────────────────────
    #[test]
    fn char_ranges_correct() {
        let input = "# Heading\n\nSimple body.\n";
        let chunks = chunk_markdown(input, &default_cfg());
        assert_eq!(chunks.len(), 1);
        let chunk = &chunks[0];
        let chars: Vec<char> = input.chars().collect();
        let extracted: String = chars[chunk.char_start..chunk.char_end].iter().collect();
        assert_eq!(extracted.trim(), chunk.text.trim());
    }

    // ── 14. unicode_content ─────────────────────────────────────────────────
    #[test]
    fn unicode_content() {
        let input = "# Раздел\n\nПривет мир 🌍.\n";
        let chunks = chunk_markdown(input, &default_cfg());
        assert_eq!(chunks.len(), 1);
        let chunk = &chunks[0];
        assert!(chunk.text.contains("Привет"));
        assert!(chunk.text.contains("🌍"));
        // Verify char offsets are sane (char_end > char_start).
        assert!(chunk.char_end > chunk.char_start);
        // Reconstruct from char offsets.
        let chars: Vec<char> = input.chars().collect();
        let reconstructed: String = chars[chunk.char_start..chunk.char_end].iter().collect();
        assert!(reconstructed.contains("Привет"));
    }

    // ── 15. nested_lists ────────────────────────────────────────────────────
    #[test]
    fn nested_lists() {
        let input = "# Section\n\n- item 1\n  - sub item\n- item 2\n\nEnd.\n";
        let chunks = chunk_markdown(input, &default_cfg());
        assert_eq!(chunks.len(), 1);
        assert!(chunks[0].text.contains("sub item"));
        assert_eq!(chunks[0].heading_level, 1);
    }

    // ── 16. table_content ───────────────────────────────────────────────────
    #[test]
    fn table_content() {
        let input =
            "# Section\n\n| Col A | Col B |\n|-------|-------|\n| val 1 | val 2 |\n\nEnd.\n";
        let chunks = chunk_markdown(input, &default_cfg());
        assert_eq!(chunks.len(), 1);
        assert!(chunks[0].text.contains("Col A"));
        assert_eq!(chunks[0].heading_level, 1);
    }

    // ── 17. heading_in_middle_of_text ───────────────────────────────────────
    #[test]
    fn heading_in_middle_of_text() {
        let input = "Preamble text.\n\n# New Section\n\nSection body.\n";
        let chunks = chunk_markdown(input, &default_cfg());
        // Preamble and section should be separate chunks.
        assert_eq!(chunks.len(), 2);
        let preamble = &chunks[0];
        assert_eq!(preamble.heading_level, 0);
        assert!(preamble.text.contains("Preamble"));
        let section = &chunks[1];
        assert_eq!(section.heading_level, 1);
        assert!(section.text.contains("Section body"));
    }

    // ── 18. multiple_heading_levels ─────────────────────────────────────────
    #[test]
    fn multiple_heading_levels() {
        let input = "# H1\n\n## H2\n\n### H3\n\n#### H4\n\nDeep content.\n";
        let chunks = chunk_markdown(input, &default_cfg());
        let deep = chunks
            .iter()
            .find(|c| c.text.contains("Deep content"))
            .expect("deep chunk not found");
        assert_eq!(deep.heading_path.len(), 4);
        assert_eq!(deep.heading_level, 4);
        assert_eq!(deep.heading_path, vec!["H1", "H2", "H3", "H4"]);
    }

    // ── 19. only_whitespace_after_frontmatter ───────────────────────────────
    #[test]
    fn only_whitespace_after_frontmatter() {
        let input = "---\ntitle: test\n---\n   \n\t\n";
        let chunks = chunk_markdown(input, &default_cfg());
        assert!(chunks.is_empty());
    }

    // ── 20. consecutive_headings ────────────────────────────────────────────
    #[test]
    fn consecutive_headings() {
        let input = "# H1\n## H2\n\nContent under H2.\n";
        let chunks = chunk_markdown(input, &default_cfg());
        // H1 section has no body content → should be skipped or empty.
        let h2_chunk = chunks
            .iter()
            .find(|c| c.text.contains("Content under H2"))
            .expect("H2 content chunk not found");
        assert!(h2_chunk.heading_path.contains(&"H2".to_string()));
        // Ensure no chunk consists only of the H1 line with substantial text.
        for chunk in &chunks {
            if chunk.heading_level == 1 && !chunk.text.contains("Content") {
                // H1 chunk should be minimal/empty body.
                let body = chunk.text.trim_start_matches("# H1").trim();
                assert!(
                    body.is_empty(),
                    "H1-only chunk should have no body: {:?}",
                    body
                );
            }
        }
    }

    // ── 21. context_prefix_with_title ───────────────────────────────────────
    #[test]
    fn context_prefix_with_title() {
        let input = "---\ntitle: Normans\n---\n# Normans\n\nThe Normans were Vikings.\n";
        let chunks = chunk_markdown(input, &cfg_with_title(2000, 0, "Normans"));
        assert_eq!(chunks.len(), 1);
        assert!(
            chunks[0].text.starts_with("Normans: "),
            "expected title prefix, got: {:?}",
            &chunks[0].text[..chunks[0].text.len().min(40)]
        );
    }

    // ── 22. context_prefix_with_nested_headings ────────────────────────────
    #[test]
    fn context_prefix_with_nested_headings() {
        let input = "# Architecture\n\n## Storage\n\nContent about storage.\n";
        let chunks = chunk_markdown(input, &cfg_with_title(2000, 0, "MemoryFS"));
        let storage = chunks
            .iter()
            .find(|c| c.text.contains("Content about storage"))
            .expect("storage chunk not found");
        assert!(
            storage
                .text
                .starts_with("MemoryFS > Architecture > Storage: "),
            "got: {:?}",
            &storage.text[..storage.text.len().min(60)]
        );
    }

    // ── 23. no_prefix_without_title ────────────────────────────────────────
    #[test]
    fn no_prefix_without_title() {
        let input = "# Section\n\nPlain text.\n";
        let chunks = chunk_markdown(input, &cfg(2000, 0));
        assert!(
            chunks[0].text.starts_with("# Section"),
            "should not have prefix without document_title"
        );
    }

    // ── 24. context_prefix_preamble_no_heading ─────────────────────────────
    #[test]
    fn context_prefix_preamble_no_heading() {
        let input = "Some standalone paragraph without headings.\n";
        let chunks = chunk_markdown(input, &cfg_with_title(2000, 0, "Construction"));
        assert_eq!(chunks.len(), 1);
        assert!(
            chunks[0].text.starts_with("Construction: "),
            "preamble should get title prefix, got: {:?}",
            &chunks[0].text[..chunks[0].text.len().min(40)]
        );
    }
}
