//! Attribute filters for queries.

use serde_json::{Value, json};

use crate::error::{Error, Result};

/// A condition on document attributes that narrows a query.
///
/// Combine conditions with [`Filter::all`], [`Filter::any`] and [`Filter::not`]. An empty `all`
/// or `any` is rejected when the query is validated, since it would match everything.
#[derive(Debug, Clone, PartialEq)]
pub struct Filter {
    node: Node,
}

#[derive(Debug, Clone, PartialEq)]
enum Node {
    Condition {
        attribute: String,
        operator: &'static str,
        value: Value,
    },
    And(Vec<Filter>),
    Or(Vec<Filter>),
    Not(Box<Filter>),
}

impl Filter {
    fn condition(attribute: impl Into<String>, operator: &'static str, value: Value) -> Self {
        Self {
            node: Node::Condition {
                attribute: attribute.into(),
                operator,
                value,
            },
        }
    }

    fn list(values: impl IntoIterator<Item = impl Into<Value>>) -> Value {
        Value::Array(values.into_iter().map(Into::into).collect())
    }

    /// The attribute equals `value`.
    #[must_use]
    pub fn equals(attribute: impl Into<String>, value: impl Into<Value>) -> Self {
        Self::condition(attribute, "Eq", value.into())
    }

    /// The attribute does not equal `value`.
    #[must_use]
    pub fn not_equals(attribute: impl Into<String>, value: impl Into<Value>) -> Self {
        Self::condition(attribute, "NotEq", value.into())
    }

    /// The attribute equals one of `values`.
    #[must_use]
    pub fn one_of(
        attribute: impl Into<String>,
        values: impl IntoIterator<Item = impl Into<Value>>,
    ) -> Self {
        Self::condition(attribute, "In", Self::list(values))
    }

    /// The attribute equals none of `values`.
    #[must_use]
    pub fn none_of(
        attribute: impl Into<String>,
        values: impl IntoIterator<Item = impl Into<Value>>,
    ) -> Self {
        Self::condition(attribute, "NotIn", Self::list(values))
    }

    /// The attribute is less than `value`.
    #[must_use]
    pub fn less_than(attribute: impl Into<String>, value: impl Into<Value>) -> Self {
        Self::condition(attribute, "Lt", value.into())
    }

    /// The attribute is less than or equal to `value`.
    #[must_use]
    pub fn at_most(attribute: impl Into<String>, value: impl Into<Value>) -> Self {
        Self::condition(attribute, "Lte", value.into())
    }

    /// The attribute is greater than `value`.
    #[must_use]
    pub fn greater_than(attribute: impl Into<String>, value: impl Into<Value>) -> Self {
        Self::condition(attribute, "Gt", value.into())
    }

    /// The attribute is greater than or equal to `value`.
    #[must_use]
    pub fn at_least(attribute: impl Into<String>, value: impl Into<Value>) -> Self {
        Self::condition(attribute, "Gte", value.into())
    }

    /// The array attribute contains `value`.
    #[must_use]
    pub fn contains(attribute: impl Into<String>, value: impl Into<Value>) -> Self {
        Self::condition(attribute, "Contains", value.into())
    }

    /// The array attribute contains at least one of `values`.
    #[must_use]
    pub fn contains_any(
        attribute: impl Into<String>,
        values: impl IntoIterator<Item = impl Into<Value>>,
    ) -> Self {
        Self::condition(attribute, "ContainsAny", Self::list(values))
    }

    /// Every one of `filters` matches.
    #[must_use]
    pub fn all(filters: impl IntoIterator<Item = Filter>) -> Self {
        Self {
            node: Node::And(filters.into_iter().collect()),
        }
    }

    /// At least one of `filters` matches.
    #[must_use]
    pub fn any(filters: impl IntoIterator<Item = Filter>) -> Self {
        Self {
            node: Node::Or(filters.into_iter().collect()),
        }
    }

    /// `filter` does not match.
    // An associated constructor beside `all` and `any`, not an operator on a receiver.
    #[allow(clippy::should_implement_trait)]
    #[must_use]
    pub fn not(filter: Filter) -> Self {
        Self {
            node: Node::Not(Box::new(filter)),
        }
    }

    /// Rejects an empty `all` or `any` anywhere in the filter.
    pub(super) fn validate(&self) -> Result<()> {
        match &self.node {
            Node::Condition { .. } => Ok(()),
            Node::And(filters) | Node::Or(filters) if filters.is_empty() => Err(Error::validation(
                "a Filter::all or Filter::any must hold at least one filter",
                Some("filter".to_string()),
            )),
            Node::And(filters) | Node::Or(filters) => filters.iter().try_for_each(Filter::validate),
            Node::Not(filter) => filter.validate(),
        }
    }

    /// The filter in turbopuffer's array form, such as `["kind", "Eq", "product"]`.
    pub(super) fn to_wire(&self) -> Value {
        match &self.node {
            Node::Condition {
                attribute,
                operator,
                value,
            } => json!([attribute, operator, value]),
            Node::And(filters) => json!([
                "And",
                filters.iter().map(Filter::to_wire).collect::<Vec<_>>()
            ]),
            Node::Or(filters) => json!([
                "Or",
                filters.iter().map(Filter::to_wire).collect::<Vec<_>>()
            ]),
            Node::Not(filter) => json!(["Not", filter.to_wire()]),
        }
    }
}
