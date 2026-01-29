use crate::api::perform_request;
use crate::conversation::{ConversationState, prompt_confirm, prompt_input};
use crate::settings::{ProviderSettings, Settings};
use serde_json::Value;
use std::path::PathBuf;
use std::process::Command as ProcessCommand;

// Helper to extract content between tags
fn extract_tag(input: &str, tag: &str) -> Option<String> {
    let open_tag = format!("<{}>", tag);
    let close_tag = format!("</{}>", tag);
    
    let start_idx = input.find(&open_tag)? + open_tag.len();
    let end_idx = input[start_idx..].find(&close_tag)? + start_idx;
    
    Some(input[start_idx..end_idx].trim().to_string())
}

pub fn handle_recursive_mode(
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
    );

    loop {
        let last_message = conversation_state.messages.last().cloned();
        if last_message.is_none() {
            println!("Error: No last message found in recursive mode. Exiting.");
            break;
        }
        let last_message_content = last_message.unwrap().content.as_str().unwrap_or("").to_string();
        let signature = extract_tag(&last_message_content, "signature");
        let command = extract_tag(&last_message_content, "command");
        let explanation = extract_tag(&last_message_content, "explanation");
        let complete = extract_tag(&last_message_content, "complete")
            .map(|val| val.to_lowercase() == "true")
            .unwrap_or(false);


        match signature.as_deref() {
            Some("__recursive_command_ignore") => {
                if let Some(explanation_text) = explanation {
                    println!("\nExplanation: {}", explanation_text);
                }

                if complete {
                    println!("Task marked as complete by the agent!");
                    break;
                }

                if let Some(command_text) = command {
                    if prompt_confirm(&format!("\nRun command: {}", command_text), false) {
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
                                );
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
                                );
                            }
                        }
                    } else {
                        let comment = prompt_input("Command rejected. Provide feedback for the agent, or type 'exit' to quit");

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
                        );
                    }
                } else {
                    println!("Agent did not provide a command and task is not complete. Requesting next step from agent.");
                    perform_request(
                        Value::String(format!(
                            "No command was provided in <command> tags, but <complete> is not true. Please provide the next command or ask for clarification using __recursive_prompt_user. Remember the original task: {}.",
                            user_input
                        )),
                        conversation_state,
                        transcript_path,
                        settings,
                        provider_settings,
                        true,
                    );
                }
            }
            Some("__recursive_prompt_user") => {
                // In the XML format, the question should be in the <explanation> tag
                if let Some(prompt_text) = explanation {
                    println!("Agent needs more information:");
                    println!("{}", prompt_text);

                    let user_response = prompt_input("Your response:");

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
                    );
                } else {
                    println!("Agent used __recursive_prompt_user signature but did not provide a prompt in the <explanation> tag.");
                    println!("LLM Raw Response:\n{}", last_message_content);
                    
                    perform_request(
                        Value::String(format!("Invalid XML structure: <explanation> tag missing during prompt request. Please ask the user your question inside <explanation> tags.")),
                        conversation_state,
                        transcript_path,
                        settings,
                        provider_settings,
                        true,
                    );
                }
            }
            _ => {
                println!("LLM response did not contain a valid <signature> tag. Treating as a normal message.");
                println!("LLM Raw Response:\n{}", last_message_content);
                let user_feedback = prompt_input("The agent's response was not valid XML. Provide feedback or type 'exit' to quit");

                if user_feedback.trim().to_lowercase() == "exit" {
                    println!(
                        "Exiting recursive mode due to invalid response structure."
                    );
                    break;
                }
                perform_request(
                    Value::String(format!("User feedback on invalid structure: {}. Please output RAW XML with <signature>, <command>, <explanation>, and <complete> tags.", user_feedback)),
                    conversation_state,
                    transcript_path,
                    settings,
                    provider_settings,
                    true,
                );
            }
        }
    }
}
