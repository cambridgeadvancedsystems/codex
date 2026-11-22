use crate::ModelProviderInfo;
use codex_protocol::models::ResponseItem;
use serde_json::Value;
use std::sync::Arc;

/// Built-in/client Responses API tool types that cannot be injected via `hosted_tools`.
pub const RESERVED_HOSTED_TOOL_TYPES: &[&str] = &[
    "function",
    "custom",
    "namespace",
    "tool_search",
    "local_shell",
    "shell",
    "apply_patch",
    "computer_use",
    "code_interpreter",
];

pub fn validate_hosted_tool(tool: &Value) -> Result<(), String> {
    let Some(tool_type) = tool
        .as_object()
        .and_then(|object| object.get("type"))
        .and_then(Value::as_str)
    else {
        return Err("must be a JSON object with a non-empty string `type`".to_string());
    };
    let trimmed = tool_type.trim();
    if trimmed.is_empty() || trimmed != tool_type {
        return Err("must be a JSON object with a non-empty string `type`".to_string());
    }
    if RESERVED_HOSTED_TOOL_TYPES
        .iter()
        .any(|reserved| reserved.eq_ignore_ascii_case(tool_type))
    {
        return Err(format!("uses reserved built-in tool type `{tool_type}`"));
    }
    Ok(())
}

pub fn validate_hosted_tool_config(
    hosted_tools: &[Value],
    hosted_tool_call_prefixes: &[String],
    hosted_tool_names: &[String],
) -> Result<(), String> {
    for (index, tool) in hosted_tools.iter().enumerate() {
        validate_hosted_tool(tool)
            .map_err(|reason| format!("provider hosted_tools[{index}] {reason}"))?;
    }
    if hosted_tool_call_prefixes
        .iter()
        .any(|prefix| prefix.trim().is_empty())
    {
        return Err(
            "provider hosted_tool_call_prefixes must not contain empty strings".to_string(),
        );
    }
    if hosted_tool_names.iter().any(|name| name.trim().is_empty()) {
        return Err("provider hosted_tool_names must not contain empty strings".to_string());
    }
    Ok(())
}

/// Returns whether a provider-hosted tool definition represents a live web-search tool
/// (e.g. `type = "web_search"`, `type = "web_search_20250305"`, or `name = "web_search"`).
pub fn is_hosted_web_search_tool(tool: &Value) -> bool {
    let Some(object) = tool.as_object() else {
        return false;
    };
    let type_matches = object
        .get("type")
        .and_then(Value::as_str)
        .is_some_and(|tool_type| {
            let lower = tool_type.to_ascii_lowercase();
            lower == "web_search" || lower.starts_with("web_search_")
        });
    let name_matches = object
        .get("name")
        .and_then(Value::as_str)
        .is_some_and(|name| name.eq_ignore_ascii_case("web_search"));
    type_matches || name_matches
}

pub fn matches_hosted_custom_tool_call(
    prefixes: &[String],
    names: &[String],
    call_id: &str,
    name: &str,
    namespace: Option<&str>,
) -> bool {
    let is_default_namespace = namespace
        .is_none_or(|ns| ns.is_empty() || ns == codex_protocol::DEFAULT_FUNCTION_NAMESPACE);
    if !is_default_namespace {
        return false;
    }
    prefixes
        .iter()
        .any(|prefix| !prefix.is_empty() && call_id.starts_with(prefix))
        || names
            .iter()
            .any(|hosted_name| !hosted_name.is_empty() && hosted_name == name)
}

pub fn matches_hosted_custom_tool_call_item(
    prefixes: &[String],
    names: &[String],
    item: &ResponseItem,
) -> bool {
    let ResponseItem::CustomToolCall {
        call_id,
        name,
        namespace,
        ..
    } = item
    else {
        return false;
    };
    matches_hosted_custom_tool_call(prefixes, names, call_id, name, namespace.as_deref())
}

/// Matches inbound `custom_tool_call` items executed server-side by a model provider.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HostedToolCallMatcher {
    prefixes: Arc<[String]>,
    names: Arc<[String]>,
}

impl HostedToolCallMatcher {
    pub fn new(prefixes: Vec<String>, names: Vec<String>) -> Self {
        Self {
            prefixes: prefixes.into(),
            names: names.into(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.prefixes.is_empty() && self.names.is_empty()
    }

    pub fn matches(&self, call_id: &str, name: &str, namespace: Option<&str>) -> bool {
        matches_hosted_custom_tool_call(&self.prefixes, &self.names, call_id, name, namespace)
    }

    pub fn matches_item(&self, item: &ResponseItem) -> bool {
        matches_hosted_custom_tool_call_item(&self.prefixes, &self.names, item)
    }
}

impl ModelProviderInfo {
    pub fn hosted_tool_call_matcher(&self) -> HostedToolCallMatcher {
        HostedToolCallMatcher::new(
            self.hosted_tool_call_prefixes.clone(),
            self.hosted_tool_names.clone(),
        )
    }

    pub fn is_hosted_custom_tool_call(
        &self,
        call_id: &str,
        name: &str,
        namespace: Option<&str>,
    ) -> bool {
        matches_hosted_custom_tool_call(
            &self.hosted_tool_call_prefixes,
            &self.hosted_tool_names,
            call_id,
            name,
            namespace,
        )
    }

    pub fn is_hosted_custom_tool_call_item(&self, item: &ResponseItem) -> bool {
        matches_hosted_custom_tool_call_item(
            &self.hosted_tool_call_prefixes,
            &self.hosted_tool_names,
            item,
        )
    }
}
