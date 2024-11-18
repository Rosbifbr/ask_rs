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
First off, be sure to configure your OPENAI_API_KEY environment variable, like scripts such as avante.nvim

The operating principle is very simple. Call the program, wait for a response and answer at will.

![image](https://github.com/user-attachments/assets/8ef71d4a-090b-41af-bc70-cf3e32c83ddc)

`ask "Hi there"` - Prompts the model.

`ask Unqouted strings work too!` - Prompts the model.

`ask Hey there. Can you help me interpret the contents of this directory? $(ls -la)` - Prompts the model with interpolated shell output (Syntax may vary. Example is in bash).

`ask` - Displays the current conversation state.

`ask -c` - Clears current conversation

`ask -o` - Manages ongoing session. 

`ask -i - Passes image on the clipboard to the model (Configure clipboard extraction command. Ask is configured to use xclip by default)`

`cat some_file.c | ask "What does this code do?"` - Parses file then question passed as argument.

ask -r - Enters interactive agent mode. The model will keep trying to follow your instructions in the shell until you kill it.
