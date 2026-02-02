use serde_json::{json, Value};
use std::collections::HashMap;
use std::fs;
use std::io::{self, Write};
use std::path::Path;
use std::process::Command;

/// A tool that can be called by the LLM
pub trait Tool: Send + Sync {
    fn name(&self) -> &'static str;
    fn description(&self) -> &'static str;
    fn parameters(&self) -> Value;
    fn execute(&self, args: &Value) -> Result<String, String>;
}

/// Registry holding all available tools
pub struct ToolRegistry {
    tools: HashMap<String, Box<dyn Tool>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self {
            tools: HashMap::new(),
        }
    }

    pub fn register(&mut self, tool: Box<dyn Tool>) {
        self.tools.insert(tool.name().to_string(), tool);
    }

    pub fn get(&self, name: &str) -> Option<&dyn Tool> {
        self.tools.get(name).map(|t| t.as_ref())
    }

    pub fn to_openai_format(&self) -> Vec<Value> {
        self.tools
            .values()
            .map(|tool| {
                json!({
                    "type": "function",
                    "function": {
                        "name": tool.name(),
                        "description": tool.description(),
                        "parameters": tool.parameters()
                    }
                })
            })
            .collect()
    }

    pub fn to_gemini_format(&self) -> Value {
        let functions: Vec<Value> = self.tools
            .values()
            .map(|tool| {
                json!({
                    "name": tool.name(),
                    "description": tool.description(),
                    "parameters": tool.parameters()
                })
            })
            .collect();

        json!({
            "function_declarations": functions
        })
    }

    pub fn execute(&self, name: &str, args: &Value) -> Result<String, String> {
        match self.get(name) {
            Some(tool) => tool.execute(args),
            None => Err(format!("Unknown tool: {}", name)),
        }
    }
}

/// Creates a registry with all default tools
pub fn create_default_registry() -> ToolRegistry {
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(ReadFileTool));
    registry.register(Box::new(WriteFileTool));
    registry.register(Box::new(WebSearchTool));
    registry.register(Box::new(SearchFilesTool));
    registry.register(Box::new(ExecuteCommandTool));
    registry
}

// ============================================================================ 
// Tool Implementations
// ============================================================================ 

pub struct ReadFileTool;

impl Tool for ReadFileTool {
    fn name(&self) -> &'static str {
        "read_file"
    }

    fn description(&self) -> &'static str {
        "Read the contents of a file at the given path. Returns the file content as text."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "The path to the file to read"
                }
            },
            "required": ["path"]
        })
    }

    fn execute(&self, args: &Value) -> Result<String, String> {
        let path = args
            .get("path")
            .and_then(|p| p.as_str())
            .ok_or("Missing 'path' argument")?;

        let expanded_path = shellexpand::tilde(path);
        let file_path = Path::new(expanded_path.as_ref());

        if !file_path.exists() {
            return Err(format!("File not found: {}", path));
        }

        fs::read_to_string(file_path)
            .map_err(|e| format!("Failed to read file '{}': {}", path, e))
    }
}

pub struct WriteFileTool;

impl Tool for WriteFileTool {
    fn name(&self) -> &'static str {
        "write_file"
    }

    fn description(&self) -> &'static str {
        "Write content to a file at the given path. Creates the file if it doesn't exist, overwrites if it does."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "The path to the file to write"
                },
                "content": {
                    "type": "string",
                    "description": "The content to write to the file"
                }
            },
            "required": ["path", "content"]
        })
    }

    fn execute(&self, args: &Value) -> Result<String, String> {
        let path = args
            .get("path")
            .and_then(|p| p.as_str())
            .ok_or("Missing 'path' argument")?;

        let content = args
            .get("content")
            .and_then(|c| c.as_str())
            .ok_or("Missing 'content' argument")?;

        let expanded_path = shellexpand::tilde(path);
        let file_path = Path::new(expanded_path.as_ref());

        // Create parent directories if they don't exist
        if let Some(parent) = file_path.parent() {
            if !parent.exists() {
                fs::create_dir_all(parent)
                    .map_err(|e| format!("Failed to create directories: {}", e))?;
            }
        }

        fs::write(file_path, content)
            .map_err(|e| format!("Failed to write file '{}': {}", path, e))?;

        Ok(format!("Successfully wrote {} bytes to '{}'", content.len(), path))
    }
}

pub struct WebSearchTool;

impl Tool for WebSearchTool {
    fn name(&self) -> &'static str {
        "web_search"
    }

    fn description(&self) -> &'static str {
        "Search the web for information using DuckDuckGo. Returns search results as text."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "The search query"
                }
            },
            "required": ["query"]
        })
    }

    fn execute(&self, args: &Value) -> Result<String, String> {
        let query = args
            .get("query")
            .and_then(|q| q.as_str())
            .ok_or("Missing 'query' argument")?;

        // Use DuckDuckGo's lite HTML version for simple scraping
        let encoded_query = urlencoding::encode(query);
        let url = format!("https://lite.duckduckgo.com/lite/?q={}", encoded_query);

        let response = ureq::get(&url)
            .set("User-Agent", "Mozilla/5.0 (compatible; ask-rs/1.0)")
            .call()
            .map_err(|e| format!("Search request failed: {}", e))?;

        let body = response
            .into_string()
            .map_err(|e| format!("Failed to read response: {}", e))?;

        // Parse the HTML to extract search results
        let results = parse_ddg_lite_results(&body);

        if results.is_empty() {
            Ok(format!("No results found for: {}", query))
        } else {
            Ok(results.join("\n\n"))
        }
    }
}

/// Parse DuckDuckGo lite HTML to extract search results
fn parse_ddg_lite_results(html: &str) -> Vec<String> {
    let mut results = Vec::new();

    // DDG lite uses simple HTML tables. We look for links and their descriptions.
    // This is a simple parser - for production you'd want a proper HTML parser.

    // Find result links (they're in <a> tags with class="result-link")
    // DDG lite format: links are followed by snippets in the next table row

    let mut current_title = String::new();
    let mut current_url = String::new();

    for line in html.lines() {
        let line = line.trim();

        // Look for result links
        if line.contains("class=\"result-link\"") || line.contains("class='result-link'") {
            // Extract href and text
            if let Some(href_start) = line.find("href=\"").or_else(|| line.find("href='")) {
                let quote_char = if line.contains("href=\"") { '"' } else { '\'' };
                let href_content = &line[href_start + 6..];
                if let Some(href_end) = href_content.find(quote_char) {
                    current_url = href_content[..href_end].to_string();
                }
            }
            // Extract link text
            if let Some(gt_pos) = line.rfind('>') {
                let text_part = &line[gt_pos + 1..];
                if let Some(lt_pos) = text_part.find('<') {
                    current_title = text_part[..lt_pos].to_string();
                }
            }
        }

        // Look for result snippets (class="result-snippet")
        if (line.contains("class=\"result-snippet\"") || line.contains("class='result-snippet'"))
            && !current_title.is_empty()
        {
            // Extract snippet text
            let snippet = extract_text_from_html_line(line);
            if !snippet.is_empty() {
                results.push(format!(
                    "**{}**\n{}\n{}",
                    html_decode(&current_title),
                    html_decode(&snippet),
                    current_url
                ));
                current_title.clear();
                current_url.clear();
            }
        }
    }

    // Limit results
    results.truncate(5);
    results
}

fn extract_text_from_html_line(line: &str) -> String {
    let mut result = String::new();
    let mut in_tag = false;

    for ch in line.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => result.push(ch),
            _ => {} // Ignore other characters when inside a tag
        }
    }

    result.trim().to_string()
}

fn html_decode(s: &str) -> String {
    s.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&nbsp;", " ")
}

pub struct SearchFilesTool;

impl Tool for SearchFilesTool {
    fn name(&self) -> &'static str {
        "search_files"
    }

    fn description(&self) -> &'static str {
        "Recursively search for files matching a pattern using the 'find' command."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "The directory to search in (defaults to current directory)"
                },
                "pattern": {
                    "type": "string",
                    "description": "Glob pattern to match files (e.g. '*.rs')"
                }
            },
            "required": []
        })
    }

    fn execute(&self, args: &Value) -> Result<String, String> {
        let path = args
            .get("path")
            .and_then(|p| p.as_str())
            .unwrap_or(".");
        
        let pattern = args
            .get("pattern")
            .and_then(|p| p.as_str());

        let expanded_path = shellexpand::tilde(path);
        
        let mut command = Command::new("find");
        command.arg(expanded_path.as_ref());

        if let Some(p) = pattern {
            command.arg("-name").arg(p);
        }
        
        // Exclude .git directory by default to reduce noise
        command.arg("-not").arg("-path").arg("*/.git/*");

        let output = command
            .output()
            .map_err(|e| format!("Failed to execute find command: {}", e))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(format!("Search failed: {}", stderr));
        }

        let result = String::from_utf8_lossy(&output.stdout).to_string();
        if result.trim().is_empty() {
            Ok("No files found matching the criteria.".to_string())
        } else {
            Ok(result)
        }
    }
}

pub struct ExecuteCommandTool;

impl Tool for ExecuteCommandTool {
    fn name(&self) -> &'static str {
        "run_shell_command"
    }

    fn description(&self) -> &'static str {
        "Execute a shell command on the system. Requires explicit user approval for each execution."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "The shell command to execute"
                }
            },
            "required": ["command"]
        })
    }

    fn execute(&self, args: &Value) -> Result<String, String> {
        let command_str = args
            .get("command")
            .and_then(|c| c.as_str())
            .ok_or("Missing 'command' argument")?;

        // Request user approval
        println!("\n\x1b[33m> The agent wants to execute the following command:\x1b[0m");
        println!("\x1b[36m{}\x1b[0m", command_str);
        print!("\x1b[33m> Do you approve this execution? [y/N]: \x1b[0m");
        io::stdout().flush().map_err(|e| format!("Failed to flush stdout: {}", e))?;

        let mut input = String::new();
        io::stdin().read_line(&mut input).map_err(|e| format!("Failed to read input: {}", e))?;

        if input.trim().to_lowercase() != "y" {
            return Err("User denied command execution.".to_string());
        }

        let output = Command::new("sh")
            .arg("-c")
            .arg(command_str)
            .output()
            .map_err(|e| format!("Failed to execute command: {}", e))?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        let mut result = String::new();
        if !stdout.is_empty() {
            result.push_str(&format!("Stdout:\n{}\n", stdout));
        }
        if !stderr.is_empty() {
            result.push_str(&format!("Stderr:\n{}\n", stderr));
        }
        
        if result.is_empty() {
            result = "(Command executed successfully with no output)".to_string();
        }

        if !output.status.success() {
             result.push_str(&format!("\nCommand failed with exit code: {}", output.status));
        }

        Ok(result)
    }
}