
use crate::settings::{Settings, get_settings};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::Command as ProcessCommand;
use dialoguer::{theme::ColorfulTheme, Select};


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
    let columns = term_size::dimensions_stdout().map(|(w, _)| w).unwrap_or(80);
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

pub fn manage_ongoing_convos(
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

