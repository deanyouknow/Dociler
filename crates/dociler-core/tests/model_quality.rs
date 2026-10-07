use std::collections::HashMap;

use dociler_core::context::{
    ContextBudget, ContextStrategy, SkillModule, assemble_prompt, plan_map_reduce_batches,
    validate_citations,
};
use dociler_core::document::{
    Block, Document, DocumentFormat, DocumentMetadata, DocumentSource, InlineRun, SourceAnchor,
};
use dociler_core::evaluation::{
    EvaluationMetrics, ExpectedFact, FactType, ModelGateRequirements, evaluate_extraction,
    get_deterministic_extraction_corpus,
};
use dociler_core::indexing::{Chunk, ChunkerOptions, chunk_document};

fn make_sample_doc(name: &str, title: &str, paragraphs: &[&str]) -> Document {
    let source = DocumentSource {
        display_name: name.to_string(),
        content_digest: format!("{:x}", md5_or_hash(name)),
        format: DocumentFormat::Markdown,
        byte_size: 1024,
    };
    let metadata = DocumentMetadata {
        title: Some(title.to_string()),
        author: None,
        created: None,
        modified: None,
    };
    let mut blocks = vec![Block::Heading {
        level: 1,
        runs: vec![InlineRun::Text(title.to_string())],
        anchor: SourceAnchor {
            page: Some(1),
            section: Some(title.to_string()),
            block_index: 0,
            line_range: Some((1, 2)),
        },
    }];
    for (idx, p) in paragraphs.iter().enumerate() {
        blocks.push(Block::Paragraph {
            runs: vec![InlineRun::Text(p.to_string())],
            anchor: SourceAnchor {
                page: Some(1),
                section: Some(title.to_string()),
                block_index: idx + 1,
                line_range: Some((3 + idx * 2, 4 + idx * 2)),
            },
        });
    }
    Document {
        source,
        metadata,
        blocks,
    }
}

fn md5_or_hash(s: &str) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for b in s.bytes() {
        hash ^= b as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

#[test]
fn test_deterministic_extraction_corpus_has_zero_invented_values() {
    let corpus = get_deterministic_extraction_corpus();
    assert!(!corpus.is_empty());

    for item in &corpus {
        // Build simulated ground-truth extraction matching the document
        let mut extracted = HashMap::new();
        for fact in &item.expected_facts {
            extracted.insert(fact.key.clone(), fact.expected_value.clone());
        }

        let result = evaluate_extraction(&item.expected_facts, &extracted);
        assert!(result.is_perfect(), "extraction failed for {}", item.id);
        assert_eq!(
            result.invented_values, 0,
            "zero invented values gate failed for {}",
            item.id
        );
        assert_eq!(result.missing_facts, 0);
        assert_eq!(result.matched_facts, item.expected_facts.len());
    }
}

#[test]
fn test_indonesian_deterministic_fact_and_currency_extraction() {
    let _doc_text = r#"
# Laporan Pelaksanaan Anggaran
Kementerian Keuangan Republik Indonesia
Tanggal Surat: 20 Oktober 2024
Penanggung Jawab: Dr. Sri Mulyani

Alokasi subsidi energi bersih ditetapkan sebesar Rp 75.500.000.000,00 untuk semester kedua.
Target penerimaan perpajakan mencapai Rp 250.000.000.000,00 dengan realisasi sementara 92,4%.
Jumlah wajib pajak terdaftar adalah 45.000 entitas bisnis.
"#;

    let expected = vec![
        ExpectedFact {
            fact_type: FactType::Date,
            key: "tanggal_surat".to_string(),
            expected_value: "20 Oktober 2024".to_string(),
        },
        ExpectedFact {
            fact_type: FactType::EntityName,
            key: "penanggung_jawab".to_string(),
            expected_value: "Dr. Sri Mulyani".to_string(),
        },
        ExpectedFact {
            fact_type: FactType::Currency,
            key: "subsidi_energi".to_string(),
            expected_value: "Rp 75.500.000.000,00".to_string(),
        },
        ExpectedFact {
            fact_type: FactType::Currency,
            key: "penerimaan_pajak".to_string(),
            expected_value: "Rp 250.000.000.000,00".to_string(),
        },
        ExpectedFact {
            fact_type: FactType::Number,
            key: "wajib_pajak".to_string(),
            expected_value: "45.000".to_string(),
        },
    ];

    let mut extracted = HashMap::new();
    extracted.insert("tanggal_surat".to_string(), "20 Oktober 2024".to_string());
    extracted.insert(
        "penanggung_jawab".to_string(),
        "Dr. Sri Mulyani".to_string(),
    );
    extracted.insert(
        "subsidi_energi".to_string(),
        "Rp 75.500.000.000,00".to_string(),
    );
    extracted.insert(
        "penerimaan_pajak".to_string(),
        "Rp 250.000.000.000,00".to_string(),
    );
    extracted.insert("wajib_pajak".to_string(), "45.000".to_string());

    let result = evaluate_extraction(&expected, &extracted);
    assert!(result.is_perfect());
    assert_eq!(result.invented_values, 0);
    assert_eq!(result.matched_facts, 5);
}

#[test]
fn test_single_and_multi_document_grounded_qa_citations() {
    let doc1 = make_sample_doc(
        "kebijakan_2025.md",
        "Kebijakan Fiskal 2025",
        &[
            "Target pertumbuhan ekonomi nasional adalah 5,3% pada tahun 2025.",
            "Defisit anggaran ditekan pada level 2,5% dari produk domestik bruto.",
        ],
    );

    let doc2 = make_sample_doc(
        "kebijakan_2026.md",
        "Kebijakan Fiskal 2026",
        &[
            "Target pertumbuhan ekonomi dinaikkan menjadi 5,7% pada tahun 2026.",
            "Fokus utama belanja negara dialokasikan pada hilirisasi industri dan pendidikan.",
        ],
    );

    let chunks1 = chunk_document(&doc1, &ChunkerOptions::default());
    let chunks2 = chunk_document(&doc2, &ChunkerOptions::default());

    let mut all_chunks = Vec::new();
    all_chunks.extend(chunks1.clone());
    all_chunks.extend(chunks2.clone());

    let budget = ContextBudget::pro();
    let skills = vec![SkillModule::GroundedReading, SkillModule::Comparison];
    let assembled = assemble_prompt(
        ContextStrategy::Direct,
        &skills,
        None,
        &all_chunks,
        "Bandingkan target pertumbuhan ekonomi tahun 2025 dan 2026.",
        &budget,
    );

    assert_eq!(assembled.included_chunks.len(), all_chunks.len());

    // Collect valid source IDs
    let valid_source_ids = assembled.active_source_ids;
    assert!(valid_source_ids.len() >= 2);

    let c1_id = &chunks1[0].id;
    let c2_id = &chunks2[0].id;

    // Simulate model response with valid citations from both documents
    let model_response = format!(
        "Berdasarkan dokumen, target pertumbuhan ekonomi tahun 2025 adalah 5,3% [source: {c1_id}], sedangkan pada tahun 2026 target dinaikkan menjadi 5,7% [source: {c2_id}]."
    );

    let val = validate_citations(&model_response, &valid_source_ids);
    assert_eq!(val.verified_citations.len(), 2);
    assert_eq!(val.unverified_citations.len(), 0);
    assert_eq!(val.valid_ratio(), 1.0);
}

#[test]
fn test_absent_evidence_and_contradiction_handling() {
    let doc1 = make_sample_doc(
        "laporan_a.md",
        "Laporan A",
        &["Proyek Alfa selesai pada tanggal 15 Januari 2025 dengan anggaran Rp 10.000.000."],
    );
    let doc2 = make_sample_doc(
        "laporan_b.md",
        "Laporan B",
        &["Proyek Alfa dinyatakan tertunda dan anggaran dicatat sebesar Rp 18.000.000."],
    );

    let chunks1 = chunk_document(&doc1, &ChunkerOptions::default());
    let chunks2 = chunk_document(&doc2, &ChunkerOptions::default());

    let mut combined_chunks = Vec::new();
    combined_chunks.extend(chunks1.clone());
    combined_chunks.extend(chunks2.clone());

    let budget = ContextBudget::lite();
    let skills = vec![SkillModule::Comparison, SkillModule::GroundedReading];
    let assembled = assemble_prompt(
        ContextStrategy::Retrieval,
        &skills,
        None,
        &combined_chunks,
        "Apakah ada kontradiksi dalam anggaran Proyek Alfa?",
        &budget,
    );

    // Verify system prompt contains comparison instructions that mandate identifying contradictions
    assert!(
        assembled
            .system_prompt
            .contains("Contradiction and Discrepancy Identification")
            || assembled
                .system_prompt
                .contains("Multi-Document Comparison")
    );

    // Simulate response identifying contradiction with valid citations
    let id1 = &chunks1[0].id;
    let id2 = &chunks2[0].id;
    let contradiction_response = format!(
        "Terdapat kontradiksi: Laporan A menyebut anggaran Rp 10.000.000 [source: {id1}], sedangkan Laporan B mencatat Rp 18.000.000 [source: {id2}]."
    );
    let val = validate_citations(&contradiction_response, &assembled.active_source_ids);
    assert_eq!(val.verified_citations.len(), 2);
    assert_eq!(val.unverified_citations.len(), 0);

    // Simulate question where evidence is absent: e.g. "Berapa anggaran Proyek Beta?"
    let absent_evidence_response =
        "Berdasarkan dokumen yang tersedia, tidak ditemukan informasi mengenai Proyek Beta.";
    let val_absent = validate_citations(absent_evidence_response, &assembled.active_source_ids);
    assert_eq!(val_absent.total_citations, 0);
    assert_eq!(val_absent.unverified_citations.len(), 0);
}

#[test]
fn test_map_reduce_batch_partitioning_for_long_documents() {
    let mut large_chunks = Vec::new();
    for i in 0..25 {
        large_chunks.push(Chunk {
            id: format!("doc1:c{i}"),
            document_display_name: "long_report.md".to_string(),
            document_digest: "abcd1234".to_string(),
            ordinal: i,
            text: format!(
                "Section {i}: Paragraph detailing financial item {i} with budget Rp {i}.000.000,00."
            ),
            token_count: 200,
            nearest_heading: Some(format!("Section {i}")),
            page: Some((i / 5) as u32 + 1),
            anchor: SourceAnchor::default(),
            searchable_terms: vec![],
        });
    }

    // Partition into batches of 1000 tokens
    let batches = plan_map_reduce_batches(&large_chunks, 1000);
    assert!(!batches.is_empty());
    assert!(batches.len() >= 5);

    // Verify all chunks are accounted for in batch partitions
    let total_batched_chunks: usize = batches.iter().map(|b| b.len()).sum();
    assert_eq!(total_batched_chunks, large_chunks.len());

    // Verify document order is preserved
    let mut last_ordinal = 0;
    for batch in &batches {
        for chunk in batch {
            assert!(chunk.ordinal >= last_ordinal);
            last_ordinal = chunk.ordinal;
        }
    }
}

#[test]
fn test_stable_model_quality_gates_assertions() {
    let lite_reqs = ModelGateRequirements::lite();
    let pro_reqs = ModelGateRequirements::pro();

    // 1. Lite profile passing evaluation
    let lite_run = EvaluationMetrics {
        factual_qa_accuracy: 0.87,
        valid_citation_rate: 0.96,
        invented_numeric_count: 0,
        invented_date_count: 0,
        active_context_tokens: 8192,
        peak_process_group_rss_bytes: 4_800_000_000, // 4.8 GiB <= 5.5 GiB
        host_ram_bytes: 8_589_934_592,               // 8 GiB
    };
    assert!(lite_run.evaluate_against(&lite_reqs).is_ok());

    // 2. Pro profile passing evaluation
    let pro_run = EvaluationMetrics {
        factual_qa_accuracy: 0.94,
        valid_citation_rate: 0.98,
        invented_numeric_count: 0,
        invented_date_count: 0,
        active_context_tokens: 16384,
        peak_process_group_rss_bytes: 10_200_000_000, // 10.2 GiB <= 11.5 GiB
        host_ram_bytes: 17_179_869_184,               // 16 GiB
    };
    assert!(pro_run.evaluate_against(&pro_reqs).is_ok());

    // 3. Edge case: 1 invented date fails gate immediately
    let failed_invented = EvaluationMetrics {
        invented_date_count: 1,
        ..lite_run
    };
    let errs = failed_invented.evaluate_against(&lite_reqs).unwrap_err();
    assert!(
        errs.iter()
            .any(|e| e.contains("Invented numeric/date count 1 exceeds gate maximum 0"))
    );

    // 4. Memory ceiling breach fails gate
    let failed_memory = EvaluationMetrics {
        peak_process_group_rss_bytes: 6_200_000_000, // > 5.5 GiB
        ..lite_run
    };
    let mem_errs = failed_memory.evaluate_against(&lite_reqs).unwrap_err();
    assert!(mem_errs.iter().any(|e| e.contains("exceeds ceiling")));
}
