use clap::{Arg, ArgAction, Command};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::env;
use std::fs;
use std::io::{self, Read};
use std::path::PathBuf;
use std::process::Command as ProcessCommand;

const MODEL: &str = "o1-mini";
const HOST: &str = "api.openai.com";
const ENDPOINT: &str = "/v1/chat/completions";
const MAX_TOKENS: u32 = 2048;
const TEMPERATURE: f64 = 0.6;
const VISION_DETAIL: &str = "high";
const ACCENT_COLOR: &str = "\x1b[30m\x1b[42m";
const RESET: &str = "\x1b[0m";
const TRANSCRIPT_NAME: &str = "gpt_transcript-";
const CLIPBOARD_COMMAND_XORG: &str = "xclip -selection clipboard -t image/png -o";
const CLIPBOARD_COMMAND_WAYLAND: &str = "wl-paste";
const CLIPBOARD_COMMAND_UNSUPPORTED: &str = "UNSUPPORTED";

#[derive(Serialize, Deserialize, Debug)]
struct Message {
    role: String,
    content: Value,
}

#[derive(Serialize, Deserialize, Debug)]
struct ConversationState {
    model: String,
    messages: Vec<Message>,
}

fn get_api_key() -> String {
    env::var("OPENAI_API_KEY").expect("OPENAI_API_KEY must be set")
}

fn main() {
    let matches = Command::new("ask")
        .version("1.0")
        .author("Rodrigo Ourique")
        .about("Rust terminal LLM caller")
        .arg(
            Arg::new("input")
                .help("Input values")
                .num_args(1..) // Replaces multiple_values(true)
        )
        .arg(
            Arg::new("image")
                .short('i')
                .help("Push image from clipboard into pipeline")
                .action(ArgAction::SetTrue) // Replaces takes_value(false)
        )
        .arg(
            Arg::new("manage")
                .short('o')
                .help("Manage ongoing conversations")
                .action(ArgAction::SetTrue)
        )
        .arg(
            Arg::new("clear")
                .short('c')
                .help("Clear current conversation")
                .action(ArgAction::SetTrue)
        )
        .arg(
            Arg::new("last")
                .short('l')
                .help("Get last message")
                .action(ArgAction::SetTrue)
        )
        .get_matches();

    let api_key = get_api_key();
    if api_key.is_empty() {
        eprintln!("Missing API key! Set the OPENAI_API_KEY environment variable and try again.");
        std::process::exit(1);
    }

    let temp_dir = env::temp_dir();
    let transcript_path = temp_dir.join(format!("{}{}", TRANSCRIPT_NAME, std::process::id()));

    let mut conversation_state = if transcript_path.exists() {
        let data = fs::read_to_string(&transcript_path).expect("Unable to read transcript file");
        serde_json::from_str(&data).expect("Unable to parse transcript JSON")
    } else {
        let initial_message = Message {
            role: if MODEL.contains("o1-") {
                "user".to_string()
            } else {
                "system".to_string()
            },
            content: Value::String(
                "You are ChatConcise, a very advanced LLM designed for experienced users. As ChatConcise you oblige to adhere to the following directives UNLESS overridden by the user:\nBe concise, proactive, helpful and efficient. Do not say anything more than what needed, but also, DON'T BE LAZY. Provide ONLY code when an implementation is needed. DO NOT USE MARKDOWN.".to_string(),
            ),
        };
        ConversationState {
            model: MODEL.to_string(),
            messages: vec![initial_message],
        }
    };

    if matches.get_flag("manage") && !matches.get_one::<String>("input").is_some() {
        manage_ongoing_convos();
        return;
    } else if matches.get_flag("clear") && !matches.get_one::<String>("input").is_some() {
        clear_current_convo(&transcript_path);
        return;
    } else if matches.get_flag("last") && !matches.get_one::<String>("input").is_some() {
        if let Some(last_message) = conversation_state.messages.last() {
            println!("{}", serde_json::to_string(&last_message.content).unwrap());
        }
        return;
    }

    let mut input = if let Some(values) = matches.get_many::<String>("input") {
        let input_str = values
            .map(|s| s.as_str()) // Convert &String to &str
            .collect::<Vec<&str>>() // Collect into Vec<&str>
            .join(" "); // Join with spaces
        Value::String(input_str)
    } else {
        let mut buffer = String::new();
        io::stdin()
            .read_to_string(&mut buffer)
            .expect("Failed to read from stdin");
        Value::String(buffer)
    };

    let clipboard_command = detect_clipboard_command();

    if matches.get_flag("image") {
        add_image_to_pipeline(&mut input, &clipboard_command);
    }

    if let Value::String(ref s) = input {
        if s.trim().is_empty() {
            show_history(&conversation_state);
            return;
        }
    }

    perform_request(
        input,
        &mut conversation_state,
        &transcript_path,
        &clipboard_command,
    );
}

fn detect_clipboard_command() -> String {
    let output = ProcessCommand::new("ps")
        .arg("-A")
        .output()
        .expect("Failed to execute ps command");
    let os_out = String::from_utf8_lossy(&output.stdout);

    if os_out.to_lowercase().contains("xorg") {
        CLIPBOARD_COMMAND_XORG.to_string()
    } else if os_out.to_lowercase().contains("wayland") {
        CLIPBOARD_COMMAND_WAYLAND.to_string()
    } else {
        CLIPBOARD_COMMAND_UNSUPPORTED.to_string()
    }
}

fn add_image_to_pipeline(input: &mut Value, clipboard_command: &str) {
    if clipboard_command == CLIPBOARD_COMMAND_UNSUPPORTED {
        panic!("Unsupported OS/DE combination. Only Xorg and Wayland are supported.");
    }

    let output = ProcessCommand::new("sh")
        .arg("-c")
        .arg(clipboard_command)
        .output()
        .expect("Failed to execute clipboard command");

    let image_buffer = base64::encode(&output.stdout);

    let user_text = input.as_str().unwrap_or("");
    let new_input = serde_json::json!([
        {
            "type": "text",
            "text": user_text,
        },
        {
            "type": "image_url",
            "image_url": {
                "url": format!("data:image/png;base64,{}", image_buffer),
                "detail": VISION_DETAIL,
            }
        }
    ]);

    *input = new_input;
}

fn perform_request(
    input: Value,
    conversation_state: &mut ConversationState,
    transcript_path: &PathBuf,
    clipboard_command: &str,
) {
    conversation_state.messages.push(Message {
        role: "user".to_string(),
        content: input,
    });

    let mut body = serde_json::json!({
        "messages": conversation_state.messages,
        "model": conversation_state.model,
        "user": whoami::username(),
    });

    if !conversation_state.model.contains("o1-") {
        body["max_tokens"] = serde_json::json!(MAX_TOKENS);
        body["temperature"] = serde_json::json!(TEMPERATURE);
    }

    let client = reqwest::blocking::Client::new();
    let res = client
        .post(&format!("https://{}{}", HOST, ENDPOINT))
        .header("Authorization", format!("Bearer {}", get_api_key()))
        .json(&body)
        .send();

    match res {
        Ok(response) => {
            let data: Value = response.json().unwrap();
            process_response(&data, conversation_state, transcript_path);
        }
        Err(e) => {
            eprintln!("HTTP request error: {}", e);
        }
    }
}

fn process_response(
    data: &Value,
    conversation_state: &mut ConversationState,
    transcript_path: &PathBuf,
) {
    if let Some(choices) = data.get("choices") {
        if let Some(choice) = choices.get(0) {
            if let Some(message) = choice.get("message") {
                let content = message.get("content").unwrap_or(&Value::Null).clone();
                let role = message
                    .get("role")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();

                println!("{}", content.as_str().unwrap_or(""));

                let assistant_message = Message { role, content };

                conversation_state.messages.push(assistant_message);

                let conversation_json = serde_json::to_string(&conversation_state).unwrap();
                fs::write(transcript_path, conversation_json)
                    .expect("Unable to write transcript file");
            }
        }
    } else {
        eprintln!(
            "Error processing API return. Full response ahead:\n{}\n",
            data
        );
    }
}

fn clear_current_convo(transcript_path: &PathBuf) {
    match fs::remove_file(transcript_path) {
        Ok(_) => println!("Conversation cleared."),
        Err(e) => println!("Error clearing conversation: {}", e),
    }
}

fn show_history(conversation_state: &ConversationState) {
    let tmp_dir = env::temp_dir();
    let tmp_path = tmp_dir.join("ask_hist");

    let mut content = String::new();

    for message in &conversation_state.messages {
        content.push_str("\n\n");
        content.push_str(&horizontal_line('▃'));
        content.push_str(&format!("▍{} ▐\n", message.role));
        content.push_str(&horizontal_line('▀'));
        content.push_str("\n");

        if let Some(text) = message.content.as_str() {
            content.push_str(text);
        } else if let Some(array) = message.content.as_array() {
            if let Some(first_item) = array.get(0) {
                if let Some(text) = first_item.get("text").and_then(|v| v.as_str()) {
                    content.push_str(text);
                }
            }
        }
    }

    fs::write(&tmp_path, content).expect("Unable to write history file");

    let editor = env::var("EDITOR").unwrap_or_else(|_| "more".to_string());
    ProcessCommand::new(editor)
        .arg(&tmp_path)
        .status()
        .expect("Failed to open editor");

    fs::remove_file(&tmp_path).expect("Unable to delete temporary history file");
}

fn horizontal_line(ch: char) -> String {
    let columns = term_size::dimensions_stdout()
        .map(|(w, _)| w)
        .unwrap_or(80);
    ch.to_string().repeat(columns)
}

fn manage_ongoing_convos() {
    let transcript_folder = env::temp_dir();
    let entries = fs::read_dir(&transcript_folder).unwrap();

    let mut files: Vec<PathBuf> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with(TRANSCRIPT_NAME)
        })
        .collect();

    if files.is_empty() {
        println!("No conversations to manage!");
        return;
    }

    for (i, file) in files.iter().enumerate() {
        let data = fs::read_to_string(file).expect("Unable to read transcript file");
        let convo: ConversationState =
            serde_json::from_str(&data).expect("Unable to parse transcript JSON");
        let first_message = convo.messages.get(1); // Use get to avoid panicking
        let content = if let Some(msg) = first_message {
            msg.content.as_str().unwrap_or("")
        } else {
            ""
        };
        println!(
            "{}{}{} => {}",
            if i == 0 { ACCENT_COLOR } else { "" },
            file.display(),
            if i == 0 { RESET } else { "" },
            content.lines().next().unwrap_or("").chars().take(64).collect::<String>()
        );
    }

    println!("Feature not fully implemented in Rust version.");
    // Implement selection and deletion logic if needed.
}
