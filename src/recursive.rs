use crate::api::perform_request;
use crate::conversation::{ConversationState, prompt_input};
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

    // 3. Enter REPL loop for continued interaction
    loop {
        // If the model finished its turn (chain of tools + final text), we end up here.
        // We prompt the user for feedback or next steps.
        let user_response = prompt_input("\n(Agent waiting) > Input (or 'exit'):");

        if user_response.trim().to_lowercase() == "exit" {
            println!("Exiting recursive mode.");
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
}