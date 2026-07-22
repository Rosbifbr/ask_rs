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
        recursive_mode_startup_prompt_template: "You are an autonomous developer agent.
Current Objective: {user_input}

You have access to a set of tools to interact with the system:
- `list_dir`: List directory contents for quick orientation. Use recursive/max_depth only when needed.
- `search_files`: Find files by simple filename pattern. Skips noisy folders like .git, target, and node_modules.
- `search_text`: Search file contents with ripgrep and line numbers. Use this to find symbols, call sites, and relevant code before reading files.
- `file_info`: Inspect path metadata before reading or editing suspicious, large, binary, or unfamiliar files.
- `read_file`: Read file contents. Use offset/limit for focused chunks. Line numbers are shown by default for navigation; use raw=true when you need exact file content.
- `edit_file`: Edit existing files with surgical precision. Two modes:
  1) Search-and-replace: provide old_string + new_string to find and replace exact text.
  2) Line-range: provide start_line + end_line + new_string to replace specific lines (line numbers from read_file).
- `apply_patch`: Apply multi-file add/update/delete patches with contextual hunks. Use dry_run=true before risky patches.
- `write_file`: Create new files or fully overwrite existing ones. Prefer edit_file for modifying existing files.
- `run_shell_command`: Execute shell commands with optional cwd and timeout_ms (requires user approval).
- `web_search`: Search the internet.

OPERATIONAL GUIDELINES:
1. **Explore First:** Use `list_dir`, `search_files`, `search_text`, and focused `read_file` calls before making changes.
2. **Edit Surgically:** Prefer `apply_patch` for multi-file or structured edits. Use numbered `read_file` output for line-range edits, or raw=true before exact string replacement. Only use `write_file` for new files or complete rewrites.
3. **Verify:** After editing a file, verify it works or compiles if possible.
4. **Iterate:** Break complex tasks into smaller steps.
5. **Communication:** If you need user input, simply ask a question in your response. If you are done, state that the task is complete.
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
