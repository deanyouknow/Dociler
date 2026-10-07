//! In-memory semantic chunking and BM25 search index.
//!
//! Chunks canonical document ASTs at semantic block boundaries, carrying
//! nearest headings and page anchors, splitting oversized paragraphs with
//! token overlap and oversized tables with repeated headers.
//! Provides an in-memory BM25 index with stable session-local IDs and
//! file-constrained retrieval.

use std::collections::HashMap;

use crate::document::{Block, Document, SourceAnchor};

const DEFAULT_K1: f64 = 1.2;
const DEFAULT_B: f64 = 0.75;
const DEFAULT_MAX_CHUNK_TOKENS: usize = 512;
const DEFAULT_OVERLAP_TOKENS: usize = 64;

/// Configuration options for the semantic document chunker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChunkerOptions {
    pub max_chunk_tokens: usize,
    pub overlap_tokens: usize,
}

impl Default for ChunkerOptions {
    fn default() -> Self {
        Self {
            max_chunk_tokens: DEFAULT_MAX_CHUNK_TOKENS,
            overlap_tokens: DEFAULT_OVERLAP_TOKENS,
        }
    }
}

/// A semantic chunk extracted from a canonical document for retrieval and context.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Chunk {
    pub id: String,
    pub document_digest: String,
    pub document_display_name: String,
    pub ordinal: usize,
    pub nearest_heading: Option<String>,
    pub page: Option<u32>,
    pub anchor: SourceAnchor,
    pub token_count: usize,
    pub text: String,
    pub searchable_terms: Vec<String>,
}

impl Chunk {
    /// Formatted citation header for prompt injection (e.g. `[source: 1a2b3c4d:c0, "spec.docx", Page 1]`).
    pub fn citation_label(&self) -> String {
        let mut parts = vec![
            format!("source: {}", self.id),
            format!("\"{}\"", self.document_display_name),
        ];
        if let Some(h) = &self.nearest_heading {
            parts.push(format!("§ {h}"));
        }
        if let Some(p) = self.page {
            parts.push(format!("Page {p}"));
        }
        format!("[{}]", parts.join(", "))
    }
}

/// Chunks a canonical document into semantic chunks according to block boundaries.
pub fn chunk_document(document: &Document, options: &ChunkerOptions) -> Vec<Chunk> {
    let mut chunks = Vec::new();
    let mut ordinal = 0;
    let mut current_heading: Option<String> = None;
    let mut current_page: Option<u32> = None;

    let digest_prefix = if document.source.content_digest.len() >= 8 {
        &document.source.content_digest[..8]
    } else {
        &document.source.content_digest
    };

    for block in &document.blocks {
        if let Some(anchor) = block.anchor() {
            if let Some(p) = anchor.page {
                current_page = Some(p);
            }
        }

        match block {
            Block::PageBreak { page } => {
                current_page = Some(*page);
            }
            Block::Heading {
                level: _,
                runs,
                anchor,
            } => {
                let title = runs.iter().map(|r| r.text()).collect::<Vec<_>>().join("");
                current_heading = Some(title.clone());

                let chunk_text = format!("# {title}");
                let tokens = approx_token_count(&chunk_text);
                let terms = tokenize_terms(&chunk_text);

                let id = format!("{digest_prefix}:c{ordinal}");
                chunks.push(Chunk {
                    id,
                    document_digest: document.source.content_digest.clone(),
                    document_display_name: document.source.display_name.clone(),
                    ordinal,
                    nearest_heading: current_heading.clone(),
                    page: anchor.page.or(current_page),
                    anchor: anchor.clone(),
                    token_count: tokens,
                    text: chunk_text,
                    searchable_terms: terms,
                });
                ordinal += 1;
            }
            Block::Paragraph { runs, anchor } => {
                let p_text = runs.iter().map(|r| r.text()).collect::<Vec<_>>().join("");
                let token_count = approx_token_count(&p_text);

                if token_count <= options.max_chunk_tokens {
                    let terms = tokenize_terms(&p_text);
                    let id = format!("{digest_prefix}:c{ordinal}");
                    chunks.push(Chunk {
                        id,
                        document_digest: document.source.content_digest.clone(),
                        document_display_name: document.source.display_name.clone(),
                        ordinal,
                        nearest_heading: current_heading.clone(),
                        page: anchor.page.or(current_page),
                        anchor: anchor.clone(),
                        token_count,
                        text: p_text,
                        searchable_terms: terms,
                    });
                    ordinal += 1;
                } else {
                    // Split oversized paragraph with token overlap
                    let sub_chunks = split_text_with_overlap(
                        &p_text,
                        options.max_chunk_tokens,
                        options.overlap_tokens,
                    );
                    for sub in sub_chunks {
                        let terms = tokenize_terms(&sub);
                        let sub_tokens = approx_token_count(&sub);
                        let id = format!("{digest_prefix}:c{ordinal}");
                        chunks.push(Chunk {
                            id,
                            document_digest: document.source.content_digest.clone(),
                            document_display_name: document.source.display_name.clone(),
                            ordinal,
                            nearest_heading: current_heading.clone(),
                            page: anchor.page.or(current_page),
                            anchor: anchor.clone(),
                            token_count: sub_tokens,
                            text: sub,
                            searchable_terms: terms,
                        });
                        ordinal += 1;
                    }
                }
            }
            Block::Table {
                headers,
                rows,
                anchor,
            } => {
                let full_table_text = block.plain_text();
                let table_tokens = approx_token_count(&full_table_text);

                if table_tokens <= options.max_chunk_tokens {
                    let terms = tokenize_terms(&full_table_text);
                    let id = format!("{digest_prefix}:c{ordinal}");
                    chunks.push(Chunk {
                        id,
                        document_digest: document.source.content_digest.clone(),
                        document_display_name: document.source.display_name.clone(),
                        ordinal,
                        nearest_heading: current_heading.clone(),
                        page: anchor.page.or(current_page),
                        anchor: anchor.clone(),
                        token_count: table_tokens,
                        text: full_table_text,
                        searchable_terms: terms,
                    });
                    ordinal += 1;
                } else {
                    // Split oversized table by rows while repeating headers
                    let header_line = headers.join(" | ");
                    let header_tokens = approx_token_count(&header_line);
                    let max_row_tokens = options
                        .max_chunk_tokens
                        .saturating_sub(header_tokens)
                        .max(1);

                    let mut current_table_rows: Vec<String> = Vec::new();
                    let mut current_row_tokens = 0;

                    for row in rows {
                        let row_line = row.join(" | ");
                        let r_tokens = approx_token_count(&row_line);

                        if current_row_tokens + r_tokens > max_row_tokens
                            && !current_table_rows.is_empty()
                        {
                            let mut chunk_text = header_line.clone();
                            chunk_text.push('\n');
                            for r in &current_table_rows {
                                chunk_text.push_str(r);
                                chunk_text.push('\n');
                            }
                            let terms = tokenize_terms(&chunk_text);
                            let c_tokens = approx_token_count(&chunk_text);
                            let id = format!("{digest_prefix}:c{ordinal}");
                            chunks.push(Chunk {
                                id,
                                document_digest: document.source.content_digest.clone(),
                                document_display_name: document.source.display_name.clone(),
                                ordinal,
                                nearest_heading: current_heading.clone(),
                                page: anchor.page.or(current_page),
                                anchor: anchor.clone(),
                                token_count: c_tokens,
                                text: chunk_text.trim_end().to_string(),
                                searchable_terms: terms,
                            });
                            ordinal += 1;
                            current_table_rows.clear();
                            current_row_tokens = 0;
                        }

                        current_table_rows.push(row_line);
                        current_row_tokens += r_tokens;
                    }

                    if !current_table_rows.is_empty() {
                        let mut chunk_text = header_line.clone();
                        chunk_text.push('\n');
                        for r in &current_table_rows {
                            chunk_text.push_str(r);
                            chunk_text.push('\n');
                        }
                        let terms = tokenize_terms(&chunk_text);
                        let c_tokens = approx_token_count(&chunk_text);
                        let id = format!("{digest_prefix}:c{ordinal}");
                        chunks.push(Chunk {
                            id,
                            document_digest: document.source.content_digest.clone(),
                            document_display_name: document.source.display_name.clone(),
                            ordinal,
                            nearest_heading: current_heading.clone(),
                            page: anchor.page.or(current_page),
                            anchor: anchor.clone(),
                            token_count: c_tokens,
                            text: chunk_text.trim_end().to_string(),
                            searchable_terms: terms,
                        });
                        ordinal += 1;
                    }
                }
            }
            Block::List {
                ordered: _,
                items: _,
                anchor,
            } => {
                let list_text = block.plain_text();
                let terms = tokenize_terms(&list_text);
                let tokens = approx_token_count(&list_text);
                let id = format!("{digest_prefix}:c{ordinal}");
                chunks.push(Chunk {
                    id,
                    document_digest: document.source.content_digest.clone(),
                    document_display_name: document.source.display_name.clone(),
                    ordinal,
                    nearest_heading: current_heading.clone(),
                    page: anchor.page.or(current_page),
                    anchor: anchor.clone(),
                    token_count: tokens,
                    text: list_text,
                    searchable_terms: terms,
                });
                ordinal += 1;
            }
            Block::Code {
                language,
                text,
                anchor,
            } => {
                let mut code_text = String::new();
                if let Some(lang) = language {
                    code_text.push_str(&format!("```{lang}\n"));
                } else {
                    code_text.push_str("```\n");
                }
                code_text.push_str(text);
                code_text.push_str("\n```");

                let terms = tokenize_terms(text);
                let tokens = approx_token_count(&code_text);
                let id = format!("{digest_prefix}:c{ordinal}");
                chunks.push(Chunk {
                    id,
                    document_digest: document.source.content_digest.clone(),
                    document_display_name: document.source.display_name.clone(),
                    ordinal,
                    nearest_heading: current_heading.clone(),
                    page: anchor.page.or(current_page),
                    anchor: anchor.clone(),
                    token_count: tokens,
                    text: code_text,
                    searchable_terms: terms,
                });
                ordinal += 1;
            }
            Block::Unsupported {
                description: _,
                anchor: _,
            } => {}
        }
    }

    chunks
}

/// Splits a long text into chunks of at most `max_tokens` with `overlap_tokens` boundary overlap.
fn split_text_with_overlap(text: &str, max_tokens: usize, overlap_tokens: usize) -> Vec<String> {
    let words: Vec<&str> = text.split_whitespace().collect();
    if words.is_empty() {
        return Vec::new();
    }

    // Heuristic: 1 token ~ 0.75 words (or 1 word ~ 1.33 tokens)
    let words_per_chunk = (max_tokens * 3 / 4).max(1);
    let overlap_words = (overlap_tokens * 3 / 4).min(words_per_chunk / 2);

    let mut chunks = Vec::new();
    let mut start = 0;

    while start < words.len() {
        let end = (start + words_per_chunk).min(words.len());
        let chunk_words = &words[start..end];
        chunks.push(chunk_words.join(" "));

        if end == words.len() {
            break;
        }
        start = end.saturating_sub(overlap_words);
    }

    chunks
}

/// Conservative token count approximation (1 token per 4 characters).
pub fn approx_token_count(text: &str) -> usize {
    text.chars().count().div_ceil(4).max(1)
}

/// Normalizes and tokenizes text into lowercased search terms.
pub fn tokenize_terms(text: &str) -> Vec<String> {
    let mut terms = Vec::new();
    for token in text.split(|c: char| !c.is_alphanumeric()) {
        let trimmed = token.trim().to_ascii_lowercase();
        if trimmed.len() >= 2 {
            terms.push(trimmed);
        }
    }
    terms
}

#[derive(Debug, Clone, Copy)]
struct Posting {
    chunk_idx: usize,
    term_frequency: u32,
}

/// Search result returned by BM25 query execution.
#[derive(Debug, Clone, PartialEq)]
pub struct SearchResult<'a> {
    pub chunk: &'a Chunk,
    pub score: f64,
}

/// In-memory BM25 search index for document chunks.
#[derive(Debug, Clone)]
pub struct Bm25Index {
    k1: f64,
    b: f64,
    chunks: Vec<Chunk>,
    chunk_lengths: Vec<usize>,
    inverted_index: HashMap<String, Vec<Posting>>,
    doc_frequencies: HashMap<String, usize>,
    total_tokens: usize,
}

impl Default for Bm25Index {
    fn default() -> Self {
        Self::new(DEFAULT_K1, DEFAULT_B)
    }
}

impl Bm25Index {
    pub fn new(k1: f64, b: f64) -> Self {
        Self {
            k1,
            b,
            chunks: Vec::new(),
            chunk_lengths: Vec::new(),
            inverted_index: HashMap::new(),
            doc_frequencies: HashMap::new(),
            total_tokens: 0,
        }
    }

    /// Clears all chunks and postings from the index.
    pub fn clear(&mut self) {
        self.chunks.clear();
        self.chunk_lengths.clear();
        self.inverted_index.clear();
        self.doc_frequencies.clear();
        self.total_tokens = 0;
    }

    /// Returns the total number of indexed chunks.
    pub fn total_chunks(&self) -> usize {
        self.chunks.len()
    }

    /// Access all indexed chunks.
    pub fn chunks(&self) -> &[Chunk] {
        &self.chunks
    }

    /// Adds all semantic chunks from a document into the index.
    pub fn add_document(&mut self, document: &Document, options: &ChunkerOptions) {
        let chunks = chunk_document(document, options);
        for chunk in chunks {
            self.add_chunk(chunk);
        }
    }

    /// Adds multiple chunks into the index.
    pub fn add_chunks(&mut self, chunks: &[Chunk]) {
        for chunk in chunks {
            self.add_chunk(chunk.clone());
        }
    }

    /// Convenient search helper without an explicit document filter.
    pub fn search_simple(&self, query: &str, limit: usize) -> Vec<SearchResult<'_>> {
        self.search(query, limit, None)
    }

    /// Adds an individual chunk to the index.
    pub fn add_chunk(&mut self, chunk: Chunk) {
        let chunk_idx = self.chunks.len();
        let term_count = chunk.searchable_terms.len();
        self.chunk_lengths.push(term_count);
        self.total_tokens += term_count;

        let mut term_freqs: HashMap<String, u32> = HashMap::new();
        for term in &chunk.searchable_terms {
            *term_freqs.entry(term.clone()).or_insert(0) += 1;
        }

        for (term, freq) in term_freqs {
            self.inverted_index
                .entry(term.clone())
                .or_default()
                .push(Posting {
                    chunk_idx,
                    term_frequency: freq,
                });
            *self.doc_frequencies.entry(term).or_insert(0) += 1;
        }

        self.chunks.push(chunk);
    }

    /// Retrieves top-matching chunks for a query string, optionally constrained by `@filename` or digest.
    pub fn search(
        &self,
        query: &str,
        limit: usize,
        filter_document: Option<&str>,
    ) -> Vec<SearchResult<'_>> {
        if self.chunks.is_empty() || limit == 0 {
            return Vec::new();
        }

        let avg_dl = if self.chunks.is_empty() {
            1.0
        } else {
            self.total_tokens as f64 / self.chunks.len() as f64
        };

        // Extract possible @file filter from query string if not explicitly given
        let (extracted_filter, clean_query) = extract_file_filter(query);
        let active_filter = filter_document.or(extracted_filter.as_deref());

        let query_terms = tokenize_terms(&clean_query);
        if query_terms.is_empty() {
            return Vec::new();
        }

        let n_docs = self.chunks.len() as f64;
        let mut scores: HashMap<usize, f64> = HashMap::new();

        for term in &query_terms {
            if let Some(postings) = self.inverted_index.get(term) {
                let n_term = *self.doc_frequencies.get(term).unwrap_or(&0) as f64;
                // Standard Robertson-Sparck Jones IDF formula with add-1 smoothing
                let idf = ((n_docs - n_term + 0.5) / (n_term + 0.5) + 1.0).ln();

                for posting in postings {
                    let chunk = &self.chunks[posting.chunk_idx];

                    // Check file filter
                    if let Some(filter) = active_filter {
                        if !chunk.document_display_name.eq_ignore_ascii_case(filter)
                            && !chunk.document_digest.starts_with(filter)
                        {
                            continue;
                        }
                    }

                    let tf = posting.term_frequency as f64;
                    let doc_len = self.chunk_lengths[posting.chunk_idx] as f64;

                    let numerator = tf * (self.k1 + 1.0);
                    let denominator = tf + self.k1 * (1.0 - self.b + self.b * (doc_len / avg_dl));
                    let term_score = idf * (numerator / denominator);

                    *scores.entry(posting.chunk_idx).or_insert(0.0) += term_score;
                }
            }
        }

        let mut ranked: Vec<SearchResult<'_>> = scores
            .into_iter()
            .map(|(idx, score)| SearchResult {
                chunk: &self.chunks[idx],
                score,
            })
            .collect();

        // Sort descending by score, tiebreak by chunk ordinal
        ranked.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.chunk.ordinal.cmp(&b.chunk.ordinal))
        });

        ranked.truncate(limit);
        ranked
    }

    /// Returns contiguous adjacent chunks for context window expansion.
    pub fn get_adjacent_chunks(&self, chunk_idx: usize, window: usize) -> Vec<&Chunk> {
        if chunk_idx >= self.chunks.len() {
            return Vec::new();
        }
        let target = &self.chunks[chunk_idx];
        let start = chunk_idx.saturating_sub(window);
        let end = (chunk_idx + window + 1).min(self.chunks.len());

        let mut out = Vec::new();
        for i in start..end {
            // Only include adjacent chunks from the exact same document
            if self.chunks[i].document_digest == target.document_digest {
                out.push(&self.chunks[i]);
            }
        }
        out
    }
}

/// Parses out `@filename` or `@file` syntax from user query string.
fn extract_file_filter(query: &str) -> (Option<String>, String) {
    let mut filter = None;
    let mut clean_words = Vec::new();

    for word in query.split_whitespace() {
        if let Some(target) = word.strip_prefix('@') {
            if !target.is_empty() && filter.is_none() {
                filter = Some(target.to_string());
                continue;
            }
        }
        clean_words.push(word);
    }

    (filter, clean_words.join(" "))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::{DocumentFormat, DocumentMetadata, DocumentSource, InlineRun};

    fn make_test_doc(name: &str, content: &[Block]) -> Document {
        Document::new(
            DocumentSource::from_bytes(name, DocumentFormat::PlainText, b"fake content"),
            DocumentMetadata::default(),
            content.to_vec(),
        )
    }

    #[test]
    fn chunking_preserves_headings_and_page_provenance() {
        let blocks = vec![
            Block::Heading {
                level: 1,
                runs: vec![InlineRun::Text("Introduction".to_string())],
                anchor: SourceAnchor {
                    page: Some(1),
                    section: None,
                    block_index: 0,
                    line_range: None,
                },
            },
            Block::Paragraph {
                runs: vec![InlineRun::Text(
                    "Dociler is a local-first assistant.".to_string(),
                )],
                anchor: SourceAnchor {
                    page: Some(1),
                    section: None,
                    block_index: 1,
                    line_range: None,
                },
            },
            Block::PageBreak { page: 2 },
            Block::Paragraph {
                runs: vec![InlineRun::Text(
                    "Privacy and security are mandatory.".to_string(),
                )],
                anchor: SourceAnchor {
                    page: Some(2),
                    section: None,
                    block_index: 2,
                    line_range: None,
                },
            },
        ];

        let doc = make_test_doc("guide.txt", &blocks);
        let options = ChunkerOptions::default();
        let chunks = chunk_document(&doc, &options);

        assert_eq!(chunks.len(), 3);
        assert_eq!(chunks[0].nearest_heading.as_deref(), Some("Introduction"));
        assert_eq!(chunks[0].page, Some(1));

        assert_eq!(chunks[1].nearest_heading.as_deref(), Some("Introduction"));
        assert_eq!(chunks[1].page, Some(1));
        assert!(chunks[1].text.contains("local-first"));

        assert_eq!(chunks[2].nearest_heading.as_deref(), Some("Introduction"));
        assert_eq!(chunks[2].page, Some(2));
        assert!(chunks[2].text.contains("Privacy and security"));

        assert!(chunks[1].citation_label().contains("source:"));
        assert!(chunks[1].citation_label().contains("guide.txt"));
        assert!(chunks[1].citation_label().contains("§ Introduction"));
    }

    #[test]
    fn oversized_table_splits_and_repeats_headers() {
        let headers = vec![
            "ID".to_string(),
            "Name".to_string(),
            "Description".to_string(),
        ];
        let rows = vec![
            vec![
                "1".to_string(),
                "Alpha".to_string(),
                "First record with detailed notes".to_string(),
            ],
            vec![
                "2".to_string(),
                "Beta".to_string(),
                "Second record with detailed notes".to_string(),
            ],
            vec![
                "3".to_string(),
                "Gamma".to_string(),
                "Third record with detailed notes".to_string(),
            ],
        ];

        let blocks = vec![Block::Table {
            headers,
            rows,
            anchor: SourceAnchor::default(),
        }];

        let doc = make_test_doc("data.txt", &blocks);
        // Set very small chunk token budget to force row splitting
        let options = ChunkerOptions {
            max_chunk_tokens: 15,
            overlap_tokens: 2,
        };
        let chunks = chunk_document(&doc, &options);

        assert!(chunks.len() >= 2);
        for chunk in &chunks {
            // Every chunk of the split table must repeat the headers
            assert!(chunk.text.starts_with("ID | Name | Description"));
        }
    }

    #[test]
    fn bm25_search_scoring_and_ranking() {
        let doc1 = make_test_doc(
            "rust.txt",
            &[Block::Paragraph {
                runs: vec![InlineRun::Text("Rust provides fearless concurrency and memory safety without garbage collection.".to_string())],
                anchor: SourceAnchor::default(),
            }],
        );

        let doc2 = make_test_doc(
            "python.txt",
            &[Block::Paragraph {
                runs: vec![InlineRun::Text(
                    "Python is an interpreted dynamic programming language with rich ecosystem."
                        .to_string(),
                )],
                anchor: SourceAnchor::default(),
            }],
        );

        let mut index = Bm25Index::default();
        let options = ChunkerOptions::default();
        index.add_document(&doc1, &options);
        index.add_document(&doc2, &options);

        assert_eq!(index.total_chunks(), 2);

        // Query matching doc1
        let results = index.search("concurrency memory safety", 5, None);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].chunk.document_display_name, "rust.txt");
        assert!(results[0].score > 0.0);

        // Query matching doc2
        let py_results = index.search("dynamic language", 5, None);
        assert_eq!(py_results.len(), 1);
        assert_eq!(py_results[0].chunk.document_display_name, "python.txt");
    }

    #[test]
    fn bm25_search_with_at_file_filter() {
        let doc1 = make_test_doc(
            "report.txt",
            &[Block::Paragraph {
                runs: vec![InlineRun::Text(
                    "Financial quarterly revenue and operating profit.".to_string(),
                )],
                anchor: SourceAnchor::default(),
            }],
        );

        let doc2 = make_test_doc(
            "summary.txt",
            &[Block::Paragraph {
                runs: vec![InlineRun::Text(
                    "Financial executive summary and revenue projections.".to_string(),
                )],
                anchor: SourceAnchor::default(),
            }],
        );

        let mut index = Bm25Index::default();
        let options = ChunkerOptions::default();
        index.add_document(&doc1, &options);
        index.add_document(&doc2, &options);

        // Search with explicit @file filter in query string
        let filtered = index.search("revenue @report.txt", 5, None);
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].chunk.document_display_name, "report.txt");

        // Search without filter returns both
        let both = index.search("revenue", 5, None);
        assert_eq!(both.len(), 2);
    }

    #[test]
    fn adjacent_chunks_context_continuity() {
        let blocks = vec![
            Block::Paragraph {
                runs: vec![InlineRun::Text("Chunk zero".to_string())],
                anchor: SourceAnchor::default(),
            },
            Block::Paragraph {
                runs: vec![InlineRun::Text("Chunk one".to_string())],
                anchor: SourceAnchor::default(),
            },
            Block::Paragraph {
                runs: vec![InlineRun::Text("Chunk two".to_string())],
                anchor: SourceAnchor::default(),
            },
        ];

        let doc = make_test_doc("multi.txt", &blocks);
        let mut index = Bm25Index::default();
        index.add_document(&doc, &ChunkerOptions::default());

        let adj = index.get_adjacent_chunks(1, 1);
        assert_eq!(adj.len(), 3);
        assert_eq!(adj[0].ordinal, 0);
        assert_eq!(adj[1].ordinal, 1);
        assert_eq!(adj[2].ordinal, 2);
    }
}
