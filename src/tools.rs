use serde_json::{json, Value};
use std::collections::HashMap;
use std::fs;
use std::io::{self, Write};
use std::path::Path;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use scraper::{Html, Selector};

static AUTO_APPROVE_COMMANDS: AtomicBool = AtomicBool::new(false);

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
    registry.register(Box::new(EditFileTool));
    registry.register(Box::new(WebSearchTool));
    registry.register(Box::new(WebPageReaderTool));
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
        "Read the contents of a file at the given path. Output includes line numbers (e.g. '  42: code here') which can be used with edit_file's line-range mode. Supports chunked reading with offset/limit, and case-insensitive string search with context."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "The path to the file to read"
                },
                "offset": {
                    "type": "integer",
                    "description": "Starting line number (1-indexed). If not specified, starts from line 1."
                },
                "limit": {
                    "type": "integer",
                    "description": "Maximum number of lines to read. If not specified, reads all remaining lines."
                },
                "search": {
                    "type": "string",
                    "description": "Case-insensitive string to search for. Returns only matching lines with surrounding context."
                },
                "context_lines": {
                    "type": "integer",
                    "description": "Number of lines to show before and after each search match (default: 3). Only used with 'search'."
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

        let content = fs::read_to_string(file_path)
            .map_err(|e| format!("Failed to read file '{}': {}", path, e))?;

        let lines: Vec<&str> = content.lines().collect();
        let total_lines = lines.len();

        // Handle search mode
        if let Some(search_term) = args.get("search").and_then(|s| s.as_str()) {
            let context = args
                .get("context_lines")
                .and_then(|c| c.as_u64())
                .unwrap_or(6) as usize;

            let search_lower = search_term.to_lowercase();
            let mut matches: Vec<(usize, &str)> = Vec::new();

            // Find all matching lines
            for (idx, line) in lines.iter().enumerate() {
                if line.to_lowercase().contains(&search_lower) {
                    matches.push((idx, line));
                }
            }

            if matches.is_empty() {
                return Ok(format!("No matches found for '{}' in {}", search_term, path));
            }

            // Build output with context
            let mut result = format!("Found {} match(es) for '{}' in {}:\n\n", matches.len(), search_term, path);
            let mut shown_lines: std::collections::HashSet<usize> = std::collections::HashSet::new();

            for (match_idx, _) in &matches {
                let start = match_idx.saturating_sub(context);
                let end = (match_idx + context + 1).min(total_lines);

                // Add separator if there's a gap from previous context
                if !shown_lines.is_empty() {
                    let last_shown = *shown_lines.iter().max().unwrap();
                    if start > last_shown + 1 {
                        result.push_str("  ...\n");
                    }
                }

                for i in start..end {
                    if shown_lines.contains(&i) {
                        continue;
                    }
                    shown_lines.insert(i);

                    let line_num = i + 1; // 1-indexed
                    let marker = if i == *match_idx { ">" } else { " " };
                    result.push_str(&format!("{} {:>4}: {}\n", marker, line_num, lines[i]));
                }
            }

            return Ok(result);
        }

        // Handle chunked reading mode
        let offset = args
            .get("offset")
            .and_then(|o| o.as_u64())
            .map(|o| (o.saturating_sub(1)) as usize) // Convert to 0-indexed
            .unwrap_or(0);

        let limit = args
            .get("limit")
            .and_then(|l| l.as_u64())
            .map(|l| l as usize);

        if offset >= total_lines {
            return Ok(format!("Offset {} exceeds file length ({} lines)", offset + 1, total_lines));
        }

        let end = match limit {
            Some(l) => (offset + l).min(total_lines),
            None => total_lines,
        };

        let mut result = String::new();

        // Add header with line range info if using chunked reading
        if offset > 0 || limit.is_some() {
            result.push_str(&format!("Lines {}-{} of {} total:\n\n", offset + 1, end, total_lines));
        }

        for (idx, line) in lines[offset..end].iter().enumerate() {
            let line_num = offset + idx + 1; // 1-indexed
            result.push_str(&format!("{:>4}: {}\n", line_num, line));
        }

        Ok(result)
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
        "Search the web using DuckDuckGo. Returns a list of relevant URLs with titles and snippets. Use this to find sources, then use web_read_page to read the content."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "The search keywords"
                }
            },
            "required": ["query"]
        })
    }

    fn execute(&self, args: &Value) -> Result<String, String> {
        let query = args.get("query")
            .and_then(|q| q.as_str())
            .ok_or("Missing 'query' argument")?;

        // 1. Fetch the Search Results
        let encoded_query = urlencoding::encode(query);
        let url = format!("https://lite.duckduckgo.com/lite/?q={}", encoded_query);

        let response = ureq::get(&url)
            .set("User-Agent", "Mozilla/5.0 (compatible; ask-rs/1.0)")
            .call()
            .map_err(|e| format!("Search request failed: {}", e))?;

        let body = response.into_string()
            .map_err(|e| format!("Failed to read response: {}", e))?;

        // 2. Robust Parsing with 'scraper'
        let document = Html::parse_document(&body);
        
        // DuckDuckGo Lite structure:
        // Results are in a table. Links have class 'result-link'.
        // Snippets are in the row immediately following the link.
        let result_link_selector = Selector::parse(".result-link").unwrap();
        let snippet_selector = Selector::parse(".result-snippet").unwrap();

        let mut formatted_results = Vec::new();

        // We zip the iterators because DDG Lite usually alternates Link Row -> Snippet Row
        let links = document.select(&result_link_selector);
        let snippets = document.select(&snippet_selector);

        for (link_element, snippet_element) in links.zip(snippets).take(5) {
            let title = link_element.text().collect::<Vec<_>>().join(" ");
            let href = link_element.value().attr("href").unwrap_or_default();
            let snippet = snippet_element.text().collect::<Vec<_>>().join(" ");

            // Skip internal DDG links or empty results
            if href.is_empty() || title.is_empty() {
                continue;
            }

            formatted_results.push(json!({
                "title": title.trim(),
                "url": href,
                "snippet": snippet.trim()
            }));
        }

        if formatted_results.is_empty() {
            return Ok("No results found.".to_string());
        }

        // Return JSON string so the AI can parse the URLs programmatically
        Ok(serde_json::to_string_pretty(&formatted_results)
           .map_err(|e| e.to_string())?)
    }
}

pub struct WebPageReaderTool;

impl Tool for WebPageReaderTool {
    fn name(&self) -> &'static str {
        "web_read_page"
    }

    fn description(&self) -> &'static str {
        "Reads the full content of a specific webpage URL. Use this after web_search to get the details of a chosen result."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "url": {
                    "type": "string",
                    "description": "The URL of the page to read (obtained from web_search)"
                }
            },
            "required": ["url"]
        })
    }

    fn execute(&self, args: &Value) -> Result<String, String> {
        let url = args.get("url")
            .and_then(|u| u.as_str())
            .ok_or("Missing 'url' argument")?;

        // 1. Fetch the Page
        let response = ureq::get(url)
            .set("User-Agent", "Mozilla/5.0 (compatible; ask-rs/1.0)")
            .call()
            .map_err(|e| format!("Failed to fetch page: {}", e))?;

        let body = response.into_string()
            .map_err(|e| format!("Failed to read page body: {}", e))?;

        // 2. Convert HTML to Readable Text
        // width: 80 is standard for readability, usually fits nicely in context windows
        let clean_text = html2text::from_read(body.as_bytes(), 80);

        // Optional: Truncate if the page is massive to prevent context overflow
        // e.g., take the first 10,000 characters
        let max_len = 10_000;
        if clean_text.len() > max_len {
            Ok(format!("{}...\n\n(Content truncated)", &clean_text[..max_len]))
        } else {
            Ok(clean_text)
        }
    }
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

pub struct EditFileTool;

impl Tool for EditFileTool {
    fn name(&self) -> &'static str {
        "edit_file"
    }

    fn description(&self) -> &'static str {
        "Edit a file by replacing specific text or a line range. Use read_file first to see the file with line numbers, then apply targeted edits. Two modes:\n\
         1) Search-and-replace: provide 'old_string' and 'new_string' to find and replace exact text.\n\
         2) Line-range: provide 'start_line', 'end_line', and 'new_string' to replace a range of lines.\n\
         For inserting new content without removing lines, set start_line and end_line to the same line and include the original line in new_string.\n\
         Always prefer this over write_file for modifying existing files."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "The path to the file to edit"
                },
                "old_string": {
                    "type": "string",
                    "description": "The exact text to find and replace (for search-and-replace mode). Must match the file content exactly, including whitespace and indentation."
                },
                "new_string": {
                    "type": "string",
                    "description": "The replacement text. Used in both modes. Use empty string to delete text/lines."
                },
                "start_line": {
                    "type": "integer",
                    "description": "Starting line number (1-indexed, inclusive) for line-range mode. Use with end_line."
                },
                "end_line": {
                    "type": "integer",
                    "description": "Ending line number (1-indexed, inclusive) for line-range mode. Use with start_line."
                },
                "replace_all": {
                    "type": "boolean",
                    "description": "If true, replace ALL occurrences of old_string. Default is false (only first occurrence). Only used in search-and-replace mode."
                }
            },
            "required": ["path", "new_string"]
        })
    }

    fn execute(&self, args: &Value) -> Result<String, String> {
        let path = args
            .get("path")
            .and_then(|p| p.as_str())
            .ok_or("Missing 'path' argument")?;

        let new_string = args
            .get("new_string")
            .and_then(|s| s.as_str())
            .ok_or("Missing 'new_string' argument")?;

        let expanded_path = shellexpand::tilde(path);
        let file_path = Path::new(expanded_path.as_ref());

        if !file_path.exists() {
            return Err(format!("File not found: {}", path));
        }

        let content = fs::read_to_string(file_path)
            .map_err(|e| format!("Failed to read file '{}': {}", path, e))?;

        let old_string = args.get("old_string").and_then(|s| s.as_str());
        let start_line = args.get("start_line").and_then(|n| n.as_u64()).map(|n| n as usize);
        let end_line = args.get("end_line").and_then(|n| n.as_u64()).map(|n| n as usize);

        let new_content = if let Some(old_str) = old_string {
            // Search-and-replace mode
            if old_str == new_string {
                return Err("old_string and new_string are identical — no change needed.".to_string());
            }

            if !content.contains(old_str) {
                // Provide helpful diagnostics
                let trimmed = old_str.trim();
                if !trimmed.is_empty() && content.contains(trimmed) {
                    return Err(format!(
                        "Exact match not found for old_string, but a match was found ignoring leading/trailing whitespace. \
                         Make sure old_string matches the file content exactly, including indentation. \
                         Use read_file to see the exact content."
                    ));
                }
                return Err(format!(
                    "old_string not found in '{}'. Use read_file to verify the exact content you want to replace.",
                    path
                ));
            }

            let replace_all = args
                .get("replace_all")
                .and_then(|b| b.as_bool())
                .unwrap_or(false);

            if replace_all {
                let count = content.matches(old_str).count();
                let result = content.replace(old_str, new_string);
                fs::write(file_path, &result)
                    .map_err(|e| format!("Failed to write file '{}': {}", path, e))?;
                return Ok(format!(
                    "Replaced all {} occurrence(s) of the specified text in '{}'.",
                    count, path
                ));
            } else {
                let count = content.matches(old_str).count();
                let result = content.replacen(old_str, new_string, 1);
                fs::write(file_path, &result)
                    .map_err(|e| format!("Failed to write file '{}': {}", path, e))?;
                if count > 1 {
                    return Ok(format!(
                        "Replaced first occurrence of the specified text in '{}'. Note: {} total occurrences exist; use replace_all=true to replace all.",
                        path, count
                    ));
                }
                return Ok(format!("Replaced the specified text in '{}'.", path));
            }
        } else if let (Some(start), Some(end)) = (start_line, end_line) {
            // Line-range mode
            let lines: Vec<&str> = content.lines().collect();
            let total_lines = lines.len();

            if start == 0 || end == 0 {
                return Err("Line numbers are 1-indexed. Use start_line >= 1 and end_line >= 1.".to_string());
            }
            if start > total_lines {
                return Err(format!(
                    "start_line {} exceeds file length ({} lines).",
                    start, total_lines
                ));
            }
            if end > total_lines {
                return Err(format!(
                    "end_line {} exceeds file length ({} lines).",
                    end, total_lines
                ));
            }
            if start > end {
                return Err(format!(
                    "start_line ({}) must be <= end_line ({}).",
                    start, end
                ));
            }

            // Build new content: lines before range + new text + lines after range
            let mut result = String::new();

            // Lines before the range (0-indexed: 0..start-1)
            for line in &lines[..start - 1] {
                result.push_str(line);
                result.push('\n');
            }

            // Insert new content
            if !new_string.is_empty() {
                result.push_str(new_string);
                if !new_string.ends_with('\n') {
                    result.push('\n');
                }
            }

            // Lines after the range (0-indexed: end..)
            for (i, line) in lines[end..].iter().enumerate() {
                result.push_str(line);
                // Add newline for all but potentially the last line,
                // preserving whether original file ended with newline
                if end + i + 1 < total_lines || content.ends_with('\n') {
                    result.push('\n');
                }
            }

            result
        } else {
            return Err(
                "Must provide either 'old_string' (search-and-replace mode) or both 'start_line' and 'end_line' (line-range mode).".to_string()
            );
        };

        let lines_before = content.lines().count();
        let lines_after = new_content.lines().count();

        fs::write(file_path, &new_content)
            .map_err(|e| format!("Failed to write file '{}': {}", path, e))?;

        let diff = lines_after as i64 - lines_before as i64;
        let diff_str = if diff > 0 {
            format!("+{} lines", diff)
        } else if diff < 0 {
            format!("{} lines", diff)
        } else {
            "same line count".to_string()
        };

        Ok(format!(
            "Edited '{}': {} lines -> {} lines ({}).",
            path, lines_before, lines_after, diff_str
        ))
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

        // Skip prompt if user previously chose "always allow"
        if !AUTO_APPROVE_COMMANDS.load(Ordering::Relaxed) {
            // Request user approval
            println!("\n\x1b[33m> The agent wants to execute the following command:\x1b[0m");
            println!("\x1b[36m{}\x1b[0m", command_str);
            print!("\x1b[33m> Do you approve this execution? [y/N/a]: \x1b[0m");
            io::stdout().flush().map_err(|e| format!("Failed to flush stdout: {}", e))?;

            let mut input = String::new();
            io::stdin().read_line(&mut input).map_err(|e| format!("Failed to read input: {}", e))?;

            match input.trim().to_lowercase().as_str() {
                "a" => AUTO_APPROVE_COMMANDS.store(true, Ordering::Relaxed),
                "y" => {}
                _ => return Err("User denied command execution.".to_string()),
            }
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
