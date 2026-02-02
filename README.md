## ask

ask is a lightweight terminal program for chatting with OpenAI-spec and Gemini chatbots, featuring:

- **Multi-provider support:** Switch between OpenAI (GPT-4, GPT-3, GPT-4o, etc.) and Gemini models via config.
- **Tool calling:** LLMs can invoke local tools/functions for advanced tasks (file, shell, web, etc.).
- **Recursive/agent mode:** LLM can autonomously execute shell actions in a loop until tasks are complete.
- **Image input:** Send clipboard images (auto-detects Xorg/Wayland).
- **Session management:** Manage, clear, and list conversations easily.
- **Configurable:** All behavior (models, endpoints, commands) is set in a JSON config.
- **POSIX functionality:** All of the POSIX shell wonders, like piping, redirection, etc are supported.

### Installation

```bash
cargo build -r
sudo cp target/release/ask /bin/ask
```

### Usage

- Set your API key(s) as environment variables (e.g., `OPENAI_API_KEY`, `GEMINI_API_KEY`).
- Run `ask` with your prompt or pipe input:
  - `ask "Hi there"`
  - `cat file.rs | ask "Explain this code"`
  - `ask -i "Describe this image"` (clipboard image)
  - `ask -r "Automate a task"` (agent mode)
  - `ask -o` (manage sessions)
  - `ask -c` (clear current session)
  - `ask -C` (clear all sessions)

### Configuration

Edit `ask.json` in your config directory to set providers, models, and commands.

### Example `ask.json`

```json
{
  "providers": {
    "oai": {
      "model": "gpt-4o-mini",
      "host": "api.openai.com",
      "endpoint": "/v1/chat/completions",
      "api_key_variable": "OPENAI_API_KEY"
    },
    "gemini": {
      "model": "gemini-1.5-flash-latest",
      "host": "generativelanguage.googleapis.com",
      "endpoint": "",
      "api_key_variable": "GEMINI_API_KEY"
    }
  },
  "provider": "oai",
  "max_tokens": 2048,
  "temperature": 0.6,
  "vision_detail": "high",
  "transcript_name": "gpt_transcript-",
  "editor": "more",
  "clipboard_command_xorg": "xclip -selection clipboard -t image/png -o",
  "clipboard_command_wayland": "wl-paste",
  "clipboard_command_unsupported": "UNSUPPORTED",
  "startup_message": "You are ChatConcise...",
  "recursive_mode_startup_prompt_template": "You are an agent. {user_input}"
}
```
