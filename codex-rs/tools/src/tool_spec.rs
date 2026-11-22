use crate::FreeformTool;
use crate::JsonSchema;
use crate::LoadableToolSpec;
use crate::ResponsesApiNamespace;
use crate::ResponsesApiTool;
use crate::default_namespace_description;
use codex_protocol::DEFAULT_FUNCTION_NAMESPACE;
use codex_protocol::config_types::WebSearchContextSize;
use codex_protocol::config_types::WebSearchFilters as ConfigWebSearchFilters;
use codex_protocol::config_types::WebSearchUserLocation as ConfigWebSearchUserLocation;
use codex_protocol::config_types::WebSearchUserLocationType;
use serde::Serialize;
use serde_json::Value;
use serde_json::value::RawValue;
use std::sync::Arc;

/// When serialized as JSON, this produces a valid "Tool" in the OpenAI
/// Responses API.
#[derive(Debug, Clone, PartialEq)]
pub enum ToolSpec {
    Function(ResponsesApiTool),
    Namespace(ResponsesApiNamespace),
    ToolSearch {
        execution: String,
        description: String,
        parameters: JsonSchema,
    },
    // TODO: Understand why we get an error on web_search although the API docs
    // say it's supported.
    // https://platform.openai.com/docs/guides/tools-web-search?api-mode=responses#:~:text=%7B%20type%3A%20%22web_search%22%20%7D%2C
    // `external_web_access` distinguishes cached from live-capable search, while
    // `indexed_web_access` restricts live fetches to indexed URLs.
    // https://platform.openai.com/docs/guides/tools-web-search#live-internet-access
    WebSearch {
        external_web_access: Option<bool>,
        indexed_web_access: Option<bool>,
        filters: Option<ResponsesApiWebSearchFilters>,
        user_location: Option<ResponsesApiWebSearchUserLocation>,
        search_context_size: Option<WebSearchContextSize>,
        search_content_types: Option<Vec<String>>,
    },
    Freeform(FreeformTool),
    /// Provider-hosted tool definition serialized verbatim as its raw JSON object.
    Hosted(Value),
}

#[derive(Serialize)]
#[serde(tag = "type")]
enum TaggedToolSpec<'a> {
    #[serde(rename = "function")]
    Function(&'a ResponsesApiTool),
    #[serde(rename = "namespace")]
    Namespace(&'a ResponsesApiNamespace),
    #[serde(rename = "tool_search")]
    ToolSearch {
        execution: &'a str,
        description: &'a str,
        parameters: &'a JsonSchema,
    },
    #[serde(rename = "web_search")]
    WebSearch {
        #[serde(skip_serializing_if = "Option::is_none")]
        external_web_access: Option<bool>,
        #[serde(skip_serializing_if = "Option::is_none")]
        indexed_web_access: Option<bool>,
        #[serde(skip_serializing_if = "Option::is_none")]
        filters: Option<&'a ResponsesApiWebSearchFilters>,
        #[serde(skip_serializing_if = "Option::is_none")]
        user_location: Option<&'a ResponsesApiWebSearchUserLocation>,
        #[serde(skip_serializing_if = "Option::is_none")]
        search_context_size: Option<WebSearchContextSize>,
        #[serde(skip_serializing_if = "Option::is_none")]
        search_content_types: Option<&'a [String]>,
    },
    #[serde(rename = "custom")]
    Freeform(&'a FreeformTool),
}

impl Serialize for ToolSpec {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match self {
            ToolSpec::Function(tool) => TaggedToolSpec::Function(tool).serialize(serializer),
            ToolSpec::Namespace(namespace) => {
                TaggedToolSpec::Namespace(namespace).serialize(serializer)
            }
            ToolSpec::ToolSearch {
                execution,
                description,
                parameters,
            } => TaggedToolSpec::ToolSearch {
                execution,
                description,
                parameters,
            }
            .serialize(serializer),
            ToolSpec::WebSearch {
                external_web_access,
                indexed_web_access,
                filters,
                user_location,
                search_context_size,
                search_content_types,
            } => TaggedToolSpec::WebSearch {
                external_web_access: *external_web_access,
                indexed_web_access: *indexed_web_access,
                filters: filters.as_ref(),
                user_location: user_location.as_ref(),
                search_context_size: *search_context_size,
                search_content_types: search_content_types.as_deref(),
            }
            .serialize(serializer),
            ToolSpec::Freeform(tool) => TaggedToolSpec::Freeform(tool).serialize(serializer),
            ToolSpec::Hosted(tool) => tool.serialize(serializer),
        }
    }
}

impl ToolSpec {
    pub fn name(&self) -> &str {
        match self {
            ToolSpec::Function(tool) => tool.name.as_str(),
            ToolSpec::Namespace(namespace) => namespace.name.as_str(),
            ToolSpec::ToolSearch { .. } => "tool_search",
            ToolSpec::WebSearch { .. } => "web_search",
            ToolSpec::Freeform(tool) => tool.name.as_str(),
            ToolSpec::Hosted(tool) => tool
                .get("name")
                .and_then(Value::as_str)
                .filter(|name| !name.is_empty())
                .or_else(|| {
                    tool.get("type")
                        .and_then(Value::as_str)
                        .filter(|tool_type| !tool_type.is_empty())
                })
                .unwrap_or("hosted"),
        }
    }
}

impl From<LoadableToolSpec> for ToolSpec {
    fn from(value: LoadableToolSpec) -> Self {
        match value {
            LoadableToolSpec::Function(tool) => ToolSpec::Function(tool),
            LoadableToolSpec::Namespace(namespace) => ToolSpec::Namespace(namespace),
        }
    }
}

/// Returns JSON values that are compatible with Function Calling in the
/// Responses API:
/// https://platform.openai.com/docs/guides/function-calling?api-mode=responses
pub fn create_tools_json_for_responses_api(
    tools: &[ToolSpec],
) -> Result<Vec<Value>, serde_json::Error> {
    let mut tools_json = Vec::new();

    for tool in tools {
        let json = serde_json::to_value(tool)?;
        tools_json.push(json);
    }

    Ok(tools_json)
}

pub fn create_tools_json_for_responses_lite<'a>(
    tools: impl IntoIterator<Item = &'a ToolSpec>,
) -> Result<Vec<Value>, serde_json::Error> {
    let mut members = Vec::new();
    let mut description = None;
    let mut functions_index = None;
    let mut tools_json = Vec::new();

    for tool in tools {
        match tool {
            ToolSpec::Function(_) | ToolSpec::Freeform(_) => {
                members.push(serde_json::to_value(tool)?);
            }
            ToolSpec::Namespace(namespace) if namespace.name == DEFAULT_FUNCTION_NAMESPACE => {
                if !namespace.description.trim().is_empty() {
                    description = Some(&namespace.description);
                }
                for member in &namespace.tools {
                    members.push(serde_json::to_value(member)?);
                }
            }
            tool => {
                tools_json.push(serde_json::to_value(tool)?);
                continue;
            }
        }
        functions_index.get_or_insert(tools_json.len());
    }

    if let Some(functions_index) = functions_index
        && !members.is_empty()
    {
        // The serialized bytes feed the additional_tools ID. Keep the namespace field order.
        let mut namespace = serde_json::to_value(ToolSpec::Namespace(ResponsesApiNamespace {
            name: DEFAULT_FUNCTION_NAMESPACE.to_string(),
            description: description
                .cloned()
                .unwrap_or_else(|| default_namespace_description(DEFAULT_FUNCTION_NAMESPACE)),
            tools: Vec::new(),
        }))?;
        namespace["tools"] = Value::Array(members);
        tools_json.insert(functions_index, namespace);
    }

    Ok(tools_json)
}

/// Returns raw JSON that can be embedded directly in a Responses API request.
pub fn create_tools_raw_json_for_responses_api(
    tools: &[ToolSpec],
) -> Result<Arc<RawValue>, serde_json::Error> {
    serde_json::value::to_raw_value(tools).map(Arc::from)
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ResponsesApiWebSearchFilters {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allowed_domains: Option<Vec<String>>,
}

impl From<ConfigWebSearchFilters> for ResponsesApiWebSearchFilters {
    fn from(filters: ConfigWebSearchFilters) -> Self {
        Self {
            allowed_domains: filters.allowed_domains,
        }
    }
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ResponsesApiWebSearchUserLocation {
    #[serde(rename = "type")]
    pub r#type: WebSearchUserLocationType,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub country: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub region: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub city: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timezone: Option<String>,
}

impl From<ConfigWebSearchUserLocation> for ResponsesApiWebSearchUserLocation {
    fn from(user_location: ConfigWebSearchUserLocation) -> Self {
        Self {
            r#type: user_location.r#type,
            country: user_location.country,
            region: user_location.region,
            city: user_location.city,
            timezone: user_location.timezone,
        }
    }
}

#[cfg(test)]
#[path = "tool_spec_tests.rs"]
mod tests;
