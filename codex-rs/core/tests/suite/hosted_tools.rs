#![allow(clippy::unwrap_used)]

use anyhow::Result;
use codex_core::TurnInputRequest;
use codex_protocol::models::ContentItem;
use codex_protocol::models::ResponseItem;
use codex_protocol::protocol::EventMsg;
use codex_protocol::user_input::UserInput;
use core_test_support::responses;
use core_test_support::responses::start_mock_server;
use core_test_support::skip_if_no_network;
use core_test_support::test_codex::test_codex;
use pretty_assertions::assert_eq;
use serde_json::Value;
use serde_json::json;

fn ev_assistant_message_with_annotations(id: &str, text: &str, annotations: Value) -> Value {
    json!({
        "type": "response.output_item.done",
        "item": {
            "type": "message",
            "role": "assistant",
            "id": id,
            "content": [{
                "type": "output_text",
                "text": text,
                "annotations": annotations,
            }],
        }
    })
}

async fn collect_turn_events(codex: &codex_core::CodexThread) -> Vec<EventMsg> {
    let mut events = Vec::new();
    loop {
        let event = tokio::time::timeout(std::time::Duration::from_secs(10), codex.next_event())
            .await
            .expect("timed out waiting for turn event")
            .expect("event stream ended before turn completion");
        let is_complete = matches!(event.msg, EventMsg::TurnComplete(_));
        events.push(event.msg);
        if is_complete {
            return events;
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn default_openai_profile_is_unchanged_without_hosted_tools() -> Result<()> {
    skip_if_no_network!(Ok(()));

    let server = start_mock_server().await;
    let resp_mock = responses::mount_sse_sequence(
        &server,
        vec![
            responses::sse(vec![
                responses::ev_response_created("resp-1"),
                responses::ev_custom_tool_call("xs_1", "x_keyword_search", r#"{"query":"codex"}"#),
                responses::ev_completed("resp-1"),
            ]),
            responses::sse(vec![
                responses::ev_response_created("resp-2"),
                responses::ev_assistant_message("msg-2", "handled unknown tool"),
                responses::ev_completed("resp-2"),
            ]),
        ],
    )
    .await;

    let test = test_codex().with_model("gpt-5.5").build(&server).await?;
    test.submit_turn("search x for codex").await?;

    let requests = resp_mock.requests();
    assert_eq!(requests.len(), 2);

    let first_tools = requests[0].body_json()["tools"]
        .as_array()
        .expect("tools array")
        .clone();
    assert!(
        !first_tools
            .iter()
            .any(|tool| tool.get("type").and_then(Value::as_str) == Some("x_search")),
        "default OpenAI profile must not emit x_search"
    );

    let output = requests[1].custom_tool_call_output("xs_1");
    let output_text = output["output"].as_str().unwrap_or_default();
    assert!(
        output_text.contains("unsupported call"),
        "default OpenAI profile must preserve local unknown custom tool dispatch, got: {output_text}"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn configured_xai_profile_sends_hosted_tools_skips_local_dispatch_and_coexists_with_local_tools()
-> Result<()> {
    skip_if_no_network!(Ok(()));

    let server = start_mock_server().await;
    let citation = json!([{
        "type": "url_citation",
        "url": "https://x.com/example/status/123",
        "title": "Example status",
    }]);
    let resp_mock = responses::mount_sse_sequence(
        &server,
        vec![
            responses::sse(vec![
                responses::ev_response_created("resp-1"),
                responses::ev_custom_tool_call(
                    "xs_1",
                    "x_keyword_search",
                    r#"{"query":"codex hosted tools"}"#,
                ),
                ev_assistant_message_with_annotations(
                    "msg-1",
                    "Found an X post.",
                    citation.clone(),
                ),
                responses::ev_completed("resp-1"),
            ]),
            responses::sse(vec![
                responses::ev_response_created("resp-2"),
                responses::ev_custom_tool_call(
                    "xs_2",
                    "x_thread_fetch",
                    r#"{"post_id":"123"}"#,
                ),
                responses::ev_exec_command_call("call-local-1", "echo hosted-coexist-ok"),
                responses::ev_completed("resp-2"),
            ]),
            responses::sse(vec![
                responses::ev_response_created("resp-3"),
                responses::ev_assistant_message("msg-3", "Combined hosted and local result."),
                responses::ev_completed("resp-3"),
            ]),
        ],
    )
    .await;

    let xai_config = r#"
model_provider = "xai"
web_search = "disabled"

[model_providers.xai]
name = "xAI"
env_key = "PATH"
wire_api = "responses"
hosted_tool_call_prefixes = ["xs_"]
hosted_tool_names = ["x_keyword_search", "x_semantic_search", "x_user_search", "x_thread_fetch"]

[[model_providers.xai.hosted_tools]]
type = "x_search"
enable_image_understanding = true
"#;

    let test = test_codex()
        .with_model("gpt-5.5")
        .with_pre_build_hook(move |codex_home| {
            std::fs::write(codex_home.join("config.toml"), xai_config)
                .expect("write xAI config.toml");
        })
        .with_config(|config| {
            let base_url = config.model_provider.base_url.clone();
            let mut xai = config
                .model_providers
                .get("xai")
                .expect("xai provider should load from config.toml")
                .clone();
            xai.base_url = base_url;
            config.model_provider_id = "xai".to_string();
            config.model_provider = xai;
        })
        .build(&server)
        .await?;

    test.codex
        .start_or_steer_turn(TurnInputRequest::user_input(vec![UserInput::Text {
            text: "Check X for codex hosted tools.".into(),
            text_elements: Vec::new(),
        }]))
        .await?;
    let turn1_events = collect_turn_events(&test.codex).await;

    assert!(
        turn1_events.iter().any(|event| matches!(
            event,
            EventMsg::RawResponseItem(raw)
                if matches!(
                    &raw.item,
                    ResponseItem::CustomToolCall { call_id, name, .. }
                        if call_id == "xs_1" && name == "x_keyword_search"
                )
        )),
        "expected RawResponseItem for provider-hosted custom tool call xs_1"
    );
    assert!(
        turn1_events.iter().any(|event| matches!(
            event,
            EventMsg::RawResponseItem(raw)
                if matches!(
                    &raw.item,
                    ResponseItem::Message { role, content, .. }
                        if role == "assistant"
                            && content.iter().any(|part| matches!(
                                part,
                                ContentItem::OutputText { annotations: Some(annotations), .. }
                                    if serde_json::Value::Array(annotations.clone()) == citation
                            ))
                )
        )),
        "expected RawResponseItem preserving assistant output_text annotations"
    );

    let turn1_requests = resp_mock.requests();
    assert_eq!(
        turn1_requests.len(),
        1,
        "provider-hosted custom tool call must not trigger local tool dispatch"
    );
    let turn1_tools = turn1_requests[0].body_json()["tools"]
        .as_array()
        .expect("tools array")
        .clone();
    assert!(
        turn1_tools.contains(&json!({
            "type": "x_search",
            "enable_image_understanding": true,
        })),
        "xAI request tools[] should include configured x_search tool, got: {turn1_tools:?}"
    );

    test.submit_turn("Fetch thread and run a local command.")
        .await?;

    let requests = resp_mock.requests();
    assert_eq!(requests.len(), 3);

    let req2_custom_calls = requests[1].inputs_of_type("custom_tool_call");
    assert_eq!(req2_custom_calls.len(), 1);
    assert_eq!(req2_custom_calls[0]["call_id"], "xs_1");
    assert!(
        requests[1].inputs_of_type("custom_tool_call_output").is_empty(),
        "turn 2 history must not inject synthetic custom_tool_call_output for xs_1"
    );

    let req3_custom_call_ids: Vec<String> = requests[2]
        .inputs_of_type("custom_tool_call")
        .into_iter()
        .filter_map(|item| item["call_id"].as_str().map(str::to_owned))
        .collect();
    assert_eq!(req3_custom_call_ids, vec!["xs_1", "xs_2"]);
    assert!(
        requests[2].inputs_of_type("custom_tool_call_output").is_empty(),
        "turn 2 follow-up must not send custom_tool_call_output for hosted xs_* calls"
    );

    let local_output = requests[2]
        .function_call_output_text("call-local-1")
        .expect("local exec_command output should be sent on follow-up request");
    assert!(
        local_output.contains("hosted-coexist-ok"),
        "expected local exec_command output, got: {local_output}"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn configured_provider_explicit_web_search_replaces_default_web_search() -> Result<()> {
    skip_if_no_network!(Ok(()));

    let server = start_mock_server().await;
    let resp_mock = responses::mount_sse_once(
        &server,
        responses::sse(vec![
            responses::ev_response_created("resp-1"),
            responses::ev_completed("resp-1"),
        ]),
    )
    .await;

    let provider_config = r#"
model_provider = "xai"
web_search = "live"

[model_providers.xai]
name = "xAI"
env_key = "PATH"
wire_api = "responses"

[[model_providers.xai.hosted_tools]]
type = "web_search"

[[model_providers.xai.hosted_tools]]
type = "x_search"
"#;

    let test = test_codex()
        .with_model("gpt-5.5")
        .with_pre_build_hook(move |codex_home| {
            std::fs::write(codex_home.join("config.toml"), provider_config)
                .expect("write provider config.toml");
        })
        .with_config(|config| {
            let base_url = config.model_provider.base_url.clone();
            let mut xai = config
                .model_providers
                .get("xai")
                .expect("xai provider should load from config.toml")
                .clone();
            xai.base_url = base_url;
            config.model_provider_id = "xai".to_string();
            config.model_provider = xai;
        })
        .build(&server)
        .await?;

    test.submit_turn("hello explicit hosted web_search").await?;

    let tools = resp_mock.single_request().body_json()["tools"]
        .as_array()
        .expect("tools array")
        .clone();
    let web_search_tools: Vec<&Value> = tools
        .iter()
        .filter(|tool| tool.get("type").and_then(Value::as_str) == Some("web_search"))
        .collect();
    assert_eq!(web_search_tools, vec![&json!({"type": "web_search"})]);
    assert!(tools.contains(&json!({"type": "x_search"})));
    Ok(())
}
