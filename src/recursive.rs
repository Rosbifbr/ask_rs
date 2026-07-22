use crate::api::{cleanup_temp_files, perform_request};
use crate::conversation::{ConversationState, Message, prompt_input};
use crate::settings::{ProviderSettings, Settings};
use crate::tools::ToolRegistry;
use serde_json::Value;
use std::path::PathBuf;

pub fn handle_recursive_mode(
    conversation_state: &mut ConversationState,
    transcript_path: &PathBuf,
    user_input: String,
    settings: &Settings,
    provider_settings: &ProviderSettings,
    tools: Option<&ToolRegistry>,
) {
    // 1. Set up the initial system/user prompt for the agent
    let initial_prompt = settings
        .recursive_mode_startup_prompt_template
        .replace("{user_input}", &user_input);

    // 2. Perform the initial request.
    // This will trigger the tool loop in `api.rs` if the model calls tools immediately.
    // If the model asks a question or finishes, it returns here.
    perform_request(
        Value::String(initial_prompt),
        conversation_state,
        transcript_path,
        settings,
        provider_settings,
        false, // Allow streaming print in recursive mode
        tools,
    );

    // 3. Enter REPL loop for continued interaction.
    // Now smarter: after each agent turn, check if it signaled completion.
    // This addresses the "basic harness" complaint without changing the prompt much.
    loop {
        if let Some(last) = conversation_state.messages.last() {
            if is_completion_signal(last) {
                println!("\n[AGENT] Model signaled task completion. Exiting recursive mode.");
                break;
            }
        }

        let user_response = prompt_input("\n(Agent waiting) > Input (or 'exit'):");

        if user_response.trim().to_lowercase() == "exit" {
            println!("Exiting recursive mode.");
            cleanup_temp_files();
            break;
        }

        // Send user input back to the model
        perform_request(
            Value::String(user_response),
            conversation_state,
            transcript_path,
            settings,
            provider_settings,
            false,
            tools,
        );
    }

    cleanup_temp_files(); // Always clean up on normal exit too
}

/// Obvious check for completion phrases in final assistant response.
/// Case-insensitive, looks for clear intent so harness doesn't force unnecessary user input.
fn is_completion_signal(msg: &Message) -> bool {
    if msg.role != "assistant" && msg.role != "model" {
        return false;
    }
    let text = match &msg.content {
        Value::String(s) => s.to_lowercase(),
        _ => {
            if let Some(s) = msg.content.as_str() {
                s.to_lowercase()
            } else {
                return false;
            }
        }
    };
    text.contains("task complete")
        || text.contains("task is complete")
        || text.contains("done with task")
        || text.contains("finished the task")
        || text.contains("objective complete")
        || text.contains("all done")
        || text.contains("i have completed")
}
