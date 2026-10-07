//! Context strategies, prompt assembly, and source citation validation.
//!
//! Enforces the 5-layer prompt order specified in Dociler architecture:
//! 1. Dociler core safety and grounding policy
//! 2. Relevant immutable built-in skill modules
//! 3. Client/system preferences (subordinate to core policy)
//! 4. Source-labelled document chunks inside explicit untrusted-content delimiters
//! 5. Conversation history and current user request
//!
//! Validates generated citation IDs against supplied source chunk IDs,
//! visibly marking unverified/hallucinated citations and reporting metrics.

use std::collections::HashSet;

use crate::indexing::{Bm25Index, Chunk, approx_token_count};

/// Explicit untrusted document boundary delimiter tags.
pub const UNTRUSTED_DOC_START: &str = "=== BEGIN UNTRUSTED DOCUMENT CONTENT ===";
pub const UNTRUSTED_DOC_END: &str = "=== END UNTRUSTED DOCUMENT CONTENT ===";

pub use crate::skills::{SKILLS_VERSION, SkillModule, select_task_skills};

/// Strategy for assembling document context for the model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContextStrategy {
    /// Small explicit selection: includes complete source-labelled blocks in document order.
    Direct,
    /// Factual question: retrieves top-scoring BM25 chunks and adjacent context.
    Retrieval,
    /// Long/multiple documents: partitions chunks deterministically into bounded batches for map/reduce.
    MapReduce,
}

/// Token budget configuration for prompt context construction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContextBudget {
    pub max_context_tokens: usize,
    pub reserved_completion_tokens: usize,
}

impl ContextBudget {
    /// Budget for `dociler-lite` (8,192 context, 1,024 reserved completion).
    pub fn lite() -> Self {
        Self {
            max_context_tokens: 8192,
            reserved_completion_tokens: 1024,
        }
    }

    /// Budget for `dociler-pro` (16,384 context, 2,048 reserved completion).
    pub fn pro() -> Self {
        Self {
            max_context_tokens: 16384,
            reserved_completion_tokens: 2048,
        }
    }

    /// Maximum tokens available for the assembled prompt (system + documents + user).
    pub fn available_prompt_tokens(&self) -> usize {
        self.max_context_tokens
            .saturating_sub(self.reserved_completion_tokens)
    }
}

impl Default for ContextBudget {
    fn default() -> Self {
        Self::lite()
    }
}

/// Selects chunks according to the chosen context strategy within the token budget.
pub fn select_chunks(
    strategy: ContextStrategy,
    all_chunks: &[Chunk],
    index: Option<&Bm25Index>,
    query: Option<&str>,
    budget_tokens: usize,
    file_filter: Option<&str>,
) -> Vec<Chunk> {
    match strategy {
        ContextStrategy::Direct => {
            let mut selected = Vec::new();
            let mut current_tokens = 0;
            for chunk in all_chunks {
                if let Some(filter) = file_filter {
                    if !chunk.document_display_name.eq_ignore_ascii_case(filter)
                        && !chunk.document_digest.starts_with(filter)
                    {
                        continue;
                    }
                }
                let chunk_cost =
                    chunk.token_count + approx_token_count(&chunk.citation_label()) + 10;
                if current_tokens + chunk_cost > budget_tokens {
                    break;
                }
                selected.push(chunk.clone());
                current_tokens += chunk_cost;
            }
            selected
        }
        ContextStrategy::Retrieval => {
            let Some(idx) = index else {
                return select_chunks(
                    ContextStrategy::Direct,
                    all_chunks,
                    None,
                    query,
                    budget_tokens,
                    file_filter,
                );
            };
            let q = query.unwrap_or("");
            let search_results = idx.search(q, 20, file_filter);

            let mut selected_indices = Vec::new();
            let mut seen_ids = HashSet::new();
            let mut current_tokens = 0;

            for res in search_results {
                if seen_ids.contains(&res.chunk.id) {
                    continue;
                }
                let cost =
                    res.chunk.token_count + approx_token_count(&res.chunk.citation_label()) + 10;
                if current_tokens + cost > budget_tokens {
                    break;
                }
                seen_ids.insert(res.chunk.id.clone());
                selected_indices.push(res.chunk.clone());
                current_tokens += cost;

                // Add adjacent chunk for continuity if budget allows
                let adjacent = idx.get_adjacent_chunks(res.chunk.ordinal, 1);
                for adj in adjacent {
                    if seen_ids.contains(&adj.id) {
                        continue;
                    }
                    let adj_cost = adj.token_count + approx_token_count(&adj.citation_label()) + 10;
                    if current_tokens + adj_cost <= budget_tokens {
                        seen_ids.insert(adj.id.clone());
                        selected_indices.push(adj.clone());
                        current_tokens += adj_cost;
                    }
                }
            }

            // Sort back into document and ordinal order for coherent reading
            selected_indices.sort_by(|a, b| {
                a.document_digest
                    .cmp(&b.document_digest)
                    .then_with(|| a.ordinal.cmp(&b.ordinal))
            });
            selected_indices
        }
        ContextStrategy::MapReduce => {
            // For map phase of MapReduce, returns chunks up to budget
            select_chunks(
                ContextStrategy::Direct,
                all_chunks,
                None,
                query,
                budget_tokens,
                file_filter,
            )
        }
    }
}

/// Partitions all chunks into deterministic batches for Map/Reduce processing.
pub fn plan_map_reduce_batches(chunks: &[Chunk], batch_token_budget: usize) -> Vec<Vec<Chunk>> {
    let mut batches = Vec::new();
    let mut current_batch = Vec::new();
    let mut current_tokens = 0;

    for chunk in chunks {
        let cost = chunk.token_count + approx_token_count(&chunk.citation_label()) + 10;
        if current_tokens + cost > batch_token_budget && !current_batch.is_empty() {
            batches.push(std::mem::take(&mut current_batch));
            current_tokens = 0;
        }
        current_batch.push(chunk.clone());
        current_tokens += cost;
    }

    if !current_batch.is_empty() {
        batches.push(current_batch);
    }

    batches
}

/// Assembled prompt ready for model dispatch, tracking active source IDs for citation verification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssembledPrompt {
    pub system_prompt: String,
    pub user_prompt: String,
    pub strategy: ContextStrategy,
    pub active_source_ids: HashSet<String>,
    pub included_chunks: Vec<Chunk>,
    pub estimated_tokens: usize,
}

/// Assembles a prompt adhering strictly to the 5-layer Dociler prompt ordering.
pub fn assemble_prompt(
    strategy: ContextStrategy,
    skills: &[SkillModule],
    client_preferences: Option<&str>,
    chunks: &[Chunk],
    user_query: &str,
    budget: &ContextBudget,
) -> AssembledPrompt {
    // 1. Core safety and grounding policy
    let mut system_parts = Vec::new();
    system_parts.push(SkillModule::CorePolicy.content().to_string());

    // 2. Relevant built-in skills
    for skill in skills {
        if *skill != SkillModule::CorePolicy {
            system_parts.push(format!("## {}\n{}", skill.title(), skill.content()));
        }
    }

    // 3. Client / system preferences (subordinate to core policy)
    if let Some(prefs) = client_preferences {
        let trimmed = prefs.trim();
        if !trimmed.is_empty() {
            system_parts.push(format!("## Client Preferences\n{trimmed}"));
        }
    }

    let system_prompt = system_parts.join("\n\n");
    let system_tokens = approx_token_count(&system_prompt);
    let query_tokens = approx_token_count(user_query);

    // Remaining budget for untrusted document block
    let available_doc_tokens = budget
        .available_prompt_tokens()
        .saturating_sub(system_tokens + query_tokens + 50);

    let mut active_source_ids = HashSet::new();
    let mut included_chunks = Vec::new();
    let mut doc_block_text = String::new();
    let mut current_doc_tokens = 0;

    if !chunks.is_empty() {
        doc_block_text.push_str(UNTRUSTED_DOC_START);
        doc_block_text.push('\n');

        for chunk in chunks {
            let label = chunk.citation_label();
            let chunk_cost = chunk.token_count + approx_token_count(&label) + 10;
            if current_doc_tokens + chunk_cost > available_doc_tokens && !included_chunks.is_empty()
            {
                break;
            }

            doc_block_text.push_str(&label);
            doc_block_text.push('\n');
            doc_block_text.push_str(&chunk.text);
            doc_block_text.push_str("\n\n");

            active_source_ids.insert(chunk.id.clone());
            included_chunks.push(chunk.clone());
            current_doc_tokens += chunk_cost;
        }

        doc_block_text.push_str(UNTRUSTED_DOC_END);
    }

    // 4 & 5. User prompt contains document block + user query
    let user_prompt = if doc_block_text.is_empty() {
        user_query.to_string()
    } else {
        format!("{doc_block_text}\n\n{user_query}")
    };

    let total_estimated_tokens = system_tokens + approx_token_count(&user_prompt);

    AssembledPrompt {
        system_prompt,
        user_prompt,
        strategy,
        active_source_ids,
        included_chunks,
        estimated_tokens: total_estimated_tokens,
    }
}

/// Result of validating model citations against active source IDs.
#[derive(Debug, Clone, PartialEq)]
pub struct CitationValidationResult {
    /// Response text with invalid/hallucinated citations rewritten to `[unverified source: <id>]`.
    pub cleaned_text: String,
    /// Verified source citation IDs that matched active prompt chunks.
    pub verified_citations: Vec<String>,
    /// Unverified/invented source citation IDs not present in active prompt chunks.
    pub unverified_citations: Vec<String>,
    /// Total count of source citations found in the text.
    pub total_citations: usize,
}

impl CitationValidationResult {
    /// Ratio of valid citations (1.0 = all valid or none cited, 0.0 = all hallucinated).
    pub fn valid_ratio(&self) -> f64 {
        if self.total_citations == 0 {
            1.0
        } else {
            self.verified_citations.len() as f64 / self.total_citations as f64
        }
    }

    /// Whether any unverified/invented citations were detected.
    pub fn has_hallucinations(&self) -> bool {
        !self.unverified_citations.is_empty()
    }
}

/// Validates citations in generated model output against supplied source IDs.
///
/// Detects `[source: <id>]` and `[source: <id>, ...]` tags.
/// Verified citations remain intact.
/// Invented citations are marked as `[unverified source: <id>]` to prevent silent misattribution.
pub fn validate_citations(
    text: &str,
    valid_source_ids: &HashSet<String>,
) -> CitationValidationResult {
    let mut cleaned_text = String::with_capacity(text.len());
    let mut verified = Vec::new();
    let mut unverified = Vec::new();
    let mut total = 0;

    let marker = "[source:";
    let mut cursor = 0;

    while let Some(rel_start) = text[cursor..].find(marker) {
        let match_start = cursor + rel_start;
        cleaned_text.push_str(&text[cursor..match_start]);

        // Find closing bracket
        if let Some(rel_end) = text[match_start..].find(']') {
            let match_end = match_start + rel_end;
            let full_citation = &text[match_start..=match_end];
            let inner = &text[match_start + marker.len()..match_end].trim();

            // Extract the ID: substring up to comma or end of inner
            let id = if let Some((first, _)) = inner.split_once(',') {
                first.trim()
            } else {
                inner
            };

            total += 1;
            if valid_source_ids.contains(id) {
                verified.push(id.to_string());
                cleaned_text.push_str(full_citation);
            } else {
                unverified.push(id.to_string());
                // Rewrite impossible citation to unverified marker
                cleaned_text.push_str(&format!("[unverified source: {id}]"));
            }

            cursor = match_end + 1;
        } else {
            // No matching closing bracket, push rest and exit
            cleaned_text.push_str(&text[match_start..]);
            cursor = text.len();
            break;
        }
    }

    if cursor < text.len() {
        cleaned_text.push_str(&text[cursor..]);
    }

    CitationValidationResult {
        cleaned_text,
        verified_citations: verified,
        unverified_citations: unverified,
        total_citations: total,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::{DocumentFormat, DocumentMetadata, DocumentSource, SourceAnchor};
    use crate::indexing::{ChunkerOptions, chunk_document};

    fn make_test_chunk(id: &str, doc_name: &str, text: &str) -> Chunk {
        Chunk {
            id: id.to_string(),
            document_digest: "abcdef0123456789".to_string(),
            document_display_name: doc_name.to_string(),
            ordinal: 0,
            nearest_heading: Some("Overview".to_string()),
            page: Some(1),
            anchor: SourceAnchor::default(),
            token_count: approx_token_count(text),
            text: text.to_string(),
            searchable_terms: crate::indexing::tokenize_terms(text),
        }
    }

    #[test]
    fn direct_selection_strategy_packs_within_budget() {
        let c1 = make_test_chunk("abc:c0", "file1.txt", "First paragraph content here.");
        let c2 = make_test_chunk("abc:c1", "file1.txt", "Second paragraph content here.");
        let c3 = make_test_chunk("abc:c2", "file1.txt", "Third paragraph content here.");
        let chunks = vec![c1, c2, c3];

        let budget_tokens = 70; // Enough for 2 chunks (~30 tokens each)
        let selected = select_chunks(
            ContextStrategy::Direct,
            &chunks,
            None,
            None,
            budget_tokens,
            None,
        );

        assert_eq!(selected.len(), 2);
        assert_eq!(selected[0].id, "abc:c0");
        assert_eq!(selected[1].id, "abc:c1");
    }

    #[test]
    fn retrieval_strategy_uses_bm25_and_adjacent_context() {
        let doc_src = DocumentSource::from_bytes("data.txt", DocumentFormat::PlainText, b"fake");
        let blocks = vec![
            crate::document::Block::Paragraph {
                runs: vec![crate::document::InlineRun::Text(
                    "Introduction to neural network systems.".to_string(),
                )],
                anchor: SourceAnchor::default(),
            },
            crate::document::Block::Paragraph {
                runs: vec![crate::document::InlineRun::Text(
                    "Optimization and gradient descent techniques.".to_string(),
                )],
                anchor: SourceAnchor::default(),
            },
            crate::document::Block::Paragraph {
                runs: vec![crate::document::InlineRun::Text(
                    "Backpropagation algorithms and learning rates.".to_string(),
                )],
                anchor: SourceAnchor::default(),
            },
        ];
        let doc = crate::document::Document::new(doc_src, DocumentMetadata::default(), blocks);
        let chunks = chunk_document(&doc, &ChunkerOptions::default());

        let mut index = Bm25Index::default();
        index.add_document(&doc, &ChunkerOptions::default());

        let selected = select_chunks(
            ContextStrategy::Retrieval,
            &chunks,
            Some(&index),
            Some("gradient descent"),
            500,
            None,
        );

        assert!(!selected.is_empty());
        let ids: Vec<_> = selected.iter().map(|c| c.id.as_str()).collect();
        // The matching chunk (index 1) and its adjacent neighbor (index 0 or 2) should be present
        assert!(ids.contains(&chunks[1].id.as_str()));
    }

    #[test]
    fn map_reduce_batches_are_bounded_and_deterministic() {
        let c1 = make_test_chunk("d:c0", "f.txt", "Short content 1");
        let c2 = make_test_chunk("d:c1", "f.txt", "Short content 2");
        let c3 = make_test_chunk("d:c2", "f.txt", "Short content 3");
        let c4 = make_test_chunk("d:c3", "f.txt", "Short content 4");
        let chunks = vec![c1, c2, c3, c4];

        let batch_budget = 40;
        let batches = plan_map_reduce_batches(&chunks, batch_budget);

        assert!(batches.len() >= 2);
        let flattened: Vec<_> = batches.iter().flatten().map(|c| c.id.as_str()).collect();
        assert_eq!(flattened, vec!["d:c0", "d:c1", "d:c2", "d:c3"]);
    }

    #[test]
    fn prompt_assembly_enforces_five_layer_order_and_untrusted_delimiters() {
        let c1 = make_test_chunk("doc1:c0", "report.md", "Executive summary findings.");
        let chunks = vec![c1];

        let skills = vec![SkillModule::GroundedReading, SkillModule::Citation];
        let budget = ContextBudget::lite();

        let prompt = assemble_prompt(
            ContextStrategy::Direct,
            &skills,
            Some("Prefer concise bullets."),
            &chunks,
            "What does the summary state?",
            &budget,
        );

        // Verify layer 1: core policy
        assert!(
            prompt
                .system_prompt
                .contains("Dociler, a local-first document assistant")
        );
        // Verify layer 2: built-in skills
        assert!(prompt.system_prompt.contains("Grounded Document Reading"));
        assert!(prompt.system_prompt.contains("Source Citation Standards"));
        // Verify layer 3: client preferences
        assert!(prompt.system_prompt.contains("Prefer concise bullets."));

        // Verify layer 4: untrusted delimiters and source tags
        assert!(prompt.user_prompt.contains(UNTRUSTED_DOC_START));
        assert!(prompt.user_prompt.contains("[source: doc1:c0"));
        assert!(prompt.user_prompt.contains("Executive summary findings."));
        assert!(prompt.user_prompt.contains(UNTRUSTED_DOC_END));

        // Verify layer 5: user query
        assert!(prompt.user_prompt.ends_with("What does the summary state?"));

        // Verify active source IDs
        assert!(prompt.active_source_ids.contains("doc1:c0"));
        assert_eq!(prompt.included_chunks.len(), 1);
    }

    #[test]
    fn citation_validation_verifies_valid_and_marks_hallucinations() {
        let mut valid_sources = HashSet::new();
        valid_sources.insert("doc1:c0".to_string());
        valid_sources.insert("doc1:c1".to_string());

        let model_output = "The system achieved 95% accuracy [source: doc1:c0]. \
It also processed records in 10ms [source: invented_id], but revenue was up [source: doc1:c1, \"report.txt\"].";

        let result = validate_citations(model_output, &valid_sources);

        assert_eq!(result.total_citations, 3);
        assert_eq!(result.verified_citations, vec!["doc1:c0", "doc1:c1"]);
        assert_eq!(result.unverified_citations, vec!["invented_id"]);
        assert!(result.has_hallucinations());
        assert!((result.valid_ratio() - (2.0 / 3.0)).abs() < 1e-6);

        // Check rewritten text
        assert!(result.cleaned_text.contains("[source: doc1:c0]"));
        assert!(
            result
                .cleaned_text
                .contains("[source: doc1:c1, \"report.txt\"]")
        );
        assert!(
            result
                .cleaned_text
                .contains("[unverified source: invented_id]")
        );
        assert!(!result.cleaned_text.contains("[source: invented_id]"));
    }

    #[test]
    fn citation_validation_handles_zero_citations() {
        let valid_sources = HashSet::new();
        let text = "General greeting without citations.";
        let result = validate_citations(text, &valid_sources);

        assert_eq!(result.total_citations, 0);
        assert_eq!(result.valid_ratio(), 1.0);
        assert!(!result.has_hallucinations());
        assert_eq!(result.cleaned_text, text);
    }
}
