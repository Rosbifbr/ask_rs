
use crate::settings::Settings;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::Command as ProcessCommand;
use std::io::{self, Read as IoRead, Write};

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Message {
    pub role: String,
    pub content: Value,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ConversationState {
    pub model: String,
    pub messages: Vec<Message>,
}

pub fn prompt_input(prompt: &str) -> String {
    print!("{} ", prompt);
    io::stdout().flush().unwrap();
    let mut buffer = String::new();
    io::stdin().read_line(&mut buffer).unwrap();
    buffer.trim().to_string()
}

pub fn prompt_confirm(prompt: &str, default: bool) -> bool {
    let default_str = if default { "Y/n" } else { "y/N" };
    print!("{} [{}] ", prompt, default_str);
    io::stdout().flush().unwrap();
    let mut buffer = String::new();
    io::stdin().read_line(&mut buffer).unwrap();
    let input = buffer.trim().to_lowercase();
    if input.is_empty() {
        default
    } else {
        input == "y" || input == "yes"
    }
}

pub fn prompt_select(prompt: &str, items: &[String]) -> usize {
    println!("{}", prompt);
    for (i, item) in items.iter().enumerate() {
        println!("{}. {}", i + 1, item);
    }
    loop {
        print!("(1-{}): ", items.len());
        io::stdout().flush().unwrap();
        if let Some(n) = read_single_digit() {
            if n > 0 && n <= items.len() {
                println!("{}", n);
                return n - 1;
            }
        }
        println!("\nInvalid selection. Please try again.");
    }
}

fn read_single_digit() -> Option<usize> {
    use std::os::unix::io::AsRawFd;
    let stdin_fd = io::stdin().as_raw_fd();
    let orig = unsafe {
        let mut t: libc::termios = std::mem::zeroed();
        libc::tcgetattr(stdin_fd, &mut t);
        t
    };
    let mut raw = orig;
    raw.c_lflag &= !(libc::ICANON | libc::ECHO);
    raw.c_cc[libc::VMIN] = 1;
    raw.c_cc[libc::VTIME] = 0;
    unsafe { libc::tcsetattr(stdin_fd, libc::TCSANOW, &raw) };
    let mut buf = [0u8; 1];
    let result = io::stdin().lock().read_exact(&mut buf);
    unsafe { libc::tcsetattr(stdin_fd, libc::TCSANOW, &orig) };
    if result.is_ok() {
        let ch = buf[0] as char;
        ch.to_digit(10).map(|d| d as usize)
    } else {
        None
    }
}

pub fn clear_current_convo(transcript_path: &PathBuf) {
    match fs::remove_file(transcript_path) {
        Ok(_) => println!("Conversation cleared."),
        Err(e) => println!("Error clearing conversation: {}", e),
    }
}

pub fn show_history(conversation_state: &ConversationState, editor_command: String) {
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
    let columns = if let Ok(output) = ProcessCommand::new("tput").arg("cols").output() {
        String::from_utf8(output.stdout)
            .ok()
            .and_then(|s| s.trim().parse::<usize>().ok())
            .unwrap_or(80)
    } else {
        80
    };
    ch.to_string().repeat(columns)
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

pub fn delete_all_files_action(settings: &Settings) {
    let transcript_folder = env::temp_dir();
    if let Ok(entries) = fs::read_dir(&transcript_folder) {
        let files_to_delete: Vec<PathBuf> = entries
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| {
                 p.is_file()
                    && (
                        p.file_name()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .starts_with(&settings.transcript_name) ||
                        p.file_name()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .starts_with("tool_output_"))
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

pub fn manage_ongoing_convos(
    current_convo: &mut ConversationState,
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

    options.push(">>> Change Provider".to_string());

    let index = prompt_select("Select a conversation to manage", &options);

    // Last option is "Change Provider"
    if index == options.len() - 1 {
        change_provider(settings);
        return;
    }

    if index >= files.len() {
        println!("Invalid selection.");
        return;
    }
    let selected_file = &files[index];

    let action_index = prompt_select(
        &format!(
            "Action for {}:",
            selected_file.file_name().unwrap_or_default().to_string_lossy()
        ),
        &[
            "Move to this shell".to_string(),
            "Copy to this shell".to_string(),
            "Delete".to_string(),
        ],
    );

    match action_index {
        0 => {
            // Move: copy messages then delete source
            merge_conversation(current_convo, current_transcript_path, selected_file, settings);
            if let Err(e) = fs::remove_file(selected_file) {
                eprintln!("Failed to delete source conversation: {}", e);
            } else {
                println!("Conversation moved to current session.");
            }
        }
        1 => {
            // Copy: keep source
            merge_conversation(current_convo, current_transcript_path, selected_file, settings);
            println!("Conversation copied to current session.");
        }
        2 => {
            if let Err(e) = fs::remove_file(selected_file) {
                println!("Failed to delete conversation: {}", e);
            } else {
                println!("Conversation deleted.");
            }
        }
        _ => {}
    }
}

fn merge_conversation(
    current_convo: &mut ConversationState,
    current_transcript_path: &PathBuf,
    source_file: &PathBuf,
    settings: &Settings,
) {
    let data = fs::read_to_string(source_file).unwrap_or_default();
    match serde_json::from_str::<ConversationState>(&data) {
        Ok(convo_to_copy) => {
            if convo_to_copy.model != current_convo.model {
                println!(
                    "Cannot merge: Model mismatch (current: {}, selected: {}).",
                    current_convo.model, convo_to_copy.model
                );
                return;
            }
            let current_messages_is_empty = current_convo.messages.is_empty();
            let messages_to_add = convo_to_copy.messages.into_iter().skip_while(|msg| {
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
                    if let Err(e) = fs::write(current_transcript_path, conversation_json) {
                        eprintln!("Unable to write updated transcript file: {}", e);
                    }
                }
                Err(e) => eprintln!("Could not serialize conversation to JSON: {}", e),
            }
        }
        Err(e) => {
            eprintln!(
                "Could not parse selected conversation file {}: {}",
                source_file.display(),
                e
            );
        }
    }
}

fn change_provider(settings: &Settings) {
    let settings_path = env::var("HOME")
        .map(|home| format!("{}/.config/ask.json", home))
        .unwrap_or_else(|_| ".config/ask.json".to_string());

    let provider_names: Vec<String> = settings.providers.keys().cloned().collect();
    if provider_names.is_empty() {
        println!("No providers configured.");
        return;
    }

    let options: Vec<String> = provider_names
        .iter()
        .map(|name| {
            let p = &settings.providers[name];
            let current = if *name == settings.provider { " (current)" } else { "" };
            format!("{}{} [{}]", name, current, p.model)
        })
        .collect();

    let index = prompt_select("Select provider", &options);
    let chosen = &provider_names[index];

    if *chosen == settings.provider {
        println!("Already using {}.", chosen);
        return;
    }

    // Read the config file, update provider field, write back
    match fs::read_to_string(&settings_path) {
        Ok(contents) => {
            match serde_json::from_str::<serde_json::Value>(&contents) {
                Ok(mut json) => {
                    json["provider"] = serde_json::Value::String(chosen.clone());
                    match serde_json::to_string_pretty(&json) {
                        Ok(updated) => {
                            if let Err(e) = fs::write(&settings_path, updated) {
                                eprintln!("Failed to write config: {}", e);
                            } else {
                                println!("Provider changed to {}.", chosen);
                            }
                        }
                        Err(e) => eprintln!("Failed to serialize config: {}", e),
                    }
                }
                Err(e) => eprintln!("Failed to parse config: {}", e),
            }
        }
        Err(e) => eprintln!("Failed to read config file: {}", e),
    }
}

