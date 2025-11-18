
use crate::conversation::{ConversationState, Message};
use crate::settings::{ProviderSettings, Settings};
use futures_util::StreamExt;
use regex::Regex;
use serde_json::Value;
use std::env;
use std::fs;
use std::io::{self, Write};
use std::path::PathBuf;
use std::time::Duration;
use dialoguer::Confirm;

pub async fn perform_request(
    input: Value,
    conversation_state: &mut ConversationState,
    transcript_path: &PathBuf,
    settings: &Settings,
    provider_settings: &ProviderSettings,
    suppress_stream_print: bool,
) {
    conversation_state.messages.push(Message {
        role: "user".to_string(),
        content: input,
    });

    let request_body_json: Value;

    if provider_settings.model.contains("gemini-") {
        let gemini_messages: Vec<Value> = conversation_state
            .messages
            .iter()
            .map(|msg| {
                let role = match msg.role.as_str() {
                    "system" => "user",
                    "assistant" => "model",
                    _ => msg.role.as_str(),
                };
                if msg.content.is_array() {
                     let parts: Vec<Value> = msg.content.as_array().unwrap().iter().map(|part_val| {
                        let part_type = part_val.get("type").and_then(|t| t.as_str()).unwrap_or("");
                        if part_type == "text" {
                            serde_json::json!({"text": part_val.get("text").unwrap_or(&Value::Null).as_str().unwrap_or("")})
                        } else if part_type == "image_url" {
                            let image_url_obj = part_val.get("image_url").unwrap();
                            let image_data_url = image_url_obj.get("url").unwrap().as_str().unwrap();
                            let parts_split: Vec<&str> = image_data_url.splitn(2, ',').collect();
                            let mime_type_part: Vec<&str> = parts_split.get(0).unwrap_or(&"data:image/png;base64").splitn(2, ':').collect::<Vec<&str>>().get(1).unwrap_or(&"image/png;base64").split(';').collect();
                            let mime_type = mime_type_part[0];
                            let base64_data = parts_split.get(1).unwrap_or(&"");

                            serde_json::json!({
                                "inlineData": {
                                    "mimeType": mime_type,
                                    "data": base64_data
                                }
                            })
                        } else {
                            Value::Null
                        }
                    }).filter(|p| !p.is_null()).collect();
                    serde_json::json!({"role": role, "parts": parts})
                } else {
                    serde_json::json!({
                        "role": role,
                        "parts": [{"text": msg.content.as_str().unwrap_or("")}]
                    })
                }
            })
            .collect();

        request_body_json = serde_json::json!({
            "contents": gemini_messages,
            "generationConfig": {
                "maxOutputTokens": settings.max_tokens,
                "temperature": settings.temperature,
            }
        });
    } else {
        let mut body = serde_json::json!({
            "messages": conversation_state.messages,
            "model": conversation_state.model,
            "stream": true
        });
        // TODO: Move API exceptions elsewhere
        // o* mini want other settings
        let pat = Regex::new(r"o\d-mini|gpt-5").unwrap();
        if !pat.is_match(provider_settings.model.as_str()) || !provider_settings.host.contains("openai")
        {
            body["max_tokens"] = serde_json::json!(settings.max_tokens);
            body["temperature"] = serde_json::json!(settings.temperature);
        }
        if settings.provider != "mistral" {
            body["user"] = serde_json::json!(whoami::username());
        }

        request_body_json = body;
    }

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(300))
        .build()
        .unwrap();

    let api_key = env::var(&provider_settings.api_key_variable).unwrap();

    let endpoint = if provider_settings.model.contains("gemini-") {
        format!(
            "/v1beta/models/{}:streamGenerateContent?alt=sse",
            provider_settings.model
        )
    } else {
        provider_settings.endpoint.clone()
    };

    let request_builder = client
        .post(format!("https://{}{}", provider_settings.host, endpoint))
        .header("Content-Type", "application/json")
        .json(&request_body_json);

    let res = if provider_settings.model.contains("gemini-") {
        request_builder.query(&[("key", api_key)]).send().await
    } else {
        request_builder
            .header("Authorization", format!("Bearer {}", api_key))
            .send()
            .await
    };

    match res {
        Ok(response) => {
            if response.status().is_success() {
                let (assistant_role, assistant_content_full) =
                    handle_stream(response, provider_settings, suppress_stream_print).await;

                if !suppress_stream_print && !assistant_content_full.is_empty() {
                    println!();
                }

                let assistant_message = Message {
                    role: assistant_role,
                    content: Value::String(assistant_content_full),
                };
                conversation_state.messages.push(assistant_message);

                let mut truncated_state = conversation_state.clone();
                if truncated_state.messages.len() >= 2 {
                    let indices = [
                        truncated_state.messages.len() - 2,
                        truncated_state.messages.len() - 1,
                    ];
                    let mut should_truncate = false;
                    for &i in &indices {
                        if let Some(text) = truncated_state.messages[i].content.as_str() {
                            if text.len() > 5000 {
                                should_truncate = true;
                                break;
                            }
                        }
                    }
                    if should_truncate {
                        if Confirm::new()
                            .with_prompt(
                                "Your last message or assistant response was too large, recommend truncating history for this session?",
                            )
                            .default(true)
                            .interact()
                            .unwrap_or(false)
                        {
                            for &i in &indices {
                                if let Some(text) = truncated_state.messages[i].content.as_str() {
                                    if text.len() > 5000 {
                                        truncated_state.messages[i].content =
                                            serde_json::json!(format!("{} [truncated]", &text[..5000]));
                                    }
                                }
                            }
                        }
                    }
                }
                let conversation_json = serde_json::to_string_pretty(&truncated_state).unwrap();
                fs::write(transcript_path, conversation_json)
                    .expect("Unable to write transcript file");
            } else {
                let status = response.status();
                let error_body = response
                    .text()
                    .await
                    .unwrap_or_else(|_| "Failed to read error body".to_string());
                eprintln!("API Error: {} - {}", status, error_body);
                eprintln!(
                    "Request body: {}",
                    serde_json::to_string_pretty(&request_body_json).unwrap_or_default()
                );
            }
        }
        Err(e) => {
            eprintln!("HTTP request error: {}", e);
        }
    }
}

async fn handle_stream(
    response: reqwest::Response,
    provider_settings: &ProviderSettings,
    suppress_print: bool,
) -> (String, String) {
    let mut stream = response.bytes_stream(); // Requires "stream" feature for reqwest
    let mut full_content = String::new();
    let mut role = if provider_settings.model.contains("gemini-") {
        "model".to_string()
    } else {
        String::new()
    };

    let mut buffer = String::new();

    while let Some(item) = stream.next().await {
        match item {
            Ok(chunk) => {
                // chunk here is of type bytes::Bytes
                buffer.push_str(&String::from_utf8_lossy(&chunk));

                loop {
                    if let Some(newline_idx) = buffer.find('\n') {
                        let line = buffer[..newline_idx + 1].to_string();
                        buffer.replace_range(..newline_idx + 1, "");

                        if line.starts_with("data: ") {
                            let json_str = line["data: ".len()..].trim();
                            if json_str == "[DONE]" {
                                if !suppress_print {
                                    io::stdout().flush().unwrap();
                                }
                                if role.is_empty() {
                                    role = if provider_settings.model.contains("gemini-") {
                                        "model".to_string()
                                    } else {
                                        "assistant".to_string()
                                    };
                                }
                                return (role, full_content);
                            }
                            if !json_str.is_empty() {
                                match serde_json::from_str::<Value>(json_str) {
                                    Ok(value) => {
                                        if provider_settings.model.contains("gemini-") {
                                            if let Some(candidates) =
                                                value.get("candidates").and_then(|c| c.as_array())
                                            {
                                                for candidate in candidates {
                                                    if let Some(content) = candidate.get("content") {
                                                        if let Some(parts) = content
                                                            .get("parts")
                                                            .and_then(|p| p.as_array())
                                                        {
                                                            for part in parts {
                                                                if let Some(text_delta) = part
                                                                    .get("text")
                                                                    .and_then(|t| t.as_str())
                                                                {
                                                                    if !suppress_print {
                                                                        print!("{}", text_delta);
                                                                        io::stdout().flush().unwrap();
                                                                    }
                                                                    full_content
                                                                        .push_str(text_delta);
                                                                }
                                                            }
                                                        }
                                                    }
                                                }
                                            }
                                        } else {
                                            if let Some(choices) =
                                                value.get("choices").and_then(|c| c.as_array())
                                            {
                                                if let Some(choice) = choices.get(0) {
                                                    if let Some(delta) = choice.get("delta") {
                                                        if role.is_empty() {
                                                            if let Some(r) = delta
                                                                .get("role")
                                                                .and_then(|r| r.as_str())
                                                            {
                                                                role = r.to_string();
                                                            }
                                                        }
                                                        if let Some(content_delta) = delta
                                                            .get("content")
                                                            .and_then(|c| c.as_str())
                                                        {
                                                            if !suppress_print {
                                                                print!("{}", content_delta);
                                                                io::stdout().flush().unwrap();
                                                            }
                                                            full_content.push_str(content_delta);
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                    Err(_e) => { /* eprintln!("Stream JSON parse error: {}", e); eprintln!("Problematic JSON: {}", json_str); */
                                    }
                                }
                            }
                        }
                    } else {
                        break;
                    }
                }
            }
            Err(e) => {
                if !suppress_print {
                    eprintln!("\nStream error: {}", e);
                }
                break;
            }
        }
    }

    if !suppress_print {
        io::stdout().flush().unwrap();
    }
    if role.is_empty() {
        role = "assistant".to_string();
    }
    (role, full_content)
}

