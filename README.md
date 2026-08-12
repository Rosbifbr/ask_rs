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

### macOS app

`Ask.app` is a small native wrapper around the same `ask` binary. It is a
normal Mac app (Dock, Spotlight, Cmd-Tab) that hosts `ask` on a PTY — no
Terminal.app, no login shell per query.

The window is just `ask`: type a prompt, enter, stream the answer. Follow-ups
reuse the same conversation because every child shares the wrapper's PID.
There is no agent mode in the app.

#### Build

```bash
macos/build-app.sh
open macos/Ask.app
# so Spotlight indexes it:
cp -R macos/Ask.app /Applications/
```

This compiles the Rust CLI, builds the AppKit host, and assembles
`macos/Ask.app` (the bundle itself is gitignored). Requires Xcode / Swift 5.9+.

#### Launch

| Invoke | What happens |
|---|---|
| Spotlight / Dock / `open macos/Ask.app` | Opens the window. Type, enter to send. |
| `open 'ask://q?text=hello'` | Opens the window and runs `ask hello` |
| `open -a Ask --args hello world` | Same as a prompt, on first launch only |
| Raycast | Add `macos/raycast/ask.sh` as a Script Command |

#### Keys

| Key | Action |
|---|---|
| enter | Send the prompt |
| ⌘K | Clear conversation **and** the screen |
| ⌘O | Manage sessions (`ask -o`) |
| ⌘/ | Toggle the controls header |
| esc / ⌘W | Hide the window (app stays in the Dock) |
| ⌘Q | Quit |

The controls header (Clear / Manage / keymap) starts **collapsed**. Expand it
from the **Controls** disclosure, or **View → Controls**.

`/c`, `/o`, and `/i …` in the prompt bar map to clear, manage, and clipboard
image (`ask -i`). Image paste still needs a working clipboard command in
`ask.json` — the Xorg/Wayland defaults do not work on macOS.

#### API keys

Spotlight and Dock do not load `config.fish` / `.zshrc`. On startup the app
dumps your login-shell environment once (2s timeout) and also reads
`~/.config/ask.env`:

```
OPENAI_API_KEY=sk-...
GEMINI_API_KEY=...
```

Layout: `macos/Sources/AskApp` (host), `macos/build-app.sh` (bundle),
`macos/raycast/ask.sh` (Raycast). The CLI is copied to
`Ask.app/Contents/Helpers/ask` so it does not collide with the `Ask` host on
case-insensitive APFS.

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
The provider `endpoint` selects the request format: `/v1/responses` uses the
Responses API, while an omitted endpoint defaults to `/v1/chat/completions`.
Set `reasoning_effort` per provider when the selected model supports it; ask
maps it to the endpoint-specific request shape automatically.

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
    "oai_responses": {
      "model": "gpt-5.6-terra",
      "host": "api.openai.com",
      "endpoint": "/v1/responses",
      "api_key_variable": "OPENAI_API_KEY",
      "reasoning_effort": "medium"
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
