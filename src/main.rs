
mod api;
mod conversation;
mod image;
mod recursive;
mod settings;
mod tools;

use crate::api::perform_request;
use crate::conversation::{
    clear_current_convo, delete_all_files_action, manage_ongoing_convos, show_history,
    ConversationState, Message,
};
use crate::image::{add_image_to_pipeline, detect_clipboard_command};
use crate::recursive::handle_recursive_mode;
use crate::settings::get_settings;
use crate::tools::create_default_registry;

use serde_json::Value;
use std::env;
use std::fs;
use std::io::{self, IsTerminal, Read};
use std::os::unix::process;
use std::process::Command;

struct Args {
    input: Vec<String>,
    image: bool,
    manage: bool,
    clear: bool,
    last: bool,
    clear_all: bool,
    recursive: bool,
    plain: bool,
    tools: bool,
}

impl Args {
    fn parse() -> Self {
        let mut args = Args {
            input: Vec::new(),
            image: false,
            manage: false,
            clear: false,
            last: false,
            clear_all: false,
            recursive: false,
            plain: false,
            tools: true, // Tools are enabled by default
        };

        let mut env_args = env::args().skip(1);
        while let Some(arg) = env_args.next() {
            match arg.as_str() {
                "-i" | "--image" => args.image = true,
                "-o" | "--manage" => args.manage = true,
                "-c" | "--clear" => args.clear = true,
                "-l" | "--last" => args.last = true,
                "-C" | "--clear_all" => args.clear_all = true,
                "-r" | "--recursive" => args.recursive = true,
                "-p" | "--plain" => args.plain = true,
                "-t" | "--tools" => args.tools = false,
                "-h" | "--help" => {
                    println!("ask-rs 1.5\nRust terminal LLM caller with streaming\n\nUSAGE:\n    ask [FLAGS] [INPUT]...\n\nFLAGS:\n    -i, --image       Push image from clipboard into pipeline\n    -o, --manage      Manage ongoing conversations\n    -c, --clear       Clear current conversation\n    -l, --last        Get last message\n    -C, --clear_all   Remove all chats\n    -r, --recursive   Interactive agent mode\n    -t, --tools       Disable tool use (read_file, write_file, web_search)\n    -p, --plain       Start conversation without system prompt\n    -h, --help        Prints help information");
                    std::process::exit(0);
                }
                val => {
                    if !val.starts_with('-') {
                         args.input.push(val.to_string());
                    } else {
                        eprintln!("Unknown flag: {}", val);
                    }
                }
            }
        }
        args
    }
}

fn main() {
    let args = Args::parse();

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
        let initial_message = if !args.plain {
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
    if !io::stdin().is_terminal() {
        let mut buffer = String::new();
        io::stdin()
            .read_to_string(&mut buffer)
            .expect("Failed to read from stdin");
        if !buffer.trim().is_empty() {
            input_parts.push(buffer);
        }
    }

    if !args.input.is_empty() {
        let input_str = args.input.join(" ");
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

    // Create tool registry if tools are enabled
    let tool_registry = if args.tools || args.recursive {
        Some(create_default_registry())
    } else {
        None
    };

    if args.recursive {
        handle_recursive_mode(
            &mut conversation_state,
            &transcript_path,
            input_string_for_recursive,
            &settings,
            &provider_settings,
            tool_registry.as_ref(),
        );
        return;
    } else if args.clear_all {
        delete_all_files_action(&settings);
        return;
    } else if args.manage && args.input.is_empty() {
        manage_ongoing_convos(&mut conversation_state, &transcript_path, &settings);
        return;
    } else if args.clear && args.input.is_empty() {
        clear_current_convo(&transcript_path);
        return;
    } else if args.last && args.input.is_empty() {
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
    if args.image {
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
        tool_registry.as_ref(),
    );
}
