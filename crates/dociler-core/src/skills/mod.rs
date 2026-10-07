//! Compact, immutable Markdown skill modules embedded and versioned with Dociler.
//!
//! Provides the canonical skill definitions and task-relevant module selection
//! for the 5-layer prompt construction pipeline.

/// Version of the embedded Dociler skills pack.
pub const SKILLS_VERSION: &str = "1.0.0";

const CORE_POLICY_MD: &str = include_str!("core_policy.md");
const DISCOVERY_MD: &str = include_str!("discovery.md");
const GROUNDED_READING_MD: &str = include_str!("grounded_reading.md");
const SUMMARIZATION_MD: &str = include_str!("summarization.md");
const COMPARISON_MD: &str = include_str!("comparison.md");
const CITATION_MD: &str = include_str!("citation.md");
const SAFE_EDITING_MD: &str = include_str!("safe_editing.md");

/// Built-in immutable skill modules embedded with the release.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SkillModule {
    CorePolicy,
    Discovery,
    GroundedReading,
    Summarization,
    Comparison,
    Citation,
    SafeEditing,
}

impl SkillModule {
    /// Unique machine-readable identifier for the skill module.
    pub fn id(&self) -> &'static str {
        match self {
            Self::CorePolicy => "core-policy",
            Self::Discovery => "discovery",
            Self::GroundedReading => "grounded-reading",
            Self::Summarization => "summarization",
            Self::Comparison => "comparison",
            Self::Citation => "citation",
            Self::SafeEditing => "safe-editing",
        }
    }

    /// Human-readable title of the skill module.
    pub fn title(&self) -> &'static str {
        match self {
            Self::CorePolicy => "Dociler Core Grounding Policy",
            Self::Discovery => "Document Discovery and Workspace Cataloging",
            Self::GroundedReading => "Grounded Document Reading",
            Self::Summarization => "Document Summarization",
            Self::Comparison => "Multi-Document Comparison",
            Self::Citation => "Source Citation Standards",
            Self::SafeEditing => "Safe Text Document Editing",
        }
    }

    /// Immutable markdown content of the skill module.
    pub fn content(&self) -> &'static str {
        match self {
            Self::CorePolicy => CORE_POLICY_MD,
            Self::Discovery => DISCOVERY_MD,
            Self::GroundedReading => GROUNDED_READING_MD,
            Self::Summarization => SUMMARIZATION_MD,
            Self::Comparison => COMPARISON_MD,
            Self::Citation => CITATION_MD,
            Self::SafeEditing => SAFE_EDITING_MD,
        }
    }

    /// All available skill modules.
    pub fn all() -> &'static [SkillModule] {
        &[
            Self::CorePolicy,
            Self::Discovery,
            Self::GroundedReading,
            Self::Summarization,
            Self::Comparison,
            Self::Citation,
            Self::SafeEditing,
        ]
    }
}

/// Selects task-relevant skill modules based on the user's query and task context.
///
/// Note: Dociler Core Grounding Policy is Layer 1 of every prompt.
/// The modules returned here represent task-relevant Layer 2 skills.
pub fn select_task_skills(
    query: &str,
    has_documents: bool,
    is_edit_request: bool,
) -> Vec<SkillModule> {
    let mut skills = Vec::new();
    let q_lower = query.to_ascii_lowercase();

    // Safe editing skill
    if is_edit_request
        || contains_any(
            &q_lower,
            &[
                "edit", "modify", "replace", "update", "patch", "ubah", "ganti", "perbaiki",
            ],
        )
    {
        skills.push(SkillModule::SafeEditing);
    }

    // Discovery skill
    if contains_any(
        &q_lower,
        &[
            "find",
            "discover",
            "list file",
            "list doc",
            "search doc",
            "workspace file",
            "workspace doc",
            "show doc",
            "show file",
            "cari berkas",
            "daftar dokumen",
            "cari dokumen",
        ],
    ) {
        skills.push(SkillModule::Discovery);
    }

    // Comparison skill
    let is_comparison = contains_any(
        &q_lower,
        &[
            "compare",
            "comparison",
            "contrast",
            "difference",
            "versus",
            " vs ",
            "bandingkan",
            "perbandingan",
            "beda",
            "perbedaan",
        ],
    );
    if is_comparison {
        skills.push(SkillModule::Comparison);
    }

    // Summarization skill
    let is_summarization = contains_any(
        &q_lower,
        &[
            "summarize",
            "summary",
            "summaries",
            "overview",
            "synopsis",
            "tl;dr",
            "tldr",
            "ringkas",
            "ringkasan",
            "ikhtisar",
            "rangkum",
        ],
    );
    if is_summarization {
        skills.push(SkillModule::Summarization);
    }

    // Grounded reading and Citation skills for document interactions
    if has_documents {
        if !is_comparison && !is_summarization {
            skills.push(SkillModule::GroundedReading);
        }
        if !skills.contains(&SkillModule::Citation) {
            skills.push(SkillModule::Citation);
        }
    } else if (is_comparison || is_summarization) && !skills.contains(&SkillModule::Citation) {
        skills.push(SkillModule::Citation);
    }

    skills
}

fn contains_any(text: &str, keywords: &[&str]) -> bool {
    keywords.iter().any(|&kw| text.contains(kw))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skills_version_and_inventory() {
        assert_eq!(SKILLS_VERSION, "1.0.0");
        assert_eq!(SkillModule::all().len(), 7);

        for skill in SkillModule::all() {
            assert!(!skill.id().is_empty());
            assert!(!skill.title().is_empty());
            assert!(!skill.content().trim().is_empty());
        }
    }

    #[test]
    fn embedded_content_matches_expected_structure() {
        assert!(
            SkillModule::CorePolicy
                .content()
                .contains("Dociler, a local-first document assistant")
        );
        assert!(
            SkillModule::Discovery
                .content()
                .contains("Workspace Boundaries")
        );
        assert!(
            SkillModule::GroundedReading
                .content()
                .contains("Grounded Document Reading")
        );
        assert!(
            SkillModule::Summarization
                .content()
                .contains("Document Summarization")
        );
        assert!(
            SkillModule::Comparison
                .content()
                .contains("Multi-Document Comparison")
        );
        assert!(
            SkillModule::Citation
                .content()
                .contains("Source Citation Standards")
        );
        assert!(
            SkillModule::SafeEditing
                .content()
                .contains("Safe Text Document Editing")
        );
    }

    #[test]
    fn select_task_skills_for_qa() {
        let skills = select_task_skills("What is the project revenue in 2024?", true, false);
        assert!(skills.contains(&SkillModule::GroundedReading));
        assert!(skills.contains(&SkillModule::Citation));
        assert!(!skills.contains(&SkillModule::Summarization));
        assert!(!skills.contains(&SkillModule::Comparison));
    }

    #[test]
    fn select_task_skills_for_summarization() {
        let skills = select_task_skills("Please summarize the attached document", true, false);
        assert!(skills.contains(&SkillModule::Summarization));
        assert!(skills.contains(&SkillModule::Citation));
        assert!(!skills.contains(&SkillModule::GroundedReading));
    }

    #[test]
    fn select_task_skills_for_comparison() {
        let skills = select_task_skills("Compare contract A vs contract B", true, false);
        assert!(skills.contains(&SkillModule::Comparison));
        assert!(skills.contains(&SkillModule::Citation));
        assert!(!skills.contains(&SkillModule::GroundedReading));
    }

    #[test]
    fn select_task_skills_for_editing() {
        let skills = select_task_skills("Edit the header in readme.md", true, true);
        assert!(skills.contains(&SkillModule::SafeEditing));
        assert!(skills.contains(&SkillModule::Citation));
        assert!(skills.contains(&SkillModule::GroundedReading));
    }

    #[test]
    fn select_task_skills_for_discovery() {
        let skills = select_task_skills("Find all files in workspace", false, false);
        assert!(skills.contains(&SkillModule::Discovery));
        assert!(!skills.contains(&SkillModule::Citation));
    }

    #[test]
    fn select_task_skills_for_general_chat() {
        let skills = select_task_skills("Hello, how are you today?", false, false);
        assert!(skills.is_empty());
    }
}
