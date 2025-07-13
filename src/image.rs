
use crate::settings::Settings;
use serde_json::Value;
use std::process::Command as ProcessCommand;

pub fn detect_clipboard_command(settings: &Settings) -> String {
    let output = ProcessCommand::new("ps")
        .arg("-A")
        .output()
        .expect("Failed to execute ps command");
    let os_out = String::from_utf8_lossy(&output.stdout);

    if os_out.to_lowercase().contains("wayland") {
        settings.clipboard_command_wayland.clone()
    } else if os_out.to_lowercase().contains("xorg") {
        settings.clipboard_command_xorg.clone()
    } else {
        settings.clipboard_command_unsupported.clone()
    }
}

pub fn add_image_to_pipeline(input: &mut Value, clipboard_command: &str, settings: &Settings) {
    if clipboard_command == settings.clipboard_command_unsupported {
        eprintln!("Unsupported OS/DE combination for clipboard image. Only Xorg and Wayland are supported via predefined commands.");
        std::process::exit(1);
    }

    let output = ProcessCommand::new("sh")
        .arg("-c")
        .arg(clipboard_command)
        .output()
        .expect("Failed to execute clipboard command");

    if output.stdout.is_empty() {
        eprintln!("Clipboard returned no data. Ensure an image is available on the clipboard. clipboard_command is '{}'", clipboard_command);
        std::process::exit(1);
    }

    use base64::Engine;
    let image_buffer = base64::engine::general_purpose::STANDARD.encode(&output.stdout);

    let user_text = input.as_str().unwrap_or("").to_string();
    let new_input_content = serde_json::json!([
        {
            "type": "text",
            "text": user_text,
        },
        {
            "type": "image_url",
            "image_url": {
                "url": format!("data:image/png;base64,{}", image_buffer),
                "detail": settings.vision_detail,
            }
        }
    ]);

    *input = new_input_content;
}

