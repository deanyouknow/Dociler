//! Model quality evaluation gates, extraction benchmarks, and release criteria.
//!
//! Evaluates factual QA accuracy, citation validity rate, numeric/date preservation,
//! and process-group memory ceilings across Lite and Pro profiles according to
//! `IMPLEMENTATION_PLAN.md` §9 and `docs/testing-and-release.md`.

use std::collections::HashMap;

/// Stable model gate requirements.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelGateRequirements {
    /// Minimum factual QA accuracy ratio (0.0 to 1.0).
    pub min_factual_qa_accuracy: f64,
    /// Minimum valid citation rate ratio (0.0 to 1.0).
    pub min_valid_citation_rate: f64,
    /// Maximum count of invented numeric or date values allowed (must be 0).
    pub max_invented_numeric_or_date_count: usize,
    /// Maximum allowed process-group peak RSS in bytes.
    pub max_process_group_peak_rss_bytes: u64,
    /// Minimum required active context tokens in a sustained session.
    pub min_active_context_tokens: usize,
    /// Target host RAM in bytes.
    pub target_host_ram_bytes: u64,
}

impl ModelGateRequirements {
    /// Requirements for the `dociler-lite` profile:
    /// - At least 85% factual QA accuracy
    /// - At least 95% valid citations
    /// - Zero invented numeric/date values
    /// - Sustained real session at 8K context on an 8 GB host with peak RSS <= 5.5 GiB
    pub const fn lite() -> Self {
        Self {
            min_factual_qa_accuracy: 0.85,
            min_valid_citation_rate: 0.95,
            max_invented_numeric_or_date_count: 0,
            max_process_group_peak_rss_bytes: 5_905_580_032, // 5.5 GiB (5.5 * 1024^3)
            min_active_context_tokens: 8_192,
            target_host_ram_bytes: 8_589_934_592, // 8 GiB
        }
    }

    /// Requirements for the `dociler-pro` profile:
    /// - At least 92% factual QA accuracy
    /// - At least 97% valid citations
    /// - Zero invented numeric/date values
    /// - Sustained real session at 16K context on a 16 GB host with peak RSS <= 11.5 GiB
    pub const fn pro() -> Self {
        Self {
            min_factual_qa_accuracy: 0.92,
            min_valid_citation_rate: 0.97,
            max_invented_numeric_or_date_count: 0,
            max_process_group_peak_rss_bytes: 12_348_030_976, // 11.5 GiB (11.5 * 1024^3)
            min_active_context_tokens: 16_384,
            target_host_ram_bytes: 17_179_869_184, // 16 GiB
        }
    }
}

/// Observed metrics from a test, evaluation, or benchmark run.
#[derive(Debug, Clone, PartialEq)]
pub struct EvaluationMetrics {
    pub factual_qa_accuracy: f64,
    pub valid_citation_rate: f64,
    pub invented_numeric_count: usize,
    pub invented_date_count: usize,
    pub active_context_tokens: usize,
    pub peak_process_group_rss_bytes: u64,
    pub host_ram_bytes: u64,
}

impl EvaluationMetrics {
    /// Validates whether these observed metrics satisfy the given gate requirements.
    /// Returns `Ok(())` if all gates pass, or `Err(Vec<String>)` with failure reasons.
    pub fn evaluate_against(&self, reqs: &ModelGateRequirements) -> Result<(), Vec<String>> {
        let mut failures = Vec::new();

        if self.factual_qa_accuracy < reqs.min_factual_qa_accuracy {
            failures.push(format!(
                "Factual QA accuracy {:.1}% below threshold {:.1}%",
                self.factual_qa_accuracy * 100.0,
                reqs.min_factual_qa_accuracy * 100.0
            ));
        }

        if self.valid_citation_rate < reqs.min_valid_citation_rate {
            failures.push(format!(
                "Valid citation rate {:.1}% below threshold {:.1}%",
                self.valid_citation_rate * 100.0,
                reqs.min_valid_citation_rate * 100.0
            ));
        }

        let total_invented = self.invented_numeric_count + self.invented_date_count;
        if total_invented > reqs.max_invented_numeric_or_date_count {
            failures.push(format!(
                "Invented numeric/date count {} exceeds gate maximum {}",
                total_invented, reqs.max_invented_numeric_or_date_count
            ));
        }

        if self.peak_process_group_rss_bytes > reqs.max_process_group_peak_rss_bytes {
            failures.push(format!(
                "Process-group peak RSS {} bytes ({:.2} GiB) exceeds ceiling {} bytes ({:.2} GiB)",
                self.peak_process_group_rss_bytes,
                self.peak_process_group_rss_bytes as f64 / 1024.0 / 1024.0 / 1024.0,
                reqs.max_process_group_peak_rss_bytes,
                reqs.max_process_group_peak_rss_bytes as f64 / 1024.0 / 1024.0 / 1024.0,
            ));
        }

        if self.active_context_tokens < reqs.min_active_context_tokens {
            failures.push(format!(
                "Active context {} tokens below required {} tokens",
                self.active_context_tokens, reqs.min_active_context_tokens
            ));
        }

        if failures.is_empty() {
            Ok(())
        } else {
            Err(failures)
        }
    }
}

/// Type of factual item for deterministic extraction testing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FactType {
    Date,
    Currency,
    Number,
    EntityName,
    TableCell,
}

/// A target item to extract in a deterministic test case.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExpectedFact {
    pub fact_type: FactType,
    pub key: String,
    pub expected_value: String,
}

/// A test document and expected deterministic extractions.
#[derive(Debug, Clone)]
pub struct DeterministicCorpusItem {
    pub id: String,
    pub document_text: String,
    pub language: &'static str,
    pub expected_facts: Vec<ExpectedFact>,
}

/// Outcome of deterministic fact extraction against a document.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ExtractionResult {
    pub matched_facts: usize,
    pub missing_facts: usize,
    pub invented_values: usize,
    pub details: Vec<String>,
}

impl ExtractionResult {
    pub fn is_perfect(&self) -> bool {
        self.missing_facts == 0 && self.invented_values == 0
    }
}

/// Evaluates extracted key-value pairs against expected facts for a document.
pub fn evaluate_extraction(
    expected: &[ExpectedFact],
    extracted: &HashMap<String, String>,
) -> ExtractionResult {
    let mut result = ExtractionResult::default();

    for exp in expected {
        if let Some(actual) = extracted.get(&exp.key) {
            let actual_norm = actual.trim().to_ascii_lowercase();
            let exp_norm = exp.expected_value.trim().to_ascii_lowercase();
            if actual_norm == exp_norm || actual_norm.contains(&exp_norm) {
                result.matched_facts += 1;
            } else {
                result.missing_facts += 1;
                result.details.push(format!(
                    "Key '{}': expected '{}', got '{}'",
                    exp.key, exp.expected_value, actual
                ));
            }
        } else {
            result.missing_facts += 1;
            result.details.push(format!(
                "Missing key '{}' (expected '{}')",
                exp.key, exp.expected_value
            ));
        }
    }

    // Check for invented keys not present in expected
    let expected_keys: std::collections::HashSet<_> = expected.iter().map(|f| &f.key).collect();
    for (k, v) in extracted {
        if !expected_keys.contains(k) {
            result.invented_values += 1;
            result
                .details
                .push(format!("Invented key '{}' with value '{}'", k, v));
        }
    }

    result
}

/// Built-in reviewed corpus items for deterministic extraction testing
/// covering English and Indonesian documents, dates, currency, numbers, and proper nouns.
pub fn get_deterministic_extraction_corpus() -> Vec<DeterministicCorpusItem> {
    vec![
        DeterministicCorpusItem {
            id: "id_finance_2026".to_string(),
            language: "id",
            document_text: r#"# Laporan Keuangan Tahunan 2026
PT Teknologi Nusantara Jaya
Tanggal: 17 Agustus 1945
Kota: Jakarta Pusat

## Ringkasan Eksekutif
Pada kuartal pertama tahun 2026, total investasi modal tercatat sebesar Rp 150.000.000,00.
Proyek strategis dipimpin oleh Direktur Utama Budi Santoso dengan alokasi dana Rp 25.000.000,00.
Tingkat pertumbuhan tahunan mencapai 14,5% dengan 120 karyawan aktif."#.to_string(),
            expected_facts: vec![
                ExpectedFact {
                    fact_type: FactType::Date,
                    key: "tanggal".to_string(),
                    expected_value: "17 Agustus 1945".to_string(),
                },
                ExpectedFact {
                    fact_type: FactType::Currency,
                    key: "investasi_modal".to_string(),
                    expected_value: "Rp 150.000.000,00".to_string(),
                },
                ExpectedFact {
                    fact_type: FactType::EntityName,
                    key: "direktur_utama".to_string(),
                    expected_value: "Budi Santoso".to_string(),
                },
                ExpectedFact {
                    fact_type: FactType::Currency,
                    key: "alokasi_proyek".to_string(),
                    expected_value: "Rp 25.000.000,00".to_string(),
                },
                ExpectedFact {
                    fact_type: FactType::Number,
                    key: "karyawan_aktif".to_string(),
                    expected_value: "120".to_string(),
                },
            ],
        },
        DeterministicCorpusItem {
            id: "en_technical_spec_2026".to_string(),
            language: "en",
            document_text: r#"# Architecture Specification v2
Project: Dociler Local Core
Date: 2026-09-11
Author: Chief Architect Jane Doe

## Memory and Context Budgets
The Lite profile operates with a maximum context window of 8,192 tokens and a reserved completion budget of 1,024 tokens.
The Pro profile doubles this capacity to 16,384 tokens with 2,048 tokens reserved for completions.
The maximum hardware process-group peak RSS ceiling is set to 5.5 GiB for Lite and 11.5 GiB for Pro.
Total capital expenditure was $45,000.00."#.to_string(),
            expected_facts: vec![
                ExpectedFact {
                    fact_type: FactType::Date,
                    key: "date".to_string(),
                    expected_value: "2026-09-11".to_string(),
                },
                ExpectedFact {
                    fact_type: FactType::EntityName,
                    key: "author".to_string(),
                    expected_value: "Jane Doe".to_string(),
                },
                ExpectedFact {
                    fact_type: FactType::Number,
                    key: "lite_context_tokens".to_string(),
                    expected_value: "8,192".to_string(),
                },
                ExpectedFact {
                    fact_type: FactType::Number,
                    key: "pro_context_tokens".to_string(),
                    expected_value: "16,384".to_string(),
                },
                ExpectedFact {
                    fact_type: FactType::Currency,
                    key: "capital_expenditure".to_string(),
                    expected_value: "$45,000.00".to_string(),
                },
            ],
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_gate_requirements_lite_evaluation() {
        let reqs = ModelGateRequirements::lite();
        assert_eq!(reqs.min_factual_qa_accuracy, 0.85);
        assert_eq!(reqs.min_valid_citation_rate, 0.95);
        assert_eq!(reqs.max_invented_numeric_or_date_count, 0);

        // Passing metrics
        let passing = EvaluationMetrics {
            factual_qa_accuracy: 0.88,
            valid_citation_rate: 0.96,
            invented_numeric_count: 0,
            invented_date_count: 0,
            active_context_tokens: 8192,
            peak_process_group_rss_bytes: 4_500_000_000,
            host_ram_bytes: 8_589_934_592,
        };
        assert!(passing.evaluate_against(&reqs).is_ok());

        // Failing metrics: low accuracy
        let low_acc = EvaluationMetrics {
            factual_qa_accuracy: 0.80,
            ..passing.clone()
        };
        let errs = low_acc.evaluate_against(&reqs).unwrap_err();
        assert!(errs.iter().any(|e| e.contains("Factual QA accuracy")));

        // Failing metrics: invented value
        let invented = EvaluationMetrics {
            invented_date_count: 1,
            ..passing.clone()
        };
        let errs = invented.evaluate_against(&reqs).unwrap_err();
        assert!(
            errs.iter()
                .any(|e| e.contains("Invented numeric/date count"))
        );

        // Failing metrics: RSS ceiling breach
        let memory_breach = EvaluationMetrics {
            peak_process_group_rss_bytes: 6_000_000_000,
            ..passing
        };
        let errs = memory_breach.evaluate_against(&reqs).unwrap_err();
        assert!(errs.iter().any(|e| e.contains("peak RSS")));
    }

    #[test]
    fn test_gate_requirements_pro_evaluation() {
        let reqs = ModelGateRequirements::pro();
        assert_eq!(reqs.min_factual_qa_accuracy, 0.92);
        assert_eq!(reqs.min_valid_citation_rate, 0.97);

        let passing = EvaluationMetrics {
            factual_qa_accuracy: 0.94,
            valid_citation_rate: 0.98,
            invented_numeric_count: 0,
            invented_date_count: 0,
            active_context_tokens: 16384,
            peak_process_group_rss_bytes: 10_000_000_000,
            host_ram_bytes: 17_179_869_184,
        };
        assert!(passing.evaluate_against(&reqs).is_ok());

        // Low accuracy for pro (0.90 is ok for lite but fails pro)
        let low_for_pro = EvaluationMetrics {
            factual_qa_accuracy: 0.90,
            ..passing
        };
        assert!(low_for_pro.evaluate_against(&reqs).is_err());
    }

    #[test]
    fn test_deterministic_extraction_evaluation() {
        let corpus = get_deterministic_extraction_corpus();
        assert_eq!(corpus.len(), 2);

        let id_item = &corpus[0];
        assert_eq!(id_item.language, "id");

        let mut extracted = HashMap::new();
        extracted.insert("tanggal".to_string(), "17 Agustus 1945".to_string());
        extracted.insert(
            "investasi_modal".to_string(),
            "Rp 150.000.000,00".to_string(),
        );
        extracted.insert("direktur_utama".to_string(), "Budi Santoso".to_string());
        extracted.insert("alokasi_proyek".to_string(), "Rp 25.000.000,00".to_string());
        extracted.insert("karyawan_aktif".to_string(), "120".to_string());

        let res = evaluate_extraction(&id_item.expected_facts, &extracted);
        assert!(res.is_perfect());
        assert_eq!(res.matched_facts, 5);
        assert_eq!(res.missing_facts, 0);
        assert_eq!(res.invented_values, 0);

        // Test with missing and invented values
        extracted.remove("karyawan_aktif");
        extracted.insert("invented_field".to_string(), "some value".to_string());

        let imperfect = evaluate_extraction(&id_item.expected_facts, &extracted);
        assert!(!imperfect.is_perfect());
        assert_eq!(imperfect.matched_facts, 4);
        assert_eq!(imperfect.missing_facts, 1);
        assert_eq!(imperfect.invented_values, 1);
    }
}
