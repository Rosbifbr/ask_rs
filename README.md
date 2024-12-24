## ask

ask is a very lightweight program that allows you to chat with OpenAI chatbots in a terminal without wasting the time to open a heavy browser or to switch screens. The application is lightweight and keeps one separate, manageable conversation history per process.

The script uses the newer chat API from OpenAI and is preconfigured to use the GPT-4 model (gpt-4). You can switch to "gpt-3" for cheaper tokens and better response time in the script settings.

## Installation

To use it on UNIX-based systems, all you need to do is to compile/download the binary and run

```bash
cargo build -r 
sudo cp target/release/ask /bin/ask
```

## Usage and Examples

First off, be sure to configure your API key in an environment variable, like you do for scripts such as avante.nvim.

The operating principle is very simple. Call the program, wait for a response and answer at will. Customize the program's behaviour in ask.json, in your .config dir.

![image](https://github.com/user-attachments/assets/8ef71d4a-090b-41af-bc70-cf3e32c83ddc)

`ask "Hi there"` - Prompts the model.

`ask Unqouted strings work too!` - Prompts the model.

`ask Hey there. Can you help me interpret the contents of this directory? $(ls -la)` - Prompts the model with interpolated shell output (Syntax may vary. Example is in bash).

`ask` - Displays the current conversation state.

`ask -c` - Clears current conversation

`ask -C` - Clears all conversations

`ask -o` - Manages ongoing session.

`ask -i - Passes image on the clipboard to the model (Configure clipboard extraction command. Ask is configured to use xclip by default)`

`cat some_file.c | ask "What does this code do?"` - Parses file then question passed as argument.

`ask -r` - Enters interactive agent mode. The model will keep trying to follow your instructions in the shell until it deems its task isfinished.

## Sample ask.json schema

```JSON
{
  "model": "o1",
  "host": "api.openai.com",
  "api_key_variable": "OPENAI_API_KEY",
  "endpoint": "/v1/chat/completions",
  "max_tokens": 2048,
  "temperature": 0.6,
  "vision_detail": "high",
  "editor": "more",
  "transcript_name": "gpt_transcript-",
  "clipboard_command_xorg": "xclip -selection clipboard -t image/png -o",
  "clipboard_command_wayland": "wl-paste",
  "clipboard_command_unsupported": "UNSUPPORTED",
  "startup_message": "You are ChatConcise, a very advanced LLM designed for experienced users. As ChatConcise you oblige to adhere to the following directives UNLESS overridden by the user:\nBe concise, proactive, helpful and efficient. Do not say anything more than what needed, but also, DON'T BE LAZY. If the user is asking for software, provide ONLY the code."
}
```
