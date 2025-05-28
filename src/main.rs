use atty::Stream;
use clap::{Arg, ArgAction, Command as ClapCommand};
use dialoguer::{theme::ColorfulTheme, Confirm, Input, Select};
use futures_util::StreamExt;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::env;
use std::fs;
use std::io::{self, Read, Write};
use std::os::unix::process;
use std::path::PathBuf;
use std::process::Command as ProcessCommand;
use std::time::Duration; // Added import for Duration

#[derive(Serialize, Deserialize, Debug, Clone)]
struct Message {
    role: String,
    content: Value,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
struct ConversationState {
    model: String,
    messages: Vec<Message>,
}

#[derive(Serialize, Deserialize, Debug)]
struct ProviderSettings {
    model: String,
    host: String,
    endpoint: String,
    api_key_variable: String,
}

#[derive(Serialize, Deserialize, Debug)]
struct Settings {
    providers: HashMap<String, ProviderSettings>,
    provider: String,
    max_tokens: u32,
    temperature: f64,
    vision_detail: String,
    transcript_name: String,
    editor: String,
    clipboard_command_xorg: String,
    clipboard_command_wayland: String,
    clipboard_command_unsupported: String,
    startup_message: String,
    recursive_mode_startup_prompt_template: String,
}

fn get_settings() -> Settings {
    let mut default_providers = HashMap::new();
    default_providers.insert(
        "oai".to_string(),
        ProviderSettings {
            model: "gpt-4o-mini".to_string(),
            host: "api.openai.com".to_string(),
            endpoint: "/v1/chat/completions".to_string(),
            api_key_variable: "OPENAI_API_KEY".to_string(),
        },
    );
    default_providers.insert(
        "gemini".to_string(),
        ProviderSettings {
            model: "gemini-1.5-flash-latest".to_string(),
            host: "generativelanguage.googleapis.com".to_string(),
            endpoint: "/v1beta/models/gemini-1.5-flash-latest:streamGenerateContent".to_string(),
            api_key_variable: "GEMINI_API_KEY".to_string(),
        },
    );

    let default_settings = Settings {
        providers: default_providers,
        provider: "oai".to_string(),
        max_tokens: 2048,
        temperature: 0.6,
        vision_detail: "high".to_string(),
        transcript_name: "gpt_transcript-".to_string(),
        editor: "more".to_string(),
        clipboard_command_xorg: "xclip -selection clipboard -t image/png -o".to_string(),
        clipboard_command_wayland: "wl-paste".to_string(),
        clipboard_command_unsupported: "UNSUPPORTED".to_string(),
        startup_message: "You are ChatConcise, a very advanced LLM designed for experienced users. As ChatConcise you oblige to adhere to the following directives UNLESS overridden by the user:\nBe concise, proactive, helpful and efficient. Do not say anything more than what needed, but also, DON'T BE LAZY. If the user is asking for software, provide ONLY the code.".to_string(),
        recursive_mode_startup_prompt_template: "You are entering 'recursive agent mode' with the following instruction: {user_input}. \
        You can respond in one of two key-value formats, with each key on a new line:\
        \
        1. To suggest a command to run:\
        signature: __recursive_command_ignore\
        complete: <true or false>\
        command: <command to run, if any>\
        explanation: <explanation of your suggestion>\
        \
        2. To ask the user for more information or provide context before proceeding:\
        signature: __recursive_prompt_user\
        complete: <true or false>\
        prompt: <question or information for the user>\
        explanation: <explanation of your suggestion>\
        \
        You can use 'cat file' to read files and 'echo *text* > file' to write to files. Remember to always write the full file. \
        Reminder 1: To edit any file, you must ALWAYS read the file with 'cat' first so that you do not halluculate its contents. \
Reminder 2: Prefer not to chain commands with && unless necessary, as it difficultates user review. \
Reminder 3: DO NOT BE LAZY! You should do EVERYTHING for the user UNTIL the task is complete. \
Do not include ANY extra text or markdown delimiters."
            .to_string(),
    };

    let settings_path = env::var("HOME")
        .map(|home| format!("{}/.config/ask.json", home))
        .unwrap_or_else(|_| ".config/ask.json".to_string());

    match fs::read_to_string(&settings_path)
        .map_err(|e| format!("Could not read file: {}", e))
        .and_then(|contents| {
            serde_json::from_str(&contents).map_err(|e| format!("Could not parse JSON: {}", e))
        }) {
        Ok(settings) => settings,
        Err(e) => {
            println!("WARNING: Using default settings. Error: {}.", e);
            default_settings
        }
    }
}

#[tokio::main]
async fn main() {
    let matches = ClapCommand::new("ask")
        .version("1.4")
        .author("Rodrigo Ourique")
        .about("Rust terminal LLM caller with streaming")
        .arg(Arg::new("input").help("Input values").num_args(0..))
        .arg(
            Arg::new("image")
                .short('i')
                .help("Push image from clipboard into pipeline")
                .action(ArgAction::SetTrue),
        )
        .arg(
            Arg::new("manage")
                .short('o')
                .help("Manage ongoing conversations")
                .action(ArgAction::SetTrue),
        )
        .arg(
            Arg::new("clear")
                .short('c')
                .help("Clear current conversation")
                .action(ArgAction::SetTrue),
        )
        .arg(
            Arg::new("last")
                .short('l')
                .help("Get last message")
                .action(ArgAction::SetTrue),
        )
        .arg(
            Arg::new("clear_all")
                .short('C')
                .help("Remove all chats")
                .action(ArgAction::SetTrue),
        )
        .arg(
            Arg::new("recursive")
                .short('r')
                .help("Interactive agent mode")
                .action(ArgAction::SetTrue),
        )
        .arg(
            Arg::new("plain")
                .short('p')
                .help("Start conversation without system prompt")
                .action(ArgAction::SetTrue),
        )
        .get_matches();

    let settings = get_settings();
    let provider_settings = settings
        .providers
        .get(&settings.provider)
        .unwrap_or_else(|| {
            eprintln!("Invalid provider: {}", settings.provider);
            std::process::exit(1);
        });

    let api_key = env::var(&provider_settings.api_key_variable).unwrap_or_else(|_| {
        panic!(
            "Missing API key environment variable: {}!",
            provider_settings.api_key_variable
        )
    });

    if api_key.is_empty() {
        eprintln!(
            "Missing API key! Set the {} environment variable and try again.",
            provider_settings.api_key_variable
        );
        std::process::exit(1);
    }

    let temp_dir = env::temp_dir();
    let transcript_path = temp_dir.join(format!(
        "{}{}",
        settings.transcript_name,
        process::parent_id()
    ));

    let mut conversation_state = if transcript_path.exists() {
        let data = fs::read_to_string(&transcript_path).expect("Unable to read transcript file");
        serde_json::from_str(&data).expect("Unable to parse transcript JSON")
    } else {
        let initial_message = if !matches.get_flag("plain") {
            let role = if provider_settings.model.contains("gemini-") {
                "user".to_string()
            } else if provider_settings.model.contains("o1-")
                || provider_settings.model.contains("o3-")
            {
                "user".to_string()
            } else {
                "system".to_string()
            };
            Some(Message {
                role,
                content: settings.startup_message.clone().into(),
            })
        } else {
            None
        };
        ConversationState {
            model: provider_settings.model.to_string(),
            messages: initial_message.map_or(vec![], |msg| vec![msg]),
        }
    };

    let mut input_parts = Vec::new();
    if !atty::is(Stream::Stdin) {
        let mut buffer = String::new();
        io::stdin()
            .read_to_string(&mut buffer)
            .expect("Failed to read from stdin");
        if !buffer.trim().is_empty() {
            input_parts.push(buffer);
        }
    }

    if let Some(values) = matches.get_many::<String>("input") {
        let input_str = values.map(|s| s.as_str()).collect::<Vec<&str>>().join(" ");
        if !input_str.trim().is_empty() {
            input_parts.push(input_str);
        }
    }

    let mut input_value = if input_parts.is_empty() {
        Value::Null
    } else {
        Value::String(input_parts.join("\n"))
    };
    let input_string_for_recursive = input_value.as_str().unwrap_or("").to_string();

    if matches.get_flag("recursive") {
        handle_recursive_mode(
            &mut conversation_state,
            &transcript_path,
            input_string_for_recursive,
            &settings,
            &provider_settings,
        )
        .await;
        return;
    } else if matches.get_flag("clear_all") {
        delete_all_files_action(&settings);
        return;
    } else if matches.get_flag("manage") && !matches.get_one::<String>("input").is_some() {
        manage_ongoing_convos(&mut conversation_state, &transcript_path, &settings);
        return;
    } else if matches.get_flag("clear") && !matches.get_one::<String>("input").is_some() {
        clear_current_convo(&transcript_path);
        return;
    } else if matches.get_flag("last") && !matches.get_one::<String>("input").is_some() {
        if let Some(last_message) = conversation_state.messages.last() {
            if let Ok(pretty_json) = serde_json::to_string_pretty(&last_message.content) {
                println!("{}", pretty_json);
            } else {
                println!(
                    "{}",
                    serde_json::to_string(&last_message.content).unwrap_or_default()
                );
            }
        }
        return;
    }

    let clipboard_command = detect_clipboard_command(&settings);
    if matches.get_flag("image") {
        add_image_to_pipeline(&mut input_value, &clipboard_command, &settings);
    }

    if input_value.is_null() {
        show_history(&conversation_state, settings.editor.clone());
        return;
    }

    perform_request(
        input_value,
        &mut conversation_state,
        &transcript_path,
        &settings,
        &provider_settings,
        false,
    )
    .await;
}

fn detect_clipboard_command(settings: &Settings) -> String {
    let output = ProcessCommand::new("ps")
        .arg("-A")
        .output()
        .expect("Failed to execute ps command");
    let os_out = String::from_utf8_lossy(&output.stdout);

    if os_out.to_lowercase().contains("wayland") {
        settings.clipboard_command_wayland.clone()
    } else if os_out.to_lowercase().contains("xorg") {
        settings.clipboard_command_xorg.clone()
    } else {
        settings.clipboard_command_unsupported.clone()
    }
}

fn add_image_to_pipeline(input: &mut Value, clipboard_command: &str, settings: &Settings) {
    if clipboard_command == settings.clipboard_command_unsupported {
        eprintln!("Unsupported OS/DE combination for clipboard image. Only Xorg and Wayland are supported via predefined commands.");
        std::process::exit(1);
    }

    let output = ProcessCommand::new("sh")
        .arg("-c")
        .arg(clipboard_command)
        .output()
        .expect("Failed to execute clipboard command");

    if output.stdout.is_empty() {
        eprintln!("Clipboard returned no data. Ensure an image is available on the clipboard. clipboard_command is '{}'", clipboard_command);
        std::process::exit(1);
    }

    use base64::Engine;
    let image_buffer = base64::engine::general_purpose::STANDARD.encode(&output.stdout);

    let user_text = input.as_str().unwrap_or("").to_string();
    let new_input_content = serde_json::json!([
        {
            "type": "text",
            "text": user_text,
        },
        {
            "type": "image_url",
            "image_url": {
                "url": format!("data:image/png;base64,{}", image_buffer),
                "detail": settings.vision_detail,
            }
        }
    ]);

    *input = new_input_content;
}

async fn perform_request(
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
        // o* mini want other settings
        let pat = Regex::new(r"o\d-mini").unwrap();
        if !provider_settings.host.contains("openai")
            || !pat.is_match(provider_settings.model.as_str())
        {
            body["max_tokens"] = serde_json::json!(settings.max_tokens);
            body["temperature"] = serde_json::json!(settings.temperature);
        }
        body["user"] = serde_json::json!(whoami::username());
        request_body_json = body;
    }

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(300))
        .build()
        .unwrap();

    let api_key = env::var(&provider_settings.api_key_variable).unwrap();

    let request_builder = client
        .post(format!(
            "https://{}{}",
            provider_settings.host, provider_settings.endpoint
        ))
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
                    if provider_settings.model.contains("gemini-") {
                        if let Some(end_of_object_idx) = buffer.find("}\n") {
                            let potential_json = &buffer[..=end_of_object_idx];
                            if let Ok(value) = serde_json::from_str::<Value>(potential_json.trim())
                            {
                                if let Some(candidates) =
                                    value.get("candidates").and_then(|c| c.as_array())
                                {
                                    for candidate in candidates {
                                        if let Some(content) = candidate.get("content") {
                                            if let Some(parts) =
                                                content.get("parts").and_then(|p| p.as_array())
                                            {
                                                for part in parts {
                                                    if let Some(text_delta) =
                                                        part.get("text").and_then(|t| t.as_str())
                                                    {
                                                        if !suppress_print {
                                                            print!("{}", text_delta);
                                                            io::stdout().flush().unwrap();
                                                        }
                                                        full_content.push_str(text_delta);
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                                buffer.replace_range(..=end_of_object_idx, "");
                                continue;
                            } else {
                                break;
                            }
                        } else {
                            break;
                        }
                    } else {
                        // OpenAI SSE
                        if let Some(newline_idx) = buffer.find('\n') {
                            let line = buffer[..newline_idx + 1].to_string();
                            buffer.replace_range(..newline_idx + 1, "");

                            if line.starts_with("data: ") {
                                let json_str = line["data: ".len()..].trim();
                                if json_str == "[DONE]" {
                                    // This comparison should be fine
                                    if !suppress_print {
                                        io::stdout().flush().unwrap();
                                    }
                                    if role.is_empty() {
                                        role = "assistant".to_string();
                                    }
                                    return (role, full_content);
                                }
                                if !json_str.is_empty() {
                                    match serde_json::from_str::<Value>(json_str) {
                                        Ok(value) => {
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

fn clear_current_convo(transcript_path: &PathBuf) {
    match fs::remove_file(transcript_path) {
        Ok(_) => println!("Conversation cleared."),
        Err(e) => println!("Error clearing conversation: {}", e),
    }
}

fn show_history(conversation_state: &ConversationState, editor_command: String) {
    let tmp_dir = env::temp_dir();
    let tmp_path = tmp_dir.join("ask_hist");
    let mut content_str = String::new();

    for message in &conversation_state.messages {
        content_str.push_str("\n\n");
        content_str.push_str(&horizontal_line('▃'));
        content_str.push_str(&format!("▍{} ▐\n", message.role));
        content_str.push_str(&horizontal_line('▀'));
        content_str.push_str("\n");

        if let Some(text) = message.content.as_str() {
            content_str.push_str(text);
        } else if let Some(array) = message.content.as_array() {
            for item in array {
                if let Some(item_type) = item.get("type").and_then(|v| v.as_str()) {
                    if item_type == "text" {
                        if let Some(text) = item.get("text").and_then(|v| v.as_str()) {
                            content_str.push_str(text);
                            content_str.push_str("\n");
                        }
                    } else if item_type == "image_url" {
                        content_str.push_str("[Image content - not displayed in text history]\n");
                        if let Some(image_url_val) =
                            item.get("image_url").and_then(|v| v.get("url"))
                        {
                            if let Some(url_str) = image_url_val.as_str() {
                                content_str.push_str(&format!(
                                    "[Image URL (truncated): {}...]\n",
                                    url_str.chars().take(70).collect::<String>()
                                ));
                            }
                        }
                    }
                }
            }
        } else {
            content_str.push_str(&message.content.to_string());
        }
    }

    fs::write(&tmp_path, content_str).expect("Unable to write history file");
    ProcessCommand::new(editor_command)
        .arg(&tmp_path)
        .status()
        .expect("Failed to open editor");

    fs::remove_file(&tmp_path).expect("Unable to delete temporary history file");
}

fn horizontal_line(ch: char) -> String {
    let columns = term_size::dimensions_stdout().map(|(w, _)| w).unwrap_or(80);
    ch.to_string().repeat(columns)
}

#[derive(Deserialize, Debug)]
struct LLMResponse {
    command: Option<String>,
    explanation: Option<String>,
    complete: bool,
    signature: Option<String>,
}

async fn handle_recursive_mode(
    conversation_state: &mut ConversationState,
    transcript_path: &PathBuf,
    user_input: String,
    settings: &Settings,
    provider_settings: &ProviderSettings,
) {
    let initial_prompt = settings
        .recursive_mode_startup_prompt_template
        .replace("{user_input}", &user_input);

    perform_request(
        Value::String(initial_prompt),
        conversation_state,
        transcript_path,
        settings,
        provider_settings,
        true,
    )
    .await;

    loop {
        let last_message = conversation_state.messages.last().cloned();
        if last_message.is_none() {
            println!("Error: No last message found in recursive mode. Exiting.");
            break;
        }
        let last_message_content = last_message.unwrap().content;

        let response_str = last_message_content.as_str().unwrap_or("");

        let mut signature: Option<String> = None;
        let mut complete: bool = false;
        let mut command: Option<String> = None;
        let mut explanation: Option<String> = None;

        for line in response_str.lines() {
            let parts: Vec<&str> = line.splitn(2, ":").collect();
            if parts.len() == 2 {
                let key = parts[0].trim();
                let value = parts[1].trim();
                match key {
                    "signature" => signature = Some(value.to_string()),
                    "complete" => complete = value.to_lowercase() == "true",
                    "command" => command = Some(value.to_string()),
                    "explanation" => explanation = Some(value.to_string()),
                    _ => {}
                }
            }
        }

        match signature.as_deref() {
            Some("__recursive_command_ignore") => {
                if let Some(explanation_text) = explanation {
                    println!("Explanation: {}", explanation_text);
                }

                if complete {
                    println!("Task marked as complete by the agent!");
                    break;
                }

                if let Some(command_text) = command {
                    if Confirm::new()
                        .with_prompt(format!("\nRun command: {}", command_text))
                        .default(false)
                        .interact()
                        .unwrap_or(false)
                    {
                        match ProcessCommand::new("sh")
                            .arg("-c")
                            .arg(&command_text)
                            .output()
                        {
                            Ok(output) => {
                                let stdout = String::from_utf8_lossy(&output.stdout);
                                let stderr = String::from_utf8_lossy(&output.stderr);
                                let result = format!(
                                    "Command executed. Output:\nstdout:\n{}\nstderr:\n{}",
                                    stdout, stderr
                                );
                                println!("{}", result);
                                perform_request(
                                    Value::String(result),
                                    conversation_state,
                                    transcript_path,
                                    settings,
                                    provider_settings,
                                    true,
                                )
                                .await;
                            }
                            Err(e) => {
                                let error_msg = format!("Failed to execute command: {}", e);
                                println!("{}", error_msg);
                                perform_request(
                                    Value::String(error_msg),
                                    conversation_state,
                                    transcript_path,
                                    settings,
                                    provider_settings,
                                    true,
                                )
                                .await;
                            }
                        }
                    } else {
                        let comment = Input::<String>::new()
                            .with_prompt(
                                "Command rejected. Provide feedback for the agent, or type 'exit' to quit",
                            )
                            .interact_text()
                            .unwrap_or_default();

                        if comment.trim().to_lowercase() == "exit" {
                            println!("Exiting recursive mode.");
                            break;
                        }

                        perform_request(
                            Value::String(format!(
                                "User rejected the command.\nFeedback: {}\nPlease suggest an alternative or ask for clarification. Original task: {}",
                                comment, user_input
                            )),
                            conversation_state,
                            transcript_path,
                            settings,
                            provider_settings,
                            true,
                        )
                        .await;
                    }
                } else {
                    println!("Agent did not provide a command and task is not complete. Requesting next step from agent.");
                    perform_request(
                        Value::String(format!(
                            "No command was provided, but the task is not yet complete. Please provide the next command or ask for clarification. Remember the original task: {}. Ensure you provide a 'command' or set 'complete' to true.",
                            user_input
                        )),
                        conversation_state,
                        transcript_path,
                        settings,
                        provider_settings,
                        true,
                    )
                    .await;
                }
            }
            Some("__recursive_prompt_user") => {
                if let Some(prompt_text) = explanation {
                    // Using explanation for prompt text
                    println!("Agent needs more information:");
                    println!("{}", prompt_text);

                    let user_response = Input::<String>::new()
                        .with_prompt("Your response:")
                        .interact_text()
                        .unwrap_or_default();

                    if user_response.trim().to_lowercase() == "exit" {
                        println!("Exiting recursive mode.");
                        break;
                    }

                    perform_request(
                        Value::String(format!(
                            "User provided information: {}. Continue with the original task: {}.",
                            user_response, user_input
                        )),
                        conversation_state,
                        transcript_path,
                        settings,
                        provider_settings,
                        true,
                    )
                    .await;
                } else {
                    println!("Agent used __recursive_prompt_user signature but did not provide a prompt in the explanation field. Treating as a normal message.");
                    println!("LLM Raw Response:\n{}", response_str);
                    let user_feedback = Input::<String>::new()
                        .with_prompt("The agent's response was not a valid prompt structure. Please provide feedback or a new instruction, or type 'exit' to quit recursive mode")
                        .interact_text()
                        .unwrap_or_else(|_| "exit".to_string());

                    if user_feedback.trim().to_lowercase() == "exit" {
                        println!(
                            "Exiting recursive mode due to invalid response structure and user choice."
                        );
                        break;
                    }
                    perform_request(
                        Value::String(format!("User feedback on invalid prompt structure: {}. Please remember the original task: {}. Adhere to the key-value output format with 'prompt' in the explanation field and 'signature' as __recursive_prompt_user.", user_feedback, user_input)),
                        conversation_state,
                        transcript_path,
                        settings,
                        provider_settings,
                        true,
                    ).await;
                }
            }
            _ => {
                println!("LLM response did not contain a recognized signature for recursive mode. Treating as a normal message.");
                println!("LLM Raw Response:\n{}", response_str);
                let user_feedback = Input::<String>::new()
                    .with_prompt("The agent's response was not a valid command or prompt structure. Please provide feedback or a new instruction, or type 'exit' to quit recursive mode")
                    .interact_text()
                    .unwrap_or_else(|_| "exit".to_string());

                if user_feedback.trim().to_lowercase() == "exit" {
                    println!(
                        "Exiting recursive mode due to invalid response structure and user choice."
                    );
                    break;
                }
                perform_request(
                    Value::String(format!("User feedback on invalid structure: {}. Please remember the original task: {}. Adhere to the key-value output format with 'command' or 'prompt' and a recognized 'signature'.", user_feedback, user_input)),
                    conversation_state,
                    transcript_path,
                    settings,
                    provider_settings,
                    true,
                ).await;
            }
        }
    }
}

fn delete_files(files: Vec<PathBuf>) -> usize {
    let mut deleted_count = 0;
    for file in &files {
        if let Err(e) = fs::remove_file(file) {
            eprintln!("Failed to delete {}: {}", file.display(), e);
        } else {
            deleted_count += 1;
        }
    }
    deleted_count
}

fn delete_all_files_action(settings: &Settings) {
    let transcript_folder = env::temp_dir();
    if let Ok(entries) = fs::read_dir(&transcript_folder) {
        let files_to_delete: Vec<PathBuf> = entries
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| {
                p.is_file()
                    && p.file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .starts_with(&settings.transcript_name)
            })
            .collect();

        if files_to_delete.is_empty() {
            println!("No conversation transcripts found to delete.");
            return;
        }

        let deleted_count = delete_files(files_to_delete);
        println!("Deleted {} conversation transcript(s).", deleted_count);
    } else {
        eprintln!(
            "Could not read transcript directory: {}",
            transcript_folder.display()
        );
    }
}

fn manage_ongoing_convos(
    current_convo: &mut ConversationState, // Corrected: typo was ¤t_convo
    current_transcript_path: &PathBuf,
    settings: &Settings,
) {
    let transcript_folder = env::temp_dir();
    let entries = match fs::read_dir(&transcript_folder) {
        Ok(e) => e,
        Err(err) => {
            eprintln!("Could not read transcript directory: {}", err);
            return;
        }
    };

    let files: Vec<PathBuf> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.is_file()
                && p.file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .starts_with(&settings.transcript_name)
        })
        .collect();

    if files.is_empty() {
        println!("No conversations to manage!");
        return;
    }

    let mut options: Vec<String> = files
        .iter()
        .map(|file| {
            let data = fs::read_to_string(file).unwrap_or_default();
            let convo_first_message_content = match serde_json::from_str::<ConversationState>(&data)
            {
                Ok(convo) => convo
                    .messages
                    .get(1)
                    .and_then(|msg| msg.content.as_str())
                    .unwrap_or("[Could not parse message content or not a string]")
                    .lines()
                    .next()
                    .unwrap_or("[Empty first line]")
                    .chars()
                    .take(64)
                    .collect::<String>(),
                Err(_) => "[Error reading transcript content]".to_string(),
            };
            format!(
                "{} => {}",
                file.file_name().unwrap_or_default().to_string_lossy(),
                convo_first_message_content
            )
        })
        .collect();

    options.insert(0, ">>> Delete All Conversations".to_string());

    let selection = Select::with_theme(&ColorfulTheme::default())
        .with_prompt("Select an option to manage")
        .default(0)
        .items(&options)
        .interact();

    if let Ok(index) = selection {
        if index == 0 {
            delete_all_files_action(settings);
            if current_transcript_path.exists() {
                let current_filename = current_transcript_path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy();
                if files.iter().any(|f| {
                    f.file_name().unwrap_or_default().to_string_lossy() == current_filename
                }) {
                    // Current convo was deleted
                }
            }
            return;
        }

        let selected_file_index = index - 1;
        if selected_file_index >= files.len() {
            println!("Invalid selection.");
            return;
        }
        let selected_file = &files[selected_file_index];

        let action = Select::with_theme(&ColorfulTheme::default())
            .with_prompt(format!(
                "Action for {}:",
                selected_file
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
            ))
            .default(0)
            .items(&["Delete", "Copy to Current Conversation", "Cancel"])
            .interact();

        match action {
            Ok(0) => {
                if let Err(e) = fs::remove_file(selected_file) {
                    println!("Failed to delete conversation: {}", e);
                } else {
                    println!(
                        "Conversation {} deleted successfully.",
                        selected_file.display()
                    );
                }
            }
            Ok(1) => {
                let data = fs::read_to_string(selected_file).unwrap_or_default();
                match serde_json::from_str::<ConversationState>(&data) {
                    Ok(convo_to_copy) => {
                        if convo_to_copy.model != current_convo.model {
                            // Corrected: current_convo
                            println!(
                                "Cannot copy conversation: Model mismatch (current: {}, selected: {}).",
                                current_convo.model, convo_to_copy.model // Corrected: current_convo
                            );
                            return;
                        }
                        let current_messages_is_empty = current_convo.messages.is_empty(); // Evaluate before closure
                        let messages_to_add =
                            convo_to_copy.messages.into_iter().skip_while(|msg| {
                                !current_messages_is_empty
                                    && (msg.role == "system"
                                        || (msg
                                            .content
                                            .as_str()
                                            .map_or(false, |s| s == settings.startup_message)))
                            });
                        current_convo.messages.extend(messages_to_add);

                        match serde_json::to_string_pretty(current_convo) {
                            Ok(conversation_json) => {
                                if let Err(e) =
                                    fs::write(current_transcript_path, conversation_json)
                                {
                                    eprintln!("Unable to write updated transcript file: {}", e);
                                } else {
                                    println!(
                                        "Conversation copied to current session successfully."
                                    );
                                }
                            }
                            Err(e) => eprintln!("Could not serialize conversation to JSON: {}", e),
                        }
                    }
                    Err(e) => {
                        eprintln!(
                            "Could not parse selected conversation file {}: {}",
                            selected_file.display(),
                            e
                        );
                    }
                }
            }
            _ => {
                println!("Action cancelled.");
            }
        }
    }
}
