
use crate::api::perform_request;
use crate::conversation::ConversationState;
use crate::settings::{ProviderSettings, Settings};
use dialoguer::{Confirm, Input};
use serde::Deserialize;
use serde_json::Value;
use std::path::PathBuf;
use std::process::Command as ProcessCommand;

#[derive(Deserialize, Debug)]
struct LLMResponse {
    command: Option<String>,
    explanation: Option<String>,
    complete: bool,
    signature: Option<String>,
}

pub async fn handle_recursive_mode(
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

