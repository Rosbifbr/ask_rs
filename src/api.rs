
use crate::conversation::{ConversationState, Message, prompt_confirm};
use crate::settings::{ProviderSettings, Settings};
use crate::tools::ToolRegistry;
use serde_json::{json, Value};
use std::env;
use std::fs;
use std::io::{self, Read, Write};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

/// Threshold in bytes for tool output before writing to temp file
/// Outputs larger than this will be written to a file and the AI
/// will be instructed to read it in chunks using the read_file tool
const LARGE_OUTPUT_THRESHOLD: usize = 32768; // 8KB

pub fn perform_request(
    input: Value,
    conversation_state: &mut ConversationState,
    transcript_path: &PathBuf,
    settings: &Settings,
    provider_settings: &ProviderSettings,
    suppress_stream_print: bool,
    tools: Option<&ToolRegistry>,
) {
    conversation_state.messages.push(Message {
        role: "user".to_string(),
        content: input,
    });

    perform_request_loop(
        conversation_state,
        transcript_path,
        settings,
        provider_settings,
        suppress_stream_print,
        tools,
    );
}

fn perform_request_loop(
    conversation_state: &mut ConversationState,
    transcript_path: &PathBuf,
    settings: &Settings,
    provider_settings: &ProviderSettings,
    suppress_stream_print: bool,
    tools: Option<&ToolRegistry>,
) {

    let request_body_json: Value;

    if provider_settings.model.contains("gemini-") {
        let gemini_messages: Vec<Value> = conversation_state
            .messages
            .iter()
            .map(|msg| convert_message_to_gemini(msg))
            .collect();

        let mut body = json!({
            "contents": gemini_messages,
            "generationConfig": {
                "maxOutputTokens": settings.max_tokens,
                "temperature": settings.temperature,
            }
        });

        // Add tools for Gemini
        if let Some(tool_registry) = tools {
            body["tools"] = json!([tool_registry.to_gemini_format()]);
        }

        request_body_json = body;
    } else {
        // Convert messages to OpenAI format (handle tool calls/results)
        let openai_messages: Vec<Value> = conversation_state
            .messages
            .iter()
            .map(|msg| convert_message_to_openai(msg))
            .collect();

        let mut body = json!({
            "messages": openai_messages,
            "model": conversation_state.model,
            "stream": true
        });
        // TODO: Move API exceptions elsewhere
        // o* mini want other settings
        let is_reasoning_model =
            (provider_settings.model.contains("o") && provider_settings.model.contains("-mini") && provider_settings.host.contains("openai"))
            || provider_settings.model.contains("gpt-5");
        if !is_reasoning_model || !provider_settings.host.contains("openai")
        {
            body["max_tokens"] = json!(settings.max_tokens);
            body["temperature"] = json!(settings.temperature);
        }
        if settings.provider != "mistral" {
            body["user"] = json!(env::var("USER").unwrap_or_else(|_| "user".to_string()));
        }

        // Add tools for OpenAI
        if let Some(tool_registry) = tools {
            body["tools"] = json!(tool_registry.to_openai_format());
        }

        request_body_json = body;
    }

    let api_key = env::var(&provider_settings.api_key_variable).unwrap();

    let endpoint = if provider_settings.model.contains("gemini-") {
        format!(
            "/v1beta/models/{}:streamGenerateContent?alt=sse",
            provider_settings.model
        )
    } else {
        provider_settings.endpoint.clone()
    };

    let url = format!("https://{}{}", provider_settings.host, endpoint);
    
    let mut request = ureq::post(&url)
        .set("Content-Type", "application/json");

    if provider_settings.model.contains("gemini-") {
        request = request.query("key", &api_key);
    } else {
        request = request.set("Authorization", &format!("Bearer {}", api_key));
    }

    match request.send_json(request_body_json) {
        Ok(response) => {
            let stream_result =
                handle_stream(response, provider_settings, suppress_stream_print);

            if !suppress_stream_print && !stream_result.content.is_empty() {
                println!();
            }

            // Check if we got tool calls
            if !stream_result.tool_calls.is_empty() {
                // Add assistant message with tool calls
                let assistant_message = Message {
                    role: stream_result.role.clone(),
                    content: json!({
                        "tool_calls": stream_result.tool_calls
                    }),
                };
                conversation_state.messages.push(assistant_message);

                // Execute each tool and add results
                if let Some(tool_registry) = tools {
                    for tool_call in &stream_result.tool_calls {
                        let tool_name = tool_call
                            .get("function")
                            .and_then(|f| f.get("name"))
                            .and_then(|n| n.as_str())
                            .unwrap_or("");
                        let tool_id = tool_call
                            .get("id")
                            .and_then(|id| id.as_str())
                            .unwrap_or("");
                        let args_str = tool_call
                            .get("function")
                            .and_then(|f| f.get("arguments"))
                            .and_then(|a| a.as_str())
                            .unwrap_or("{}");

                        let args: Value = serde_json::from_str(args_str).unwrap_or(json!({}));

                        if !suppress_stream_print {
                            eprintln!("[Tool: {} with args: {}]", tool_name, args_str);
                        }

                        let result = tool_registry.execute(tool_name, &args);
                        let result_content = match result {
                            Ok(output) => handle_large_tool_output(tool_name, output),
                            Err(err) => format!("Error: {}", err),
                        };

                        // Add tool result message
                        let tool_message = Message {
                            role: "tool".to_string(),
                            content: json!({
                                "tool_call_id": tool_id,
                                "content": result_content
                            }),
                        };
                        conversation_state.messages.push(tool_message);
                    }

                    // Save state and recurse
                    save_conversation(conversation_state, transcript_path);

                    // Continue the conversation with tool results
                    perform_request_loop(
                        conversation_state,
                        transcript_path,
                        settings,
                        provider_settings,
                        suppress_stream_print,
                        tools,
                    );
                    return;
                }
            }

            // Regular text response
            let assistant_message = Message {
                role: stream_result.role,
                content: Value::String(stream_result.content),
            };
            conversation_state.messages.push(assistant_message);

            save_conversation(conversation_state, transcript_path);
        }
        Err(ureq::Error::Status(code, response)) => {
            let error_body = response
                .into_string()
                .unwrap_or_else(|_| "Failed to read error body".to_string());
            eprintln!("API Error: {} - {}", code, error_body);
        }
        Err(e) => {
            eprintln!("HTTP request error: {}", e);
        }
    }
}

struct StreamResult {
    role: String,
    content: String,
    tool_calls: Vec<Value>,
}

fn handle_stream(
    response: ureq::Response,
    provider_settings: &ProviderSettings,
    suppress_print: bool,
) -> StreamResult {
    let mut reader = response.into_reader();
    let mut full_content = String::new();
    let mut role = if provider_settings.model.contains("gemini-") {
        "model".to_string()
    } else {
        String::new()
    };
    let mut tool_calls: Vec<Value> = Vec::new();
    let mut tool_call_buffer: std::collections::HashMap<i64, Value> = std::collections::HashMap::new();

    let mut buffer = String::new();
    let mut chunk = [0; 1024];

    loop {
        match reader.read(&mut chunk) {
            Ok(0) => break, // End of stream
            Ok(n) => {
                buffer.push_str(&String::from_utf8_lossy(&chunk[..n]));

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
                                // Finalize tool calls from buffer
                                for (_, tc) in tool_call_buffer.drain() {
                                    tool_calls.push(tc);
                                }
                                return StreamResult {
                                    role,
                                    content: full_content,
                                    tool_calls,
                                };
                            }
                            if !json_str.is_empty() {
                                match serde_json::from_str::<Value>(json_str) {
                                    Ok(value) => {
                                        if provider_settings.model.contains("gemini-") {
                                            // Handle Gemini response (including function calls)
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
                                                                // Check for function call
                                                                if let Some(fc) = part.get("functionCall") {
                                                                    let name = fc.get("name").and_then(|n| n.as_str()).unwrap_or("");
                                                                    let args = fc.get("args").cloned().unwrap_or(json!({}));
                                                                    tool_calls.push(json!({
                                                                        "id": format!("call_{}", tool_calls.len()),
                                                                        "type": "function",
                                                                        "function": {
                                                                            "name": name,
                                                                            "arguments": serde_json::to_string(&args).unwrap_or_default()
                                                                        }
                                                                    }));
                                                                }
                                                                // Check for text
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
                                            // Handle OpenAI response
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
                                                        // Handle tool calls streaming
                                                        if let Some(tcs) = delta.get("tool_calls").and_then(|t| t.as_array()) {
                                                            for tc in tcs {
                                                                let index = tc.get("index").and_then(|i| i.as_i64()).unwrap_or(0);
                                                                let entry = tool_call_buffer.entry(index).or_insert_with(|| json!({
                                                                    "id": "",
                                                                    "type": "function",
                                                                    "function": {
                                                                        "name": "",
                                                                        "arguments": ""
                                                                    }
                                                                }));

                                                                if let Some(id) = tc.get("id").and_then(|i| i.as_str()) {
                                                                    entry["id"] = json!(id);
                                                                }
                                                                if let Some(f) = tc.get("function") {
                                                                    if let Some(name) = f.get("name").and_then(|n| n.as_str()) {
                                                                        entry["function"]["name"] = json!(name);
                                                                    }
                                                                    if let Some(args) = f.get("arguments").and_then(|a| a.as_str()) {
                                                                        let current = entry["function"]["arguments"].as_str().unwrap_or("");
                                                                        entry["function"]["arguments"] = json!(format!("{}{}", current, args));
                                                                    }
                                                                }
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
    // Finalize any remaining tool calls
    for (_, tc) in tool_call_buffer.drain() {
        tool_calls.push(tc);
    }
    StreamResult {
        role,
        content: full_content,
        tool_calls,
    }
}

/// Convert a Message to OpenAI format, handling tool messages specially
fn convert_message_to_openai(msg: &Message) -> Value {
    // Handle tool result messages
    if msg.role == "tool" {
        if let Some(obj) = msg.content.as_object() {
            return json!({
                "role": "tool",
                "tool_call_id": obj.get("tool_call_id").and_then(|v| v.as_str()).unwrap_or(""),
                "content": obj.get("content").and_then(|v| v.as_str()).unwrap_or("")
            });
        }
    }

    // Handle assistant messages with tool calls
    if msg.role == "assistant" {
        if let Some(obj) = msg.content.as_object() {
            if let Some(tool_calls) = obj.get("tool_calls") {
                return json!({
                    "role": "assistant",
                    "content": null,
                    "tool_calls": tool_calls
                });
            }
        }
    }

    // Regular message
    json!({
        "role": msg.role,
        "content": msg.content
    })
}

/// Convert a Message to Gemini format
fn convert_message_to_gemini(msg: &Message) -> Value {
    let role = match msg.role.as_str() {
        "system" => "user",
        "assistant" => "model",
        "tool" => "user", // Gemini uses functionResponse in user turn
        _ => msg.role.as_str(),
    };

    // Handle tool result messages
    if msg.role == "tool" {
        if let Some(obj) = msg.content.as_object() {
            let content = obj.get("content").and_then(|v| v.as_str()).unwrap_or("");
            // For Gemini, we need to format this as a functionResponse
            return json!({
                "role": "user",
                "parts": [{
                    "functionResponse": {
                        "name": "tool_result",
                        "response": {
                            "content": content
                        }
                    }
                }]
            });
        }
    }

    // Handle assistant messages with tool calls (convert to Gemini format)
    if msg.role == "assistant" || msg.role == "model" {
        if let Some(obj) = msg.content.as_object() {
            if let Some(tool_calls) = obj.get("tool_calls").and_then(|t| t.as_array()) {
                let parts: Vec<Value> = tool_calls
                    .iter()
                    .map(|tc| {
                        let name = tc
                            .get("function")
                            .and_then(|f| f.get("name"))
                            .and_then(|n| n.as_str())
                            .unwrap_or("");
                        let args_str = tc
                            .get("function")
                            .and_then(|f| f.get("arguments"))
                            .and_then(|a| a.as_str())
                            .unwrap_or("{}");
                        let args: Value = serde_json::from_str(args_str).unwrap_or(json!({}));
                        json!({
                            "functionCall": {
                                "name": name,
                                "args": args
                            }
                        })
                    })
                    .collect();
                return json!({"role": "model", "parts": parts});
            }
        }
    }

    // Handle array content (images)
    if msg.content.is_array() {
        let parts: Vec<Value> = msg
            .content
            .as_array()
            .unwrap()
            .iter()
            .map(|part_val| {
                let part_type = part_val.get("type").and_then(|t| t.as_str()).unwrap_or("");
                if part_type == "text" {
                    json!({"text": part_val.get("text").unwrap_or(&Value::Null).as_str().unwrap_or("")})
                } else if part_type == "image_url" {
                    let image_url_obj = part_val.get("image_url").unwrap();
                    let image_data_url = image_url_obj.get("url").unwrap().as_str().unwrap();
                    let parts_split: Vec<&str> = image_data_url.splitn(2, ',').collect();
                    let mime_type_part: Vec<&str> = parts_split
                        .get(0)
                        .unwrap_or(&"data:image/png;base64")
                        .splitn(2, ':')
                        .collect::<Vec<&str>>()
                        .get(1)
                        .unwrap_or(&"image/png;base64")
                        .split(';')
                        .collect();
                    let mime_type = mime_type_part[0];
                    let base64_data = parts_split.get(1).unwrap_or(&"");

                    json!({
                        "inlineData": {
                            "mimeType": mime_type,
                            "data": base64_data
                        }
                    })
                } else {
                    Value::Null
                }
            })
            .filter(|p| !p.is_null())
            .collect();
        return json!({"role": role, "parts": parts});
    }

    // Regular text message
    json!({
        "role": role,
        "parts": [{"text": msg.content.as_str().unwrap_or("")}]
    })
}

/// Handle potentially large tool output by writing to temp file if needed
/// Returns the content to use in the tool result message
fn handle_large_tool_output(tool_name: &str, output: String) -> String {
    if output.len() <= LARGE_OUTPUT_THRESHOLD {
        return output;
    }

    // Generate a unique temp file path
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let temp_dir = env::temp_dir();
    let temp_file = temp_dir.join(format!("tool_output_{}_{}.txt", tool_name, timestamp));

    // Write the full output to the temp file
    match fs::write(&temp_file, &output) {
        Ok(_) => {
            let line_count = output.lines().count();
            let byte_count = output.len();

            // Return a message instructing the AI to read the file
            format!(
                "Output too large ({} bytes, {} lines). Written to temp file: {}\n\n\
                To read the contents, use the read_file tool with this path.\n\
                Preview (first 2k chars):\n{}\n[...]",
                byte_count,
                line_count,
                temp_file.display(),
                &output[..output.len().min(2000)]
            )
        }
        Err(e) => {
            // If we can't write to temp file, truncate and return with warning
            eprintln!("Warning: Could not write large output to temp file: {}", e);
            format!(
                "[Output truncated due to size ({} bytes). Error writing temp file: {}]\n\n{}",
                output.len(),
                e,
                &output[..output.len().min(LARGE_OUTPUT_THRESHOLD)]
            )
        }
    }
}

/// Save conversation state to transcript file with truncation handling
fn save_conversation(conversation_state: &mut ConversationState, transcript_path: &PathBuf) {
    let mut truncated_state = conversation_state.clone();
    if truncated_state.messages.len() >= 2 {
        let indices = [
            truncated_state.messages.len() - 2,
            truncated_state.messages.len() - 1,
        ];
        let mut should_truncate = false;
        for &i in &indices {
            if let Some(text) = truncated_state.messages[i].content.as_str() {
                if text.len() > LARGE_OUTPUT_THRESHOLD {
                    should_truncate = true;
                    break;
                }
            }
        }
        if should_truncate {
            if prompt_confirm(
                "Your last message or assistant response was too large, recommend truncating history for this session?",
                true,
            ) {
                for &i in &indices {
                    if let Some(text) = truncated_state.messages[i].content.as_str() {
                        if text.len() > LARGE_OUTPUT_THRESHOLD {
                            truncated_state.messages[i].content =
                                json!(format!("{} [truncated]", &text[..LARGE_OUTPUT_THRESHOLD]));
                        }
                    }
                }
            }
        }
    }
    let conversation_json = serde_json::to_string_pretty(&truncated_state).unwrap();
    fs::write(transcript_path, conversation_json).expect("Unable to write transcript file");
}
