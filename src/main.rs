
mod api;
mod conversation;
mod image;
mod recursive;
mod settings;

use crate::api::perform_request;
use crate::conversation::{
    clear_current_convo, delete_all_files_action, manage_ongoing_convos, show_history,
    ConversationState, Message,
};
use crate::image::{add_image_to_pipeline, detect_clipboard_command};
use crate::recursive::handle_recursive_mode;
use crate::settings::get_settings;

use atty::Stream;
use clap::{Arg, ArgAction, Command as ClapCommand};
use serde_json::Value;
use std::env;
use std::fs;
use std::io::{self, Read};
use std::os::unix::process;
use std::process::Command;

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
        // TODO: Move API exceptions elsewhere
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
            let command_to_run =
                format!("echo \"{}\"", &settings.startup_message.replace('"', "\\\""));
            let output = Command::new("sh")
                .arg("-c")
                .arg(command_to_run)
                .output();

            let startup_message = match output {
                Ok(out) if out.status.success() => {
                    String::from_utf8_lossy(&out.stdout).trim_end().to_string()
                }
                _ => settings.startup_message.clone(),
            };

            Some(Message {
                role,
                content: startup_message.into(),
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
