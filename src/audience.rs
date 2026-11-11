//! Fixed audience taxonomy, selected from the pilot audience labels.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "kebab-case")]
pub enum Audience {
    Developers,
    SystemAdministrators,
    StudentsLearners,
    Educators,
    Researchers,
    CreativeProfessionals,
    Gamers,
    MakersHobbyists,
    BusinessProfessionals,
    FoundersBusinessOwners,
    CommunityOrganizers,
    PersonalUse,
    GeneralPublic,
}
impl Audience {
    pub const ALL: [Self; 13] = [
        Self::Developers,
        Self::SystemAdministrators,
        Self::StudentsLearners,
        Self::Educators,
        Self::Researchers,
        Self::CreativeProfessionals,
        Self::Gamers,
        Self::MakersHobbyists,
        Self::BusinessProfessionals,
        Self::FoundersBusinessOwners,
        Self::CommunityOrganizers,
        Self::PersonalUse,
        Self::GeneralPublic,
    ];
    pub fn slug(self) -> &'static str {
        match self {
            Self::Developers => "developers",
            Self::SystemAdministrators => "system-administrators",
            Self::StudentsLearners => "students-learners",
            Self::Educators => "educators",
            Self::Researchers => "researchers",
            Self::CreativeProfessionals => "creative-professionals",
            Self::Gamers => "gamers",
            Self::MakersHobbyists => "makers-hobbyists",
            Self::BusinessProfessionals => "business-professionals",
            Self::FoundersBusinessOwners => "founders-business-owners",
            Self::CommunityOrganizers => "community-organizers",
            Self::PersonalUse => "personal-use",
            Self::GeneralPublic => "general-public",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Developers => "Developers",
            Self::SystemAdministrators => "System administrators",
            Self::StudentsLearners => "Students & learners",
            Self::Educators => "Educators",
            Self::Researchers => "Researchers",
            Self::CreativeProfessionals => "Creative professionals",
            Self::Gamers => "Gamers",
            Self::MakersHobbyists => "Makers & hobbyists",
            Self::BusinessProfessionals => "Business professionals",
            Self::FoundersBusinessOwners => "Founders & business owners",
            Self::CommunityOrganizers => "Community organizers",
            Self::PersonalUse => "Personal use",
            Self::GeneralPublic => "General public",
        }
    }
    pub fn description(self) -> &'static str {
        match self {
            Self::Developers => "Software developers and testers.",
            Self::SystemAdministrators => "Infrastructure operators and self-hosters.",
            Self::StudentsLearners => "People studying or learning skills.",
            Self::Educators => "Teachers and educational practitioners.",
            Self::Researchers => "Scientific and technical investigators.",
            Self::CreativeProfessionals => "Writers, artists, designers and media creators.",
            Self::Gamers => "People playing games.",
            Self::MakersHobbyists => "Hands-on makers and people pursuing hobbies.",
            Self::BusinessProfessionals => "People doing business, finance or operational work.",
            Self::FoundersBusinessOwners => "Entrepreneurs, founders and business owners.",
            Self::CommunityOrganizers => "People coordinating communities, events and civic work.",
            Self::PersonalUse => "Explicit individual or personal use.",
            Self::GeneralPublic => "Explicit broad public or consumer audience.",
        }
    }
    pub fn from_slug(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|a| a.slug() == s)
    }
}
#[derive(Serialize)]
pub struct AudienceMetadata {
    pub slug: &'static str,
    pub label: &'static str,
    pub description: &'static str,
}
pub fn metadata() -> Vec<AudienceMetadata> {
    Audience::ALL
        .into_iter()
        .map(|a| AudienceMetadata {
            slug: a.slug(),
            label: a.label(),
            description: a.description(),
        })
        .collect()
}
