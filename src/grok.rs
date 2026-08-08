use std::time::Duration;

use eventsource_stream::Eventsource;
use futures_util::StreamExt;
use reqwest::{Client, Response, header::CONTENT_TYPE};
use serde::Serialize;
use serde_json::Value;

use crate::config::{ApiFormat, UpstreamConfig};

/// 利用者からモデルを変更できないよう、各検索機能の上流モデルを固定します。
const FAST_MODEL: &str = "grok-4.3";
/// 深度検索と究極検索で共用します。
const DEEP_MODEL: &str = "grok-4.20-multi-agent-0309";
/// 究極検索でのみ最大限の推論を要求します。
const ULTRA_REASONING_EFFORT: &str = "xhigh";
/// 上流モデルへの固定指示は英語で記述し、利用者の入力言語で回答させます。
const FAST_INSTRUCTIONS: &str = "You are a web search assistant. Search the web and X when relevant. Provide an accurate and concise answer in the same language as the user's query. Include direct source URLs whenever available. Never fabricate sources, URLs, or claims. If no reliable information is available, clearly state that no reliable information was found and briefly suggest how to verify it. Never return an empty response.";
const DEEP_INSTRUCTIONS: &str = "You are a deep research assistant. Conduct a comprehensive search of the web and X when relevant, and cross-check multiple reliable sources. Provide a detailed and well-structured answer in the same language as the user's query. Include direct source URLs whenever available, distinguish confirmed facts from uncertainty, and never fabricate sources, URLs, or claims. If no reliable information is available, clearly state that no reliable information was found and explain how to verify it. Never return an empty response.";
const ULTRA_INSTRUCTIONS: &str = "You are an exhaustive research assistant operating at maximum rigor. Search the web and X extensively, gather every relevant primary source, and cross-verify each material claim against at least two independent reliable sources. Decompose the question into sub-questions, investigate each one, and reconcile conflicting evidence explicitly. Provide an exhaustive, well-structured answer in the same language as the user's query, organized with clear headings, and state the reasoning behind each conclusion. Include direct source URLs for every substantive claim, note publication dates when recency matters, and clearly separate confirmed facts, contested points, and your own inference. Never fabricate sources, URLs, or claims, and never present speculation as fact. If no reliable information is available, clearly state that no reliable information was found and explain in detail how to verify it. Never return an empty response.";
/// 互換APIがstream指定を無視してJSONを返す場合にも備え、両方の形式を受け付けます。
const ACCEPT_VALUE: &str = "text/event-stream, application/json";
/// 究極検索の推論が長時間に及ぶため、十分な待機時間を確保します。
const REQUEST_TIMEOUT: Duration = Duration::from_secs(600);
const ERROR_BODY_LIMIT: usize = 1_000;

/// Grok互換APIへのリクエストを担当するクライアントです。
pub struct GrokClient {
    client: Client,
    standard: UpstreamConfig,
    deep: UpstreamConfig,
}

impl GrokClient {
    pub fn new(standard: UpstreamConfig, deep: UpstreamConfig) -> Result<Self, reqwest::Error> {
        let client = Client::builder().timeout(REQUEST_TIMEOUT).build()?;

        Ok(Self {
            client,
            standard,
            deep,
        })
    }

    /// Grok 4.3に検索クエリを送り、回答本文のみを返します。
    pub async fn search(&self, query: &str) -> Result<String, String> {
        self.search_with_model(&self.standard, FAST_MODEL, FAST_INSTRUCTIONS, query, None)
            .await
    }

    /// Grok 4.20 Multi-Agent 0309で詳細な調査を実行します。
    pub async fn deep_search(&self, query: &str) -> Result<String, String> {
        self.search_with_model(&self.deep, DEEP_MODEL, DEEP_INSTRUCTIONS, query, None)
            .await
    }

    /// 深度検索と同じ上流に対し、最大限の推論を要求して徹底的な調査を実行します。
    pub async fn ultra_search(&self, query: &str) -> Result<String, String> {
        self.search_with_model(
            &self.deep,
            DEEP_MODEL,
            ULTRA_INSTRUCTIONS,
            query,
            Some(ULTRA_REASONING_EFFORT),
        )
        .await
    }

    async fn search_with_model(
        &self,
        upstream: &UpstreamConfig,
        model: &'static str,
        instructions: &'static str,
        query: &str,
        reasoning_effort: Option<&'static str>,
    ) -> Result<String, String> {
        // 上流から継続的にイベントを受信し、ゲートウェイの待機タイムアウトを防ぎます。
        let payload = build_payload(upstream.format, model, instructions, query, reasoning_effort);

        let response = self
            .client
            .post(upstream.url.as_ref())
            .bearer_auth(upstream.api_key.as_ref())
            .header("Accept", ACCEPT_VALUE)
            .json(&payload)
            .send()
            .await
            .map_err(|error| format!("Grok上流APIへのリクエストに失敗しました: {error}"))?;

        let status = response.status();
        if !status.is_success() {
            let body = response
                .text()
                .await
                .map_err(|error| format!("Grok上流APIのエラーを読み取れませんでした: {error}"))?;
            return Err(format!(
                "Grok上流APIがHTTP {status}を返しました: {}",
                truncate(&body, ERROR_BODY_LIMIT)
            ));
        }

        let is_event_stream = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.starts_with("text/event-stream"));

        if is_event_stream {
            read_streaming_answer(response, upstream.format).await
        } else {
            // 互換APIがstream指定を無視して通常JSONを返す場合にも対応します。
            let body = response.text().await.map_err(|error| {
                format!("Grok上流APIのレスポンスを読み取れませんでした: {error}")
            })?;
            parse_non_streaming_answer(&body, upstream.format)
        }
    }
}

/// 上流の形式に合わせたリクエスト本文を構築します。
fn build_payload<'a>(
    format: ApiFormat,
    model: &'static str,
    instructions: &'static str,
    query: &'a str,
    reasoning_effort: Option<&'static str>,
) -> RequestPayload<'a> {
    match format {
        ApiFormat::Responses => RequestPayload::Responses(ResponsesRequest {
            model,
            instructions,
            input: query,
            stream: true,
            reasoning_effort,
        }),
        ApiFormat::Chat => RequestPayload::Chat(ChatRequest {
            model,
            messages: vec![
                ChatMessage {
                    role: "system",
                    content: instructions,
                },
                ChatMessage {
                    role: "user",
                    content: query,
                },
            ],
            stream: true,
            reasoning_effort,
        }),
    }
}

#[derive(Debug, Serialize)]
#[serde(untagged)]
enum RequestPayload<'a> {
    Responses(ResponsesRequest<'a>),
    Chat(ChatRequest<'a>),
}

#[derive(Debug, Serialize)]
struct ResponsesRequest<'a> {
    model: &'static str,
    instructions: &'static str,
    input: &'a str,
    stream: bool,
    /// 究極検索以外では送信しません。
    #[serde(skip_serializing_if = "Option::is_none")]
    reasoning_effort: Option<&'static str>,
}

#[derive(Debug, Serialize)]
struct ChatRequest<'a> {
    model: &'static str,
    messages: Vec<ChatMessage<'a>>,
    stream: bool,
    /// 究極検索以外では送信しません。
    #[serde(skip_serializing_if = "Option::is_none")]
    reasoning_effort: Option<&'static str>,
}

#[derive(Debug, Serialize)]
struct ChatMessage<'a> {
    role: &'static str,
    content: &'a str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StreamControl {
    Continue,
    Completed,
}

async fn read_streaming_answer(response: Response, format: ApiFormat) -> Result<String, String> {
    let mut events = response.bytes_stream().eventsource();
    let mut answer = String::new();

    while let Some(event) = events.next().await {
        let event =
            event.map_err(|error| format!("Grok上流APIのストリームが中断されました: {error}"))?;

        if process_stream_event(format, &event.event, &event.data, &mut answer)?
            == StreamControl::Completed
        {
            break;
        }
    }

    if answer.trim().is_empty() {
        Err("Grok上流APIのストリームから回答を取得できませんでした".to_owned())
    } else {
        Ok(answer)
    }
}

fn process_stream_event(
    format: ApiFormat,
    event_name: &str,
    data: &str,
    answer: &mut String,
) -> Result<StreamControl, String> {
    let data = data.trim();
    if data.is_empty() {
        return Ok(StreamControl::Continue);
    }
    if data == "[DONE]" {
        return Ok(StreamControl::Completed);
    }

    let event: Value = match serde_json::from_str(data) {
        Ok(event) => event,
        Err(_) if event_name == "ping" => return Ok(StreamControl::Continue),
        Err(error) => {
            return Err(format!(
                "Grok上流APIが不正なストリームイベントを返しました: {error}"
            ));
        }
    };

    match format {
        ApiFormat::Responses => process_responses_event(&event, event_name, answer),
        ApiFormat::Chat => process_chat_event(&event, answer),
    }
}

fn process_responses_event(
    event: &Value,
    event_name: &str,
    answer: &mut String,
) -> Result<StreamControl, String> {
    let event_type = event
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or(event_name);

    match event_type {
        "response.output_text.delta" => {
            if let Some(delta) = event.get("delta").and_then(Value::as_str) {
                answer.push_str(delta);
            }
            Ok(StreamControl::Continue)
        }
        "response.output_text.done" => {
            // 出力パートごとに送信されるため、deltaを受信済みの場合は上書きしません。
            if answer.is_empty()
                && let Some(text) = event.get("text").and_then(Value::as_str)
            {
                answer.push_str(text);
            }
            Ok(StreamControl::Continue)
        }
        "response.completed" => {
            let response = event.get("response").unwrap_or(event);
            if let Some(text) = extract_responses_answer(response) {
                *answer = text;
            }
            Ok(StreamControl::Completed)
        }
        "response.failed" | "response.incomplete" | "error" => Err(format!(
            "Grok上流APIの処理が完了しませんでした: {}",
            extract_stream_error(event)
        )),
        _ => Ok(StreamControl::Continue),
    }
}

fn process_chat_event(event: &Value, answer: &mut String) -> Result<StreamControl, String> {
    // `"error": null` を返す実装があるため、null以外のみエラーとして扱います。
    if event.get("error").is_some_and(|error| !error.is_null()) {
        return Err(format!(
            "Grok上流APIの処理が完了しませんでした: {}",
            extract_stream_error(event)
        ));
    }

    let Some(choice) = event
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|choices| choices.first())
    else {
        return Ok(StreamControl::Continue);
    };

    if let Some(delta) = choice.pointer("/delta/content").and_then(Value::as_str) {
        answer.push_str(delta);
    }

    let finished = choice
        .get("finish_reason")
        .is_some_and(|reason| !reason.is_null());

    Ok(if finished {
        StreamControl::Completed
    } else {
        StreamControl::Continue
    })
}

fn parse_non_streaming_answer(body: &str, format: ApiFormat) -> Result<String, String> {
    let response: Value = serde_json::from_str(body)
        .map_err(|error| format!("Grok上流APIが不正なJSONを返しました: {error}"))?;

    let answer = match format {
        ApiFormat::Responses => extract_responses_answer(&response),
        ApiFormat::Chat => extract_chat_answer(&response),
    };

    answer.ok_or_else(|| {
        format!(
            "Grok上流APIのレスポンスから回答を取得できませんでした: {}",
            truncate(body, ERROR_BODY_LIMIT)
        )
    })
}

fn extract_responses_answer(response: &Value) -> Option<String> {
    // 一部のOpenAI互換APIが返すトップレベルの補助フィールドにも対応します。
    if let Some(text) = response.get("output_text").and_then(Value::as_str)
        && !text.trim().is_empty()
    {
        return Some(text.to_owned());
    }

    let text = response
        .get("output")?
        .as_array()?
        .iter()
        .filter_map(|output| output.get("content").and_then(Value::as_array))
        .flatten()
        .filter(|part| {
            part.get("type")
                .and_then(Value::as_str)
                .is_none_or(|kind| kind == "output_text")
        })
        .filter_map(|part| part.get("text").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join("\n");

    (!text.trim().is_empty()).then_some(text)
}

fn extract_chat_answer(response: &Value) -> Option<String> {
    let text = response
        .pointer("/choices/0/message/content")
        .and_then(Value::as_str)?;

    (!text.trim().is_empty()).then(|| text.to_owned())
}

fn extract_stream_error(event: &Value) -> String {
    [
        "/response/error/message",
        "/response/incomplete_details/reason",
        "/error/message",
        "/message",
    ]
    .into_iter()
    .find_map(|pointer| event.pointer(pointer).and_then(Value::as_str))
    .unwrap_or("詳細不明")
    .to_owned()
}

fn truncate(text: &str, max_chars: usize) -> String {
    let mut chars = text.chars();
    let shortened: String = chars.by_ref().take(max_chars).collect();

    if chars.next().is_some() {
        format!("{shortened}…")
    } else {
        shortened
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use axum::{Json, Router, http::HeaderMap, routing::post};
    use serde_json::{Value, json};

    use super::{
        ApiFormat, DEEP_INSTRUCTIONS, DEEP_MODEL, FAST_INSTRUCTIONS, FAST_MODEL, GrokClient,
        StreamControl, ULTRA_INSTRUCTIONS, ULTRA_REASONING_EFFORT, UpstreamConfig, build_payload,
        extract_chat_answer, extract_responses_answer, process_stream_event, truncate,
    };

    fn upstream(url: String, key: &str, format: ApiFormat) -> UpstreamConfig {
        UpstreamConfig {
            api_key: Arc::from(key),
            url: Arc::from(url),
            format,
        }
    }

    #[test]
    fn upstream_models_are_fixed() {
        assert_eq!(FAST_MODEL, "grok-4.3");
        assert_eq!(DEEP_MODEL, "grok-4.20-multi-agent-0309");
    }

    #[test]
    fn builds_responses_payload() {
        let payload = build_payload(
            ApiFormat::Responses,
            FAST_MODEL,
            FAST_INSTRUCTIONS,
            "問い合わせ",
            None,
        );
        let json = serde_json::to_value(&payload).unwrap();

        assert_eq!(json["model"], FAST_MODEL);
        assert_eq!(json["instructions"], FAST_INSTRUCTIONS);
        assert_eq!(json["input"], "問い合わせ");
        assert_eq!(json["stream"], true);
        assert!(json.get("reasoning_effort").is_none());
    }

    #[test]
    fn builds_chat_payload() {
        let payload = build_payload(
            ApiFormat::Chat,
            DEEP_MODEL,
            ULTRA_INSTRUCTIONS,
            "問い合わせ",
            Some(ULTRA_REASONING_EFFORT),
        );
        let json = serde_json::to_value(&payload).unwrap();

        assert_eq!(json["model"], DEEP_MODEL);
        assert_eq!(json["messages"][0]["role"], "system");
        assert_eq!(json["messages"][0]["content"], ULTRA_INSTRUCTIONS);
        assert_eq!(json["messages"][1]["role"], "user");
        assert_eq!(json["messages"][1]["content"], "問い合わせ");
        assert_eq!(json["stream"], true);
        assert_eq!(json["reasoning_effort"], ULTRA_REASONING_EFFORT);
        assert!(json.get("input").is_none());
    }

    #[test]
    fn standard_search_omits_reasoning_effort() {
        let payload = build_payload(ApiFormat::Chat, FAST_MODEL, FAST_INSTRUCTIONS, "q", None);
        let json = serde_json::to_value(&payload).unwrap();

        assert!(json.get("reasoning_effort").is_none());
    }

    #[test]
    fn deep_search_omits_reasoning_effort() {
        let payload = build_payload(ApiFormat::Chat, DEEP_MODEL, DEEP_INSTRUCTIONS, "q", None);
        let json = serde_json::to_value(&payload).unwrap();

        assert!(json.get("reasoning_effort").is_none());
    }

    #[tokio::test]
    async fn search_modes_use_expected_upstreams() {
        let requests = Arc::new(Mutex::new(Vec::<(String, Value)>::new()));
        let standard_requests = Arc::clone(&requests);
        let deep_requests = Arc::clone(&requests);
        let app = Router::new()
            .route(
                "/standard",
                post(move |headers: HeaderMap, Json(payload): Json<Value>| {
                    let requests = Arc::clone(&standard_requests);
                    async move {
                        requests.lock().unwrap().push((
                            headers["authorization"].to_str().unwrap().to_owned(),
                            payload,
                        ));
                        Json(json!({ "output_text": "standard answer" }))
                    }
                }),
            )
            .route(
                "/deep",
                post(move |headers: HeaderMap, Json(payload): Json<Value>| {
                    let requests = Arc::clone(&deep_requests);
                    async move {
                        requests.lock().unwrap().push((
                            headers["authorization"].to_str().unwrap().to_owned(),
                            payload,
                        ));
                        Json(json!({
                            "choices": [{
                                "finish_reason": "stop",
                                "message": { "role": "assistant", "content": "deep answer" }
                            }]
                        }))
                    }
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let client = GrokClient::new(
            upstream(
                format!("http://{address}/standard"),
                "standard-key",
                ApiFormat::Responses,
            ),
            upstream(
                format!("http://{address}/deep"),
                "deep-key",
                ApiFormat::Chat,
            ),
        )
        .unwrap();

        let standard_answer = client.search("standard query").await;
        let deep_answer = client.deep_search("deep query").await;
        let ultra_answer = client.ultra_search("ultra query").await;
        server.abort();

        assert_eq!(standard_answer.as_deref(), Ok("standard answer"));
        assert_eq!(deep_answer.as_deref(), Ok("deep answer"));
        assert_eq!(ultra_answer.as_deref(), Ok("deep answer"));
        let requests = requests.lock().unwrap();
        assert_eq!(requests.len(), 3);
        assert_eq!(requests[0].0, "Bearer standard-key");
        assert_eq!(requests[0].1["input"], "standard query");
        assert!(requests[0].1.get("reasoning_effort").is_none());
        assert_eq!(requests[1].0, "Bearer deep-key");
        assert_eq!(requests[1].1["messages"][0]["content"], DEEP_INSTRUCTIONS);
        assert_eq!(requests[1].1["messages"][1]["content"], "deep query");
        assert!(requests[1].1.get("reasoning_effort").is_none());
        assert_eq!(requests[2].0, "Bearer deep-key");
        assert_eq!(requests[2].1["messages"][0]["content"], ULTRA_INSTRUCTIONS);
        assert_eq!(requests[2].1["messages"][1]["content"], "ultra query");
        assert_eq!(requests[2].1["reasoning_effort"], ULTRA_REASONING_EFFORT);
    }

    #[test]
    fn upstream_instructions_are_english() {
        for instructions in [FAST_INSTRUCTIONS, DEEP_INSTRUCTIONS, ULTRA_INSTRUCTIONS] {
            assert!(instructions.is_ascii());
            assert!(instructions.contains("same language as the user's query"));
            assert!(instructions.contains("Never return an empty response"));
        }
    }

    #[test]
    fn ultra_instructions_are_strictest() {
        assert!(ULTRA_INSTRUCTIONS.len() > DEEP_INSTRUCTIONS.len());
        assert!(ULTRA_INSTRUCTIONS.contains("cross-verify"));
    }

    #[test]
    fn extracts_output_text() {
        let response = json!({
            "output": [{
                "type": "message",
                "content": [{
                    "type": "output_text",
                    "text": "検索結果です"
                }]
            }]
        });

        assert_eq!(
            extract_responses_answer(&response).as_deref(),
            Some("検索結果です")
        );
    }

    #[test]
    fn combines_multiple_output_text_parts() {
        let response = json!({
            "output": [
                {
                    "type": "message",
                    "content": [{ "type": "output_text", "text": "1行目" }]
                },
                {
                    "type": "message",
                    "content": [{ "type": "output_text", "text": "2行目" }]
                }
            ]
        });

        assert_eq!(
            extract_responses_answer(&response).as_deref(),
            Some("1行目\n2行目")
        );
    }

    #[test]
    fn extracts_top_level_output_text() {
        let response = json!({ "output_text": "検索結果です" });

        assert_eq!(
            extract_responses_answer(&response).as_deref(),
            Some("検索結果です")
        );
    }

    #[test]
    fn extracts_chat_message_content() {
        let response = json!({
            "choices": [{ "message": { "content": "検索結果です" } }]
        });

        assert_eq!(
            extract_chat_answer(&response).as_deref(),
            Some("検索結果です")
        );
    }

    #[test]
    fn appends_streaming_deltas() {
        let mut answer = String::new();

        let first = process_stream_event(
            ApiFormat::Responses,
            "response.output_text.delta",
            r#"{"type":"response.output_text.delta","delta":"検索"}"#,
            &mut answer,
        );
        let second = process_stream_event(
            ApiFormat::Responses,
            "response.output_text.delta",
            r#"{"type":"response.output_text.delta","delta":"結果"}"#,
            &mut answer,
        );

        assert_eq!(first, Ok(StreamControl::Continue));
        assert_eq!(second, Ok(StreamControl::Continue));
        assert_eq!(answer, "検索結果");
    }

    #[test]
    fn output_text_done_keeps_accumulated_deltas() {
        let mut answer = String::new();

        process_stream_event(
            ApiFormat::Responses,
            "response.output_text.delta",
            r#"{"type":"response.output_text.delta","delta":"1つ目"}"#,
            &mut answer,
        )
        .unwrap();
        process_stream_event(
            ApiFormat::Responses,
            "response.output_text.done",
            r#"{"type":"response.output_text.done","text":"1つ目"}"#,
            &mut answer,
        )
        .unwrap();
        process_stream_event(
            ApiFormat::Responses,
            "response.output_text.delta",
            r#"{"type":"response.output_text.delta","delta":"2つ目"}"#,
            &mut answer,
        )
        .unwrap();
        process_stream_event(
            ApiFormat::Responses,
            "response.output_text.done",
            r#"{"type":"response.output_text.done","text":"2つ目"}"#,
            &mut answer,
        )
        .unwrap();

        assert_eq!(answer, "1つ目2つ目");
    }

    #[test]
    fn output_text_done_fills_answer_without_deltas() {
        let mut answer = String::new();

        let result = process_stream_event(
            ApiFormat::Responses,
            "response.output_text.done",
            r#"{"type":"response.output_text.done","text":"回答のみ"}"#,
            &mut answer,
        );

        assert_eq!(result, Ok(StreamControl::Continue));
        assert_eq!(answer, "回答のみ");
    }

    #[test]
    fn appends_chat_streaming_deltas() {
        let mut answer = String::new();

        let first = process_stream_event(
            ApiFormat::Chat,
            "message",
            r#"{"choices":[{"delta":{"content":"検索"},"finish_reason":null}]}"#,
            &mut answer,
        );
        let second = process_stream_event(
            ApiFormat::Chat,
            "message",
            r#"{"choices":[{"delta":{"content":"結果"},"finish_reason":null}]}"#,
            &mut answer,
        );
        let last = process_stream_event(
            ApiFormat::Chat,
            "message",
            r#"{"choices":[{"delta":{},"finish_reason":"stop"}]}"#,
            &mut answer,
        );

        assert_eq!(first, Ok(StreamControl::Continue));
        assert_eq!(second, Ok(StreamControl::Continue));
        assert_eq!(last, Ok(StreamControl::Completed));
        assert_eq!(answer, "検索結果");
    }

    #[test]
    fn ignores_null_error_field_in_chat_stream() {
        let mut answer = String::new();

        let result = process_stream_event(
            ApiFormat::Chat,
            "message",
            r#"{"error":null,"choices":[{"delta":{"content":"検索結果"},"finish_reason":null}]}"#,
            &mut answer,
        );

        assert_eq!(result, Ok(StreamControl::Continue));
        assert_eq!(answer, "検索結果");
    }

    #[test]
    fn reports_chat_stream_error() {
        let mut answer = String::new();

        let result = process_stream_event(
            ApiFormat::Chat,
            "message",
            r#"{"error":{"message":"レート制限"}}"#,
            &mut answer,
        );

        assert_eq!(
            result,
            Err("Grok上流APIの処理が完了しませんでした: レート制限".to_owned())
        );
    }

    #[test]
    fn uses_completed_response_as_final_answer() {
        let mut answer = "途中".to_owned();
        let data = json!({
            "type": "response.completed",
            "response": {
                "output": [{
                    "type": "message",
                    "content": [{ "type": "output_text", "text": "最終回答" }]
                }]
            }
        })
        .to_string();

        let result = process_stream_event(
            ApiFormat::Responses,
            "response.completed",
            &data,
            &mut answer,
        );

        assert_eq!(result, Ok(StreamControl::Completed));
        assert_eq!(answer, "最終回答");
    }

    #[test]
    fn truncates_by_character_count() {
        assert_eq!(truncate("あいうえお", 3), "あいう…");
        assert_eq!(truncate("あいう", 3), "あいう");
    }
}
