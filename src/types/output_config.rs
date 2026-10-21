use serde::ser::SerializeMap;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::types::OutputFormat;

/// A token budget for an entire agentic task.
///
/// Serializes as `{"type":"tokens","total":N,"remaining":M}`. Requires the
/// `task-budgets-2026-03-13` beta, which is added automatically.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct TaskBudget {
    /// Total tokens for the task; at least 20,000 (checked by request validation).
    pub total: u64,

    /// Tokens left, when the task spans several requests.
    pub remaining: Option<u64>,
}

impl TaskBudget {
    /// Create a task budget with the given total.
    #[must_use]
    pub fn new(total: u64) -> Self {
        Self {
            total,
            remaining: None,
        }
    }

    /// Set the remaining tokens.
    #[must_use]
    pub fn with_remaining(mut self, remaining: u64) -> Self {
        self.remaining = Some(remaining);
        self
    }
}

impl Serialize for TaskBudget {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(2 + usize::from(self.remaining.is_some())))?;
        map.serialize_entry("type", "tokens")?;
        map.serialize_entry("total", &self.total)?;
        if let Some(remaining) = &self.remaining {
            map.serialize_entry("remaining", remaining)?;
        }
        map.end()
    }
}

impl<'de> Deserialize<'de> for TaskBudget {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct Raw {
            total: u64,
            #[serde(default)]
            remaining: Option<u64>,
        }
        let raw = Raw::deserialize(deserializer)?;
        Ok(Self {
            total: raw.total,
            remaining: raw.remaining,
        })
    }
}

/// Effort level for controlling thinking depth with adaptive thinking.
///
/// Used with `ThinkingConfig::Adaptive` to control how much thinking
/// Claude applies to a task.
///
/// Variants are declared from least to most effort, so the derived `Ord` ranks them.
#[derive(Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum Effort {
    /// Low effort - minimal thinking for simple tasks
    Low,
    /// Medium effort - moderate thinking
    Medium,
    /// High effort - thorough thinking for complex tasks; API default
    High,
    /// Extra-high effort - extended capability for long-horizon work.
    #[serde(rename = "xhigh")]
    XHigh,
    /// Maximum effort - always think with no constraints on thinking depth.
    Max,
}

/// Output configuration for API requests.
///
/// It supports the structured output format, the effort parameter for adaptive thinking,
/// and a task budget.
///
/// # Example
///
/// ```
/// use topcoat_si::{OutputConfig, Effort};
///
/// // Control thinking effort
/// let config = OutputConfig::new().with_effort(Effort::High);
/// ```
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[non_exhaustive]
pub struct OutputConfig {
    /// Output format configuration for structured outputs.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format: Option<OutputFormat>,

    /// Effort level for controlling thinking depth.
    ///
    /// Used with `ThinkingConfig::Adaptive` to control how much thinking
    /// Claude applies to a task.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effort: Option<Effort>,

    /// Token budget for the whole agentic task.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_budget: Option<TaskBudget>,
}

impl OutputConfig {
    /// Create a new empty output configuration.
    #[must_use]
    pub fn new() -> Self {
        Self {
            format: None,
            effort: None,
            task_budget: None,
        }
    }

    /// Set the output format.
    #[must_use]
    pub fn with_format(mut self, format: OutputFormat) -> Self {
        self.format = Some(format);
        self
    }

    /// Set the effort level.
    #[must_use]
    pub fn with_effort(mut self, effort: Effort) -> Self {
        self.effort = Some(effort);
        self
    }

    /// Set the task budget.
    #[must_use]
    pub fn with_task_budget(mut self, task_budget: TaskBudget) -> Self {
        self.task_budget = Some(task_budget);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, to_value};

    #[test]
    fn effort_orders_from_low_to_max() {
        assert!(Effort::Low < Effort::Medium);
        assert!(Effort::Medium < Effort::High);
        assert!(Effort::High < Effort::XHigh);
        assert!(Effort::XHigh < Effort::Max);
    }

    #[test]
    fn effort_serialization() {
        assert_eq!(to_value(Effort::Low).unwrap(), json!("low"));
        assert_eq!(to_value(Effort::Medium).unwrap(), json!("medium"));
        assert_eq!(to_value(Effort::High).unwrap(), json!("high"));
        assert_eq!(to_value(Effort::XHigh).unwrap(), json!("xhigh"));
        assert_eq!(to_value(Effort::Max).unwrap(), json!("max"));
    }

    #[test]
    fn effort_deserialization() {
        let low: Effort = serde_json::from_value(json!("low")).unwrap();
        assert_eq!(low, Effort::Low);

        let medium: Effort = serde_json::from_value(json!("medium")).unwrap();
        assert_eq!(medium, Effort::Medium);

        let high: Effort = serde_json::from_value(json!("high")).unwrap();
        assert_eq!(high, Effort::High);

        let xhigh: Effort = serde_json::from_value(json!("xhigh")).unwrap();
        assert_eq!(xhigh, Effort::XHigh);

        let max: Effort = serde_json::from_value(json!("max")).unwrap();
        assert_eq!(max, Effort::Max);
    }

    #[test]
    fn output_config_with_effort_only() {
        let config = OutputConfig::new().with_effort(Effort::High);
        let json = to_value(&config).unwrap();

        assert_eq!(
            json,
            json!({
                "effort": "high"
            })
        );
    }

    #[test]
    fn output_config_with_format_only() {
        let config = OutputConfig::new().with_format(OutputFormat::json_schema(json!({
            "type": "object",
            "properties": {
                "name": { "type": "string" }
            },
            "required": ["name"],
            "additionalProperties": false
        })));

        let json = to_value(&config).unwrap();

        assert_eq!(
            json,
            json!({
                "format": {
                    "type": "json_schema",
                    "schema": {
                        "type": "object",
                        "properties": {
                            "name": { "type": "string" }
                        },
                        "required": ["name"],
                        "additionalProperties": false
                    }
                }
            })
        );
    }

    #[test]
    fn output_config_with_both_format_and_effort() {
        let config = OutputConfig::new()
            .with_format(OutputFormat::json_schema(json!({
                "type": "object",
                "properties": {
                    "answer": { "type": "boolean" }
                },
                "required": ["answer"],
                "additionalProperties": false
            })))
            .with_effort(Effort::Medium);

        let json = to_value(&config).unwrap();

        assert_eq!(
            json,
            json!({
                "format": {
                    "type": "json_schema",
                    "schema": {
                        "type": "object",
                        "properties": {
                            "answer": { "type": "boolean" }
                        },
                        "required": ["answer"],
                        "additionalProperties": false
                    }
                },
                "effort": "medium"
            })
        );
    }

    #[test]
    fn output_config_empty_skips_none_fields() {
        let config = OutputConfig::new();
        let json = to_value(&config).unwrap();

        // Both fields are None, so they should be omitted
        assert_eq!(json, json!({}));
        assert!(!json.as_object().unwrap().contains_key("format"));
        assert!(!json.as_object().unwrap().contains_key("effort"));
    }

    #[test]
    fn output_config_deserialization_effort_only() {
        let json = json!({
            "effort": "low"
        });

        let config: OutputConfig = serde_json::from_value(json).unwrap();
        assert_eq!(config.effort, Some(Effort::Low));
        assert_eq!(config.format, None);
    }

    #[test]
    fn output_config_deserialization_format_only() {
        let json = json!({
            "format": {
                "type": "json_schema",
                "schema": {
                    "type": "object",
                    "properties": {
                        "name": { "type": "string" }
                    },
                    "required": ["name"],
                    "additionalProperties": false
                }
            }
        });

        let config: OutputConfig = serde_json::from_value(json).unwrap();
        assert!(config.format.is_some());
        assert_eq!(config.effort, None);
    }

    #[test]
    fn output_config_deserialization_both() {
        let json = json!({
            "format": {
                "type": "json_schema",
                "schema": {
                    "type": "object",
                    "properties": {
                        "result": { "type": "string" }
                    },
                    "required": ["result"],
                    "additionalProperties": false
                }
            },
            "effort": "high"
        });

        let config: OutputConfig = serde_json::from_value(json).unwrap();
        assert!(config.format.is_some());
        assert_eq!(config.effort, Some(Effort::High));
    }

    #[test]
    fn output_config_deserialization_empty() {
        let json = json!({});

        let config: OutputConfig = serde_json::from_value(json).unwrap();
        assert_eq!(config.format, None);
        assert_eq!(config.effort, None);
    }

    #[test]
    fn output_config_task_budget_serialization() {
        let config = OutputConfig::new()
            .with_effort(Effort::High)
            .with_task_budget(TaskBudget::new(64_000));
        assert_eq!(
            to_value(&config).unwrap(),
            json!({"effort": "high", "task_budget": {"type": "tokens", "total": 64000}})
        );
    }

    #[test]
    fn task_budget_with_remaining_round_trips() {
        let budget = TaskBudget::new(64_000).with_remaining(10_000);
        let value = to_value(budget).unwrap();
        assert_eq!(
            value,
            json!({"type": "tokens", "total": 64000, "remaining": 10000})
        );
        assert_eq!(serde_json::from_value::<TaskBudget>(value).unwrap(), budget);
    }

    #[test]
    fn output_config_default() {
        let config = OutputConfig::default();
        assert_eq!(config.format, None);
        assert_eq!(config.effort, None);
    }
}
