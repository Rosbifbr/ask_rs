
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::env;
use std::fs;

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ProviderSettings {
    pub model: String,
    pub host: String,
    pub endpoint: String,
    pub api_key_variable: String,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct Settings {
    pub providers: HashMap<String, ProviderSettings>,
    pub provider: String,
    pub max_tokens: u32,
    pub temperature: f64,
    pub vision_detail: String,
    pub transcript_name: String,
    pub editor: String,
    pub clipboard_command_xorg: String,
    pub clipboard_command_wayland: String,
    pub clipboard_command_unsupported: String,
    pub startup_message: String,
    pub recursive_mode_startup_prompt_template: String,
}

pub fn get_settings() -> Settings {
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
            endpoint: "".to_string(),
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
        recursive_mode_startup_prompt_template: "You are an autonomous developer agent running in a recursive shell loop. 
Current Objective: {user_input}

CRITICAL OUTPUT RULES:
1. Output ONLY raw XML. Do NOT use Markdown code blocks (no ```xml wrappers).
2. The XML structure must be exactly as follows:

<signature>ACTION_SIGNATURE</signature>
<explanation>Reasoning for the command OR the question for the user.</explanation>
<command>Shell command to execute (leave empty if asking a question).</command>
<complete>true or false</complete>

VALID SIGNATURES:
- '__recursive_command_ignore': Use this to execute a command or to mark the task as complete.
- '__recursive_prompt_user': Use this ONLY to ask the user for clarification. Put your question inside the <explanation> tag.

OPERATIONAL GUIDELINES:
1. **Read Before Write:** You must ALWAYS `cat` a file to read its current contents before editing it to avoid data loss or hallucinations.
2. **Atomic Steps:** Avoid chaining complex commands with `&&`. execute one distinct step per turn to allow the user to review it.
3. **Full Writes:** When writing to files, output the full file content (e.g., `echo 'content' > file`) rather than complex sed/awk patches unless necessary.
4. **Persistence:** Do not set <complete> to 'true' until you have verified the task is fully finished.
        ".to_string(),
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

