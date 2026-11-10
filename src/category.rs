//! Fixed discovery taxonomy. Slugs are the persisted and public identities.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "kebab-case")]
pub enum Category {
    AiAutomation,
    DeveloperTools,
    Productivity,
    Education,
    Games,
    CreativeMedia,
    DataAnalytics,
    BusinessFinance,
    CommunityCivic,
    HardwareDiy,
    InfrastructureHosting,
    SecurityPrivacy,
    ScienceResearch,
    HealthFitness,
    LifestyleHobbies,
}

impl Category {
    pub const ALL: [Self; 15] = [
        Self::AiAutomation,
        Self::DeveloperTools,
        Self::Productivity,
        Self::Education,
        Self::Games,
        Self::CreativeMedia,
        Self::DataAnalytics,
        Self::BusinessFinance,
        Self::CommunityCivic,
        Self::HardwareDiy,
        Self::InfrastructureHosting,
        Self::SecurityPrivacy,
        Self::ScienceResearch,
        Self::HealthFitness,
        Self::LifestyleHobbies,
    ];

    pub fn slug(self) -> &'static str {
        match self {
            Self::AiAutomation => "ai-automation",
            Self::DeveloperTools => "developer-tools",
            Self::Productivity => "productivity",
            Self::Education => "education",
            Self::Games => "games",
            Self::CreativeMedia => "creative-media",
            Self::DataAnalytics => "data-analytics",
            Self::BusinessFinance => "business-finance",
            Self::CommunityCivic => "community-civic",
            Self::HardwareDiy => "hardware-diy",
            Self::InfrastructureHosting => "infrastructure-hosting",
            Self::SecurityPrivacy => "security-privacy",
            Self::ScienceResearch => "science-research",
            Self::HealthFitness => "health-fitness",
            Self::LifestyleHobbies => "lifestyle-hobbies",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::AiAutomation => "AI & automation",
            Self::DeveloperTools => "Developer tools",
            Self::Productivity => "Productivity",
            Self::Education => "Education",
            Self::Games => "Games",
            Self::CreativeMedia => "Creative tools & media",
            Self::DataAnalytics => "Data & analytics",
            Self::BusinessFinance => "Business & finance",
            Self::CommunityCivic => "Community & civic",
            Self::HardwareDiy => "Hardware & DIY",
            Self::InfrastructureHosting => "Infrastructure & hosting",
            Self::SecurityPrivacy => "Security & privacy",
            Self::ScienceResearch => "Science & research",
            Self::HealthFitness => "Health & fitness",
            Self::LifestyleHobbies => "Lifestyle & hobbies",
        }
    }
    pub fn description(self) -> &'static str {
        match self {
            Self::AiAutomation => {
                "Products whose core purpose uses AI, machine learning, or automation; using AI to build a product is insufficient."
            }
            Self::DeveloperTools => {
                "Tools for writing, testing, releasing, or maintaining software."
            }
            Self::Productivity => {
                "Tools for organizing work, notes, tasks, and personal information."
            }
            Self::Education => "Learning, teaching, study, and educational resources.",
            Self::Games => "Playable games and tools centered on gaming.",
            Self::CreativeMedia => {
                "Tools and works for writing, design, images, audio, video, or other media."
            }
            Self::DataAnalytics => "Databases, data processing, visualization, and analysis.",
            Self::BusinessFinance => {
                "Commerce, business operations, finance, and professional services."
            }
            Self::CommunityCivic => {
                "Communities, social coordination, public affairs, and civic participation."
            }
            Self::HardwareDiy => {
                "Physical devices, electronics, fabrication, repair, and DIY builds."
            }
            Self::InfrastructureHosting => {
                "Hosting, networking, storage, operating systems, and service infrastructure."
            }
            Self::SecurityPrivacy => {
                "Authentication, protection, encryption, and privacy-focused products."
            }
            Self::ScienceResearch => {
                "Scientific inquiry, research methods, and technical experiments."
            }
            Self::HealthFitness => "Health, wellness, exercise, and fitness.",
            Self::LifestyleHobbies => "Personal interests, food, home, travel, and other hobbies.",
        }
    }
    pub fn from_slug(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|c| c.slug() == s)
    }
}

#[derive(Serialize)]
pub struct CategoryMetadata {
    pub slug: &'static str,
    pub label: &'static str,
    pub description: &'static str,
}
pub fn metadata() -> Vec<CategoryMetadata> {
    Category::ALL
        .into_iter()
        .map(|c| CategoryMetadata {
            slug: c.slug(),
            label: c.label(),
            description: c.description(),
        })
        .collect()
}
