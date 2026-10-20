use serde::de::{self, Deserializer};
use serde::ser::{SerializeMap, Serializer};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

/// Whether a skill is provided by Anthropic or by the user.
#[derive(Debug, Copy, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum SkillType {
    /// A skill provided by Anthropic.
    Anthropic,
    /// A skill created by the user.
    Custom,
}

/// A reference to a skill loaded into a container.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SkillRef {
    /// Whether the skill is Anthropic-provided or custom.
    pub r#type: SkillType,

    /// Identifier of the skill.
    pub skill_id: String,

    /// Skill version; the latest version is used when omitted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

impl SkillRef {
    /// Create a reference to an Anthropic-provided skill.
    pub fn anthropic(skill_id: impl Into<String>) -> Self {
        Self {
            r#type: SkillType::Anthropic,
            skill_id: skill_id.into(),
            version: None,
        }
    }

    /// Create a reference to a custom skill.
    pub fn custom(skill_id: impl Into<String>) -> Self {
        Self {
            r#type: SkillType::Custom,
            skill_id: skill_id.into(),
            version: None,
        }
    }

    /// Pin the skill to a version.
    pub fn with_version(mut self, version: impl Into<String>) -> Self {
        self.version = Some(version.into());
        self
    }
}

/// The container to run code execution and skills in.
///
/// Serializes as a bare string for [`Container::Id`] and as an object for
/// [`Container::Config`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Container {
    /// Reuse an existing container by identifier.
    Id(String),
    /// Configure a container, optionally loading skills (at most 20).
    Config {
        /// Identifier of a container to reuse.
        id: Option<String>,
        /// Skills to load into the container.
        skills: Vec<SkillRef>,
    },
}

impl Container {
    /// Reuse the container with the given identifier.
    pub fn id(id: impl Into<String>) -> Self {
        Self::Id(id.into())
    }

    /// Create a new container with the given skills.
    pub fn with_skills(skills: Vec<SkillRef>) -> Self {
        Self::Config { id: None, skills }
    }
}

impl Serialize for Container {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Container::Id(id) => serializer.serialize_str(id),
            Container::Config { id, skills } => {
                let mut map = serializer.serialize_map(Some(1 + usize::from(id.is_some())))?;
                if let Some(id) = id {
                    map.serialize_entry("id", id)?;
                }
                map.serialize_entry("skills", skills)?;
                map.end()
            }
        }
    }
}

impl<'de> Deserialize<'de> for Container {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            Id(String),
            Config {
                #[serde(default)]
                id: Option<String>,
                #[serde(default)]
                skills: Vec<SkillRef>,
            },
        }

        match Raw::deserialize(deserializer).map_err(de::Error::custom)? {
            Raw::Id(id) => Ok(Container::Id(id)),
            Raw::Config { id, skills } => Ok(Container::Config { id, skills }),
        }
    }
}

/// Information about the container used in a response.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ContainerInfo {
    /// Identifier of the container; pass it back to reuse the container.
    pub id: String,

    /// When the container expires.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "time::serde::rfc3339::option"
    )]
    pub expires_at: Option<OffsetDateTime>,

    /// Skills loaded in the container.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skills: Option<Vec<SkillRef>>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use time::macros::datetime;

    #[test]
    fn container_id_serializes_as_string() {
        let value = serde_json::to_value(Container::id("container_011")).unwrap();
        assert_eq!(value, json!("container_011"));
    }

    #[test]
    fn container_config_serializes_as_object() {
        let container = Container::Config {
            id: None,
            skills: vec![
                SkillRef::anthropic("pptx").with_version("latest"),
                SkillRef::custom("skill_01"),
            ],
        };
        assert_eq!(
            serde_json::to_value(container).unwrap(),
            json!({
                "skills": [
                    {"type": "anthropic", "skill_id": "pptx", "version": "latest"},
                    {"type": "custom", "skill_id": "skill_01"}
                ]
            })
        );
    }

    #[test]
    fn container_config_with_id() {
        let container = Container::Config {
            id: Some("container_011".to_string()),
            skills: vec![],
        };
        assert_eq!(
            serde_json::to_value(container).unwrap(),
            json!({"id": "container_011", "skills": []})
        );
    }

    #[test]
    fn container_round_trips() {
        let id: Container = serde_json::from_value(json!("container_011")).unwrap();
        assert_eq!(id, Container::id("container_011"));
        let config: Container =
            serde_json::from_value(json!({"skills": [{"type": "anthropic", "skill_id": "xlsx"}]}))
                .unwrap();
        assert_eq!(
            config,
            Container::with_skills(vec![SkillRef::anthropic("xlsx")])
        );
    }

    #[test]
    fn container_info_deserialization() {
        let info: ContainerInfo = serde_json::from_value(json!({
            "id": "container_011",
            "expires_at": "2026-10-19T12:00:00Z",
            "skills": [{"type": "anthropic", "skill_id": "pptx", "version": "1"}]
        }))
        .unwrap();
        assert_eq!(info.id, "container_011");
        assert_eq!(info.expires_at, Some(datetime!(2026-10-19 12:00:00 UTC)));
        assert_eq!(
            info.skills,
            Some(vec![SkillRef::anthropic("pptx").with_version("1")])
        );
    }

    #[test]
    fn container_info_minimal() {
        let info: ContainerInfo = serde_json::from_value(json!({"id": "c"})).unwrap();
        assert_eq!(info.expires_at, None);
        assert_eq!(info.skills, None);
        assert_eq!(serde_json::to_value(&info).unwrap(), json!({"id": "c"}));
    }
}
