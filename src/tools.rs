use scraper::{Html, Selector};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, UNIX_EPOCH};

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

    pub fn to_responses_format(&self) -> Vec<Value> {
        self.tools
            .values()
            .map(|tool| {
                json!({
                    "type": "function",
                    "name": tool.name(),
                    "description": tool.description(),
                    "parameters": tool.parameters()
                })
            })
            .collect()
    }

    pub fn to_gemini_format(&self) -> Value {
        let functions: Vec<Value> = self
            .tools
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
    registry.register(Box::new(ApplyPatchTool));
    registry.register(Box::new(FileInfoTool));
    registry.register(Box::new(WebSearchTool));
    registry.register(Box::new(WebPageReaderTool));
    registry.register(Box::new(WebFetchRawTool));
    registry.register(Box::new(SearchFilesTool));
    registry.register(Box::new(SearchTextTool));
    registry.register(Box::new(ListDirTool));
    registry.register(Box::new(ExecuteCommandTool));
    registry
}

fn resolve_path(path: &str) -> Result<PathBuf, String> {
    let expanded = shellexpand::tilde(path);
    let raw = Path::new(expanded.as_ref());
    let base = if raw.is_absolute() {
        raw.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|e| format!("Failed to get current directory: {}", e))?
            .join(raw)
    };

    // Lexically normalize without requiring the target path to exist.
    let mut normalized = PathBuf::new();
    for component in base.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            _ => normalized.push(component.as_os_str()),
        }
    }
    Ok(normalized)
}

fn should_skip_path(path: &Path) -> bool {
    path.components().any(|component| {
        let name = component.as_os_str().to_string_lossy();
        matches!(
            name.as_ref(),
            ".git" | "target" | "node_modules" | ".DS_Store" | "__pycache__"
        )
    })
}

fn truncate_output(output: String, max_bytes: usize) -> String {
    if output.len() <= max_bytes {
        return output;
    }

    let mut end = max_bytes;
    while !output.is_char_boundary(end) {
        end -= 1;
    }
    format!(
        "{}\n\n[Output truncated at {} bytes; narrow the query for more detail.]",
        &output[..end],
        max_bytes
    )
}

fn simple_name_match(name: &str, pattern: &str) -> bool {
    if pattern == "*" || pattern.is_empty() {
        return true;
    }
    if let Some(suffix) = pattern.strip_prefix("*.") {
        return name.ends_with(&format!(".{}", suffix));
    }
    if pattern.contains('*') {
        let parts: Vec<&str> = pattern.split('*').filter(|part| !part.is_empty()).collect();
        let mut remaining = name;
        for part in parts {
            if let Some(idx) = remaining.find(part) {
                remaining = &remaining[idx + part.len()..];
            } else {
                return false;
            }
        }
        return true;
    }
    name.contains(pattern)
}

fn collect_files(
    root: &Path,
    pattern: Option<&str>,
    max_results: usize,
) -> Result<(Vec<PathBuf>, bool), String> {
    let mut results = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    let mut truncated = false;

    while let Some(path) = stack.pop() {
        if should_skip_path(&path) {
            continue;
        }

        let entries = match fs::read_dir(&path) {
            Ok(entries) => entries,
            Err(_) => continue,
        };

        for entry in entries.filter_map(|entry| entry.ok()) {
            let entry_path = entry.path();
            if should_skip_path(&entry_path) {
                continue;
            }

            if entry_path.is_dir() {
                stack.push(entry_path);
            } else if entry_path.is_file() {
                let name = entry_path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("");
                if pattern.map_or(true, |pattern| simple_name_match(name, pattern)) {
                    results.push(entry_path);
                    if results.len() >= max_results {
                        truncated = true;
                        return Ok((results, truncated));
                    }
                }
            }
        }
    }

    results.sort();
    Ok((results, truncated))
}

fn count_line_delta(old: &str, new: &str) -> (usize, usize, i64) {
    let old_lines = old.lines().count();
    let new_lines = new.lines().count();
    (old_lines, new_lines, new_lines as i64 - old_lines as i64)
}

fn summarize_content_change(path: &Path, old: &str, new: &str) -> String {
    let (old_lines, new_lines, delta) = count_line_delta(old, new);
    let old_bytes = old.len();
    let new_bytes = new.len();
    format!(
        "{}: {} -> {} lines ({:+}), {} -> {} bytes ({:+})",
        path.display(),
        old_lines,
        new_lines,
        delta,
        old_bytes,
        new_bytes,
        new_bytes as i64 - old_bytes as i64
    )
}

fn is_probably_binary(bytes: &[u8]) -> bool {
    bytes.iter().any(|byte| *byte == 0)
}

struct ParsedPatch {
    operations: Vec<PatchOperation>,
}

enum PatchOperation {
    Add { path: String, content: String },
    Delete { path: String },
    Update { path: String, hunks: Vec<PatchHunk> },
}

struct PatchHunk {
    old: String,
    new: String,
}

fn parse_patch(patch: &str) -> Result<ParsedPatch, String> {
    let lines: Vec<&str> = patch.lines().collect();
    if lines.first().map(|line| line.trim()) != Some("*** Begin Patch") {
        return Err("Patch must start with '*** Begin Patch'.".to_string());
    }
    if lines.last().map(|line| line.trim()) != Some("*** End Patch") {
        return Err("Patch must end with '*** End Patch'.".to_string());
    }

    let mut operations = Vec::new();
    let mut i = 1;
    while i + 1 < lines.len() {
        let line = lines[i];
        if let Some(path) = line.strip_prefix("*** Add File: ") {
            i += 1;
            let mut content = String::new();
            while i + 1 < lines.len() && !lines[i].starts_with("*** ") {
                let add_line = lines[i]
                    .strip_prefix('+')
                    .ok_or("Add File content lines must start with '+'.")?;
                content.push_str(add_line);
                content.push('\n');
                i += 1;
            }
            operations.push(PatchOperation::Add {
                path: path.to_string(),
                content,
            });
        } else if let Some(path) = line.strip_prefix("*** Delete File: ") {
            operations.push(PatchOperation::Delete {
                path: path.to_string(),
            });
            i += 1;
        } else if let Some(path) = line.strip_prefix("*** Update File: ") {
            i += 1;
            let mut hunks = Vec::new();
            let mut old = String::new();
            let mut new = String::new();
            let mut in_hunk = false;

            while i + 1 < lines.len() && !lines[i].starts_with("*** ") {
                let hunk_line = lines[i];
                if hunk_line.starts_with("@@") {
                    if in_hunk {
                        hunks.push(PatchHunk { old, new });
                        old = String::new();
                        new = String::new();
                    }
                    in_hunk = true;
                } else if in_hunk {
                    if let Some(text) = hunk_line.strip_prefix(' ') {
                        old.push_str(text);
                        old.push('\n');
                        new.push_str(text);
                        new.push('\n');
                    } else if let Some(text) = hunk_line.strip_prefix('-') {
                        old.push_str(text);
                        old.push('\n');
                    } else if let Some(text) = hunk_line.strip_prefix('+') {
                        new.push_str(text);
                        new.push('\n');
                    } else if hunk_line == r"\ No newline at end of file" {
                        if old.ends_with('\n') {
                            old.pop();
                        }
                        if new.ends_with('\n') {
                            new.pop();
                        }
                    } else if !hunk_line.trim().is_empty() {
                        return Err(format!(
                            "Update hunk lines must start with ' ', '+', '-', or '@@': {}",
                            hunk_line
                        ));
                    }
                } else if hunk_line.starts_with("*** Move to: ") {
                    return Err("Move/rename patches are not supported yet.".to_string());
                } else if !hunk_line.trim().is_empty() {
                    return Err("Update File sections require '@@' before hunk lines.".to_string());
                }
                i += 1;
            }

            if in_hunk {
                hunks.push(PatchHunk { old, new });
            }
            if hunks.is_empty() {
                return Err(format!("Update File '{}' has no hunks.", path));
            }
            operations.push(PatchOperation::Update {
                path: path.to_string(),
                hunks,
            });
        } else if line.trim().is_empty() {
            i += 1;
        } else {
            return Err(format!("Unknown patch header: {}", line));
        }
    }

    if operations.is_empty() {
        return Err("Patch contains no file operations.".to_string());
    }
    Ok(ParsedPatch { operations })
}

fn apply_parsed_patch(parsed: ParsedPatch, dry_run: bool) -> Result<String, String> {
    let mut planned_writes: Vec<(PathBuf, Option<String>, String)> = Vec::new();

    for operation in parsed.operations {
        match operation {
            PatchOperation::Add { path, content } => {
                let file_path = resolve_path(&path)?;
                if file_path.exists() {
                    return Err(format!(
                        "Add File target already exists: {}",
                        file_path.display()
                    ));
                }
                planned_writes.push((
                    file_path.clone(),
                    Some(content.clone()),
                    format!(
                        "add {} ({} lines)",
                        file_path.display(),
                        content.lines().count()
                    ),
                ));
            }
            PatchOperation::Delete { path } => {
                let file_path = resolve_path(&path)?;
                if !file_path.is_file() {
                    return Err(format!(
                        "Delete File target is not a file: {}",
                        file_path.display()
                    ));
                }
                let old = fs::read_to_string(&file_path)
                    .map_err(|e| format!("Failed to read '{}': {}", file_path.display(), e))?;
                planned_writes.push((
                    file_path.clone(),
                    None,
                    format!(
                        "delete {} ({} lines)",
                        file_path.display(),
                        old.lines().count()
                    ),
                ));
            }
            PatchOperation::Update { path, hunks } => {
                let file_path = resolve_path(&path)?;
                if !file_path.is_file() {
                    return Err(format!(
                        "Update File target is not a file: {}",
                        file_path.display()
                    ));
                }
                let original = fs::read_to_string(&file_path)
                    .map_err(|e| format!("Failed to read '{}': {}", file_path.display(), e))?;
                let mut updated = original.clone();

                for hunk in hunks {
                    if hunk.old.is_empty() {
                        return Err(format!(
                            "Refusing empty-context update hunk for '{}'. Add surrounding context lines.",
                            file_path.display()
                        ));
                    }
                    let matches = updated.matches(&hunk.old).count();
                    if matches != 1 {
                        return Err(format!(
                            "Patch hunk for '{}' matched {} times; expected exactly 1.",
                            file_path.display(),
                            matches
                        ));
                    }
                    updated = updated.replacen(&hunk.old, &hunk.new, 1);
                }

                planned_writes.push((
                    file_path.clone(),
                    Some(updated.clone()),
                    summarize_content_change(&file_path, &original, &updated),
                ));
            }
        }
    }

    if !dry_run {
        for (path, content, _) in &planned_writes {
            if let Some(content) = content {
                if let Some(parent) = path.parent() {
                    if !parent.exists() {
                        fs::create_dir_all(parent).map_err(|e| {
                            format!("Failed to create '{}': {}", parent.display(), e)
                        })?;
                    }
                }
                fs::write(path, content)
                    .map_err(|e| format!("Failed to write '{}': {}", path.display(), e))?;
            } else {
                fs::remove_file(path)
                    .map_err(|e| format!("Failed to delete '{}': {}", path.display(), e))?;
            }
        }
    }

    let mut result = if dry_run {
        "Patch applies cleanly. No files were changed.\n".to_string()
    } else {
        "Patch applied successfully.\n".to_string()
    };
    for (_, _, summary) in planned_writes {
        result.push_str(&format!("- {}\n", summary));
    }
    Ok(result)
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
        "Read a file with optional line-windowing. By default output includes line numbers for navigation/editing; set raw=true for exact file content without line-number decoration. Supports case-insensitive search with context."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "The path to the file to read"
                },
                "search": {
                    "type": "string",
                    "description": "Case-insensitive string to search for. Returns only matching lines with surrounding context."
                },
                "offset": {
                    "type": "integer",
                    "description": "1-indexed starting line for normal reads. Defaults to 1."
                },
                "limit": {
                    "type": "integer",
                    "description": "Maximum number of lines to return for normal reads. Defaults to 2000."
                },
                "line_numbers": {
                    "type": "boolean",
                    "description": "Whether to include line numbers. Defaults to true unless raw=true."
                },
                "raw": {
                    "type": "boolean",
                    "description": "Return exact file content without headers, footers, or line numbers. Use this before exact string replacement or copying code."
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

        let file_path = resolve_path(path)?;

        if !file_path.exists() {
            return Err(format!("File not found: {}", path));
        }

        let content = fs::read_to_string(&file_path)
            .map_err(|e| format!("Failed to read file '{}': {}", path, e))?;

        let lines: Vec<&str> = content.lines().collect();
        let total_lines = lines.len();

        // Handle search mode
        if let Some(search_term) = args.get("search").and_then(|s| s.as_str()) {
            let context: usize = 6;

            let search_lower = search_term.to_lowercase();
            let mut matches: Vec<(usize, &str)> = Vec::new();

            // Find all matching lines
            for (idx, line) in lines.iter().enumerate() {
                if line.to_lowercase().contains(&search_lower) {
                    matches.push((idx, line));
                }
            }

            if matches.is_empty() {
                return Ok(format!(
                    "No matches found for '{}' in {}",
                    search_term, path
                ));
            }

            // Build output with context
            let mut result = format!(
                "Found {} match(es) for '{}' in {}:\n\n",
                matches.len(),
                search_term,
                path
            );
            let mut shown_lines: std::collections::HashSet<usize> =
                std::collections::HashSet::new();

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

        let raw = args.get("raw").and_then(|v| v.as_bool()).unwrap_or(false);
        let line_numbers = args
            .get("line_numbers")
            .and_then(|v| v.as_bool())
            .unwrap_or(!raw);
        let offset = args
            .get("offset")
            .and_then(|n| n.as_u64())
            .unwrap_or(1)
            .max(1) as usize;
        let limit = args
            .get("limit")
            .and_then(|n| n.as_u64())
            .unwrap_or(2000)
            .max(1) as usize;

        if offset > total_lines.max(1) {
            return Err(format!(
                "offset {} exceeds file length ({} lines).",
                offset, total_lines
            ));
        }

        let start_idx = offset - 1;
        let end_idx = (start_idx + limit).min(total_lines);
        let selected = &lines[start_idx..end_idx];

        if raw {
            if offset == 1 && end_idx == total_lines {
                return Ok(content);
            }

            let mut raw_chunk = String::new();
            let mut current_line = 1;
            for segment in content.split_inclusive('\n') {
                if current_line >= offset && current_line <= end_idx {
                    raw_chunk.push_str(segment);
                }
                current_line += 1;
            }
            if current_line <= end_idx && !content.ends_with('\n') {
                if let Some(segment) = content.rsplit('\n').next() {
                    raw_chunk.push_str(segment);
                }
            }
            return Ok(raw_chunk);
        }

        let mut result = String::new();
        let shown_start = if total_lines == 0 { 0 } else { offset };
        result.push_str(&format!(
            "File: {}\nShowing lines {}-{} of {}. Line numbers are not part of the file.\n\n",
            file_path.display(),
            shown_start,
            end_idx,
            total_lines
        ));

        for (idx, line) in selected.iter().enumerate() {
            let line_num = start_idx + idx + 1;
            if line_numbers {
                result.push_str(&format!("{:>4} | {}\n", line_num, line));
            } else {
                result.push_str(line);
                result.push('\n');
            }
        }

        if end_idx < total_lines {
            result.push_str(&format!(
                "\n[{} more line(s). Continue with offset={} limit={}.]",
                total_lines - end_idx,
                end_idx + 1,
                limit
            ));
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

        let file_path = resolve_path(path)?;

        // Create parent directories if they don't exist
        if let Some(parent) = file_path.parent() {
            if !parent.exists() {
                fs::create_dir_all(parent)
                    .map_err(|e| format!("Failed to create directories: {}", e))?;
            }
        }

        fs::write(&file_path, content)
            .map_err(|e| format!("Failed to write file '{}': {}", path, e))?;

        Ok(format!(
            "Successfully wrote {} bytes to '{}'",
            content.len(),
            file_path.display()
        ))
    }
}

pub struct ApplyPatchTool;

impl Tool for ApplyPatchTool {
    fn name(&self) -> &'static str {
        "apply_patch"
    }

    fn description(&self) -> &'static str {
        "Apply a simple multi-file patch after validating every operation. Supports Add File, Delete File, and Update File sections with contextual @@ hunks. Use dry_run=true to verify without changing files."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "patch": {
                    "type": "string",
                    "description": "Patch text. Format: *** Begin Patch, then file sections like *** Add File: path, *** Delete File: path, or *** Update File: path with @@ hunks using space/context, -removed, +added lines, ending with *** End Patch."
                },
                "dry_run": {
                    "type": "boolean",
                    "description": "If true, validate and summarize the patch without writing files. Defaults to false."
                }
            },
            "required": ["patch"]
        })
    }

    fn execute(&self, args: &Value) -> Result<String, String> {
        let patch = args
            .get("patch")
            .and_then(|p| p.as_str())
            .ok_or("Missing 'patch' argument")?;
        let dry_run = args
            .get("dry_run")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let parsed = parse_patch(patch)?;
        apply_parsed_patch(parsed, dry_run)
    }
}

pub struct FileInfoTool;

impl Tool for FileInfoTool {
    fn name(&self) -> &'static str {
        "file_info"
    }

    fn description(&self) -> &'static str {
        "Return basic metadata for a path so the agent can choose a safe read/edit strategy. Reports existence, kind, size, line count for text files, binary detection, and modified time."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Path to inspect."
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
        let file_path = resolve_path(path)?;

        if !file_path.exists() {
            return Ok(format!("Path: {}\nExists: false\n", file_path.display()));
        }

        let metadata = fs::symlink_metadata(&file_path)
            .map_err(|e| format!("Failed to stat '{}': {}", file_path.display(), e))?;
        let file_type = metadata.file_type();
        let kind = if file_type.is_symlink() {
            "symlink"
        } else if metadata.is_dir() {
            "directory"
        } else if metadata.is_file() {
            "file"
        } else {
            "other"
        };

        let modified = metadata
            .modified()
            .ok()
            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
            .map(|duration| duration.as_secs().to_string())
            .unwrap_or_else(|| "unknown".to_string());

        let mut result = format!(
            "Path: {}\nExists: true\nKind: {}\nSize bytes: {}\nModified unix seconds: {}\n",
            file_path.display(),
            kind,
            metadata.len(),
            modified
        );

        if metadata.is_file() {
            let sample = fs::read(&file_path)
                .map_err(|e| format!("Failed to read '{}': {}", file_path.display(), e))?;
            let binary = is_probably_binary(&sample[..sample.len().min(8192)]);
            result.push_str(&format!("Probably binary: {}\n", binary));
            if !binary {
                let text = String::from_utf8_lossy(&sample);
                result.push_str(&format!("Line count: {}\n", text.lines().count()));
            }
        }

        Ok(result)
    }
}

pub struct WebSearchTool;

impl Tool for WebSearchTool {
    fn name(&self) -> &'static str {
        "web_search"
    }

    fn description(&self) -> &'static str {
        "Search the web using DuckDuckGo. Returns a list of relevant URLs with titles and snippets. Use this to find sources, but DO NOT STOP THERE. Use web_read_page to then delve into content you find relevant."
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
        let query = args
            .get("query")
            .and_then(|q| q.as_str())
            .ok_or("Missing 'query' argument")?;

        // 1. Fetch the Search Results
        let encoded_query = urlencoding::encode(query);
        let url = format!("https://lite.duckduckgo.com/lite/?q={}", encoded_query);

        let response = ureq::get(&url)
            .set("User-Agent", "Mozilla/5.0 (compatible; ask-rs/1.0)")
            .call()
            .map_err(|e| format!("Search request failed: {}", e))?;

        let body = response
            .into_string()
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
        Ok(serde_json::to_string_pretty(&formatted_results).map_err(|e| e.to_string())?)
    }
}

pub struct WebPageReaderTool;

impl Tool for WebPageReaderTool {
    fn name(&self) -> &'static str {
        "web_read_page"
    }

    fn description(&self) -> &'static str {
        "Reads the main content of a webpage, extracting clean readable text. \
         Use this after web_search to read a result. Prefers blog posts, docs, and forum threads — \
         JavaScript-heavy pages (GitHub, SPAs) may return little or nothing. \
         If this tool returns a WARNING about no structured content and you absolutely need \
         the raw HTML of that page, use web_fetch_raw instead."
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
        let url = args
            .get("url")
            .and_then(|u| u.as_str())
            .ok_or("Missing 'url' argument")?;

        // 1. Fetch the Page
        let response = ureq::get(url)
            .set("User-Agent", "Mozilla/5.0 (compatible; ask-rs/1.0)")
            .call()
            .map_err(|e| format!("Failed to fetch page: {}", e))?;

        let mut body = response
            .into_string()
            .map_err(|e| format!("Failed to read page body: {}", e))?;

        // Limit the payload size to avoid scraper hanging on massive documents
        let max_body_len = 200_000;
        if body.len() > max_body_len {
            body.truncate(max_body_len);
        }

        // 2. DOM leaf-content extraction
        //
        // Strategy: select semantic content-atom tags (p, li, h1-h6, etc.) and keep
        // only those with no block-level children (true leaf nodes) and enough text.
        // This works regardless of accessibility markup — Reddit comments, forum posts,
        // docs, blogs all expose their content through these tags deep in the tree.
        //
        // Attributes are discarded entirely (we only call .text()), so no HTML noise
        // reaches the model. script/style are excluded by tag name.
        let document = scraper::Html::parse_document(&body);

        // Tags that carry visible content and are naturally leaf-level in good HTML
        let sel_atoms = scraper::Selector::parse(
            "p, li, td, th, dt, dd, h1, h2, h3, h4, h5, h6, blockquote, pre, figcaption, caption, span"
        ).unwrap();

        // Block-level tags — if an atom contains one of these it isn't really a leaf
        const BLOCK_CHILDREN: &[&str] = &[
            "p",
            "div",
            "section",
            "article",
            "main",
            "aside",
            "header",
            "footer",
            "nav",
            "ul",
            "ol",
            "dl",
            "table",
            "form",
            "blockquote",
            "figure",
            "h1",
            "h2",
            "h3",
            "h4",
            "h5",
            "h6",
            "pre",
            "li",
            "tr",
        ];

        // Ancestors that mean "this is UI chrome, not content"
        const SKIP_ANCESTORS: &[&str] =
            &["script", "style", "noscript", "template", "svg", "iframe"];

        let blocks: Vec<String> = document
            .select(&sel_atoms)
            // drop anything nested inside non-content ancestors
            .filter(|el| {
                !el.ancestors()
                    .filter_map(scraper::ElementRef::wrap)
                    .any(|a| SKIP_ANCESTORS.contains(&a.value().name()))
            })
            // keep only leaf-ish nodes: no block-level element children
            .filter(|el| {
                !el.children()
                    .filter_map(scraper::ElementRef::wrap)
                    .any(|child| BLOCK_CHILDREN.contains(&child.value().name()))
            })
            // collapse whitespace
            .map(|el| el.text().collect::<Vec<_>>().join(" "))
            .map(|s| s.split_whitespace().collect::<Vec<_>>().join(" "))
            // drop short fragments (nav links, button labels, etc.)
            .filter(|s| s.len() >= 80)
            .collect();

        if blocks.is_empty() {
            // Fall back to raw text dump so the agent can still recover if this
            // page is the only viable source. Warn clearly so it knows what it's getting.
            let raw = document
                .root_element()
                .text()
                .map(|t| t.trim())
                .filter(|t| !t.is_empty())
                .collect::<Vec<_>>()
                .join(" ");

            let raw_capped = if raw.len() > 5_000 {
                &raw[..5_000]
            } else {
                &raw
            };

            return Ok(format!(
                "WARNING: No structured content could be extracted from this page \
                 (it likely requires JavaScript to render). \
                 A raw text dump follows — it will be noisy and may include menus, \
                 scripts, and boilerplate. Only use this if the information here is \
                 critical and unavailable elsewhere; otherwise, try a blog post, \
                 documentation page, or forum thread from your search results instead.\n\n\
                 --- RAW DUMP ---\n{raw_capped}"
            ));
        }

        let result_text = blocks.join("\n\n");

        let max_len = 10_000;
        if result_text.len() > max_len {
            Ok(format!(
                "{}...\n\n(Content truncated at 10k chars — if you need more, search for a more specific page.)",
                &result_text[..max_len]
            ))
        } else {
            Ok(result_text)
        }
    }
}

pub struct WebFetchRawTool;

impl Tool for WebFetchRawTool {
    fn name(&self) -> &'static str {
        "web_fetch_raw"
    }

    fn description(&self) -> &'static str {
        "Fetches the raw HTTP response body of a URL as plain text (GET only). \
         Use this as a last resort when web_read_page returns a WARNING about no structured content \
         and the page is the only viable source of critical information. \
         The output will be noisy HTML — scan it carefully for what you need. \
         Do NOT use this as a first step; always try web_read_page first."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "url": {
                    "type": "string",
                    "description": "The URL to fetch (must be http:// or https://)"
                }
            },
            "required": ["url"]
        })
    }

    fn execute(&self, args: &Value) -> Result<String, String> {
        let url = args
            .get("url")
            .and_then(|u| u.as_str())
            .ok_or("Missing 'url' argument")?;

        if !url.starts_with("http://") && !url.starts_with("https://") {
            return Err("Only http:// and https:// URLs are allowed.".to_string());
        }

        let mut body = ureq::get(url)
            .set("User-Agent", "Mozilla/5.0 (compatible; ask-rs/1.0)")
            .call()
            .map_err(|e| format!("Fetch failed: {}", e))?
            .into_string()
            .map_err(|e| format!("Failed to read response: {}", e))?;

        let max_len = 15_000;
        if body.len() > max_len {
            body.truncate(max_len);
            body.push_str("\n\n(Truncated at 15k chars)");
        }

        Ok(body)
    }
}

pub struct SearchFilesTool;

impl Tool for SearchFilesTool {
    fn name(&self) -> &'static str {
        "search_files"
    }

    fn description(&self) -> &'static str {
        "Recursively find files by name pattern. Resolves relative paths from the current working directory and skips noisy folders like .git, target, node_modules, and __pycache__."
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
                    "description": "Simple glob-like filename pattern (e.g. '*.rs', 'main', '*test*'). Defaults to all files."
                },
                "max_results": {
                    "type": "integer",
                    "description": "Maximum number of files to return. Defaults to 200."
                }
            },
            "required": []
        })
    }

    fn execute(&self, args: &Value) -> Result<String, String> {
        let path = args.get("path").and_then(|p| p.as_str()).unwrap_or(".");

        let pattern = args.get("pattern").and_then(|p| p.as_str());

        let max_results = args
            .get("max_results")
            .and_then(|n| n.as_u64())
            .unwrap_or(200)
            .max(1) as usize;

        let root = resolve_path(path)?;
        if !root.is_dir() {
            return Err(format!(
                "Search path is not a directory: {}",
                root.display()
            ));
        }

        let (files, truncated) = collect_files(&root, pattern, max_results)?;
        if files.is_empty() {
            Ok("No files found matching the criteria.".to_string())
        } else {
            let mut result = format!(
                "Found {} file(s) under {}{}:\n",
                files.len(),
                root.display(),
                if truncated { " (truncated)" } else { "" }
            );
            for file in files {
                result.push_str(&format!("{}\n", file.display()));
            }
            if truncated {
                result.push_str("\n[Results truncated. Use a narrower path or pattern.]\n");
            }
            Ok(result)
        }
    }
}

pub struct SearchTextTool;

impl Tool for SearchTextTool {
    fn name(&self) -> &'static str {
        "search_text"
    }

    fn description(&self) -> &'static str {
        "Search file contents using ripgrep. Returns matching file paths with line numbers. Use this for code/symbol exploration before reading specific files."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "Regex pattern to search for."
                },
                "path": {
                    "type": "string",
                    "description": "File or directory to search. Defaults to current directory."
                },
                "include": {
                    "type": "string",
                    "description": "Optional file glob passed to ripgrep, e.g. '*.rs' or '*.{ts,tsx}'."
                },
                "case_sensitive": {
                    "type": "boolean",
                    "description": "If true, force case-sensitive search. Defaults to ripgrep smart-case."
                },
                "max_bytes": {
                    "type": "integer",
                    "description": "Maximum bytes returned. Defaults to 20000."
                }
            },
            "required": ["pattern"]
        })
    }

    fn execute(&self, args: &Value) -> Result<String, String> {
        let pattern = args
            .get("pattern")
            .and_then(|p| p.as_str())
            .ok_or("Missing 'pattern' argument")?;
        let path = args.get("path").and_then(|p| p.as_str()).unwrap_or(".");
        let root = resolve_path(path)?;
        let max_bytes = args
            .get("max_bytes")
            .and_then(|n| n.as_u64())
            .unwrap_or(20_000)
            .max(1) as usize;

        let mut command = Command::new("rg");
        command
            .arg("--line-number")
            .arg("--no-heading")
            .arg("--color")
            .arg("never")
            .arg("--glob")
            .arg("!.git/**")
            .arg("--glob")
            .arg("!target/**")
            .arg("--glob")
            .arg("!node_modules/**");

        if !args
            .get("case_sensitive")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
        {
            command.arg("--smart-case");
        }

        if let Some(include) = args.get("include").and_then(|v| v.as_str()) {
            command.arg("--glob").arg(include);
        }

        command.arg(pattern).arg(&root);

        let output = command
            .output()
            .map_err(|e| format!("Failed to execute rg. Is ripgrep installed? {}", e))?;

        if output.status.code() == Some(1) {
            return Ok(format!(
                "No matches found for '{}' under {}.",
                pattern,
                root.display()
            ));
        }
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(format!("Search failed: {}", stderr));
        }

        let result = String::from_utf8_lossy(&output.stdout).to_string();
        Ok(truncate_output(result, max_bytes))
    }
}

pub struct ListDirTool;

impl Tool for ListDirTool {
    fn name(&self) -> &'static str {
        "list_dir"
    }

    fn description(&self) -> &'static str {
        "List directory entries for cheap workspace orientation. Directories end with '/'. Skips noisy folders by default."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Directory to list. Defaults to current directory."
                },
                "recursive": {
                    "type": "boolean",
                    "description": "Whether to list recursively. Defaults to false."
                },
                "max_depth": {
                    "type": "integer",
                    "description": "Maximum recursion depth when recursive=true. Defaults to 2."
                },
                "max_entries": {
                    "type": "integer",
                    "description": "Maximum entries returned. Defaults to 200."
                }
            },
            "required": []
        })
    }

    fn execute(&self, args: &Value) -> Result<String, String> {
        let path = args.get("path").and_then(|p| p.as_str()).unwrap_or(".");
        let recursive = args
            .get("recursive")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let max_depth = args.get("max_depth").and_then(|n| n.as_u64()).unwrap_or(2) as usize;
        let max_entries = args
            .get("max_entries")
            .and_then(|n| n.as_u64())
            .unwrap_or(200)
            .max(1) as usize;

        let root = resolve_path(path)?;
        if !root.is_dir() {
            return Err(format!("Path is not a directory: {}", root.display()));
        }

        let mut result = format!("Directory: {}\n", root.display());
        let mut stack = vec![(root.clone(), 0usize)];
        let mut shown = 0usize;
        let mut truncated = false;

        while let Some((dir, depth)) = stack.pop() {
            let mut entries: Vec<PathBuf> = fs::read_dir(&dir)
                .map_err(|e| format!("Failed to read directory '{}': {}", dir.display(), e))?
                .filter_map(|entry| entry.ok().map(|entry| entry.path()))
                .filter(|path| !should_skip_path(path))
                .collect();
            entries.sort();

            for entry in entries.into_iter().rev() {
                if shown >= max_entries {
                    truncated = true;
                    break;
                }

                let rel = entry.strip_prefix(&root).unwrap_or(&entry);
                let suffix = if entry.is_dir() { "/" } else { "" };
                result.push_str(&format!("{}{}\n", rel.display(), suffix));
                shown += 1;

                if recursive && entry.is_dir() && depth < max_depth {
                    stack.push((entry, depth + 1));
                }
            }

            if truncated {
                break;
            }
        }

        if shown == 0 {
            result.push_str("(empty)\n");
        }
        if truncated {
            result.push_str(
                "\n[Directory listing truncated. Use a narrower path or higher max_entries.]\n",
            );
        }
        Ok(result)
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
                },
                "expected_replacements": {
                    "type": "integer",
                    "description": "Optional safety check for search-and-replace mode. The edit fails unless the number of replacements equals this value."
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

        let file_path = resolve_path(path)?;

        if !file_path.exists() {
            return Err(format!("File not found: {}", path));
        }

        let content = fs::read_to_string(&file_path)
            .map_err(|e| format!("Failed to read file '{}': {}", path, e))?;

        let old_string = args.get("old_string").and_then(|s| s.as_str());
        let start_line = args
            .get("start_line")
            .and_then(|n| n.as_u64())
            .map(|n| n as usize);
        let end_line = args
            .get("end_line")
            .and_then(|n| n.as_u64())
            .map(|n| n as usize);

        let new_content = if let Some(old_str) = old_string {
            // Search-and-replace mode
            if old_str == new_string {
                return Err(
                    "old_string and new_string are identical — no change needed.".to_string(),
                );
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

            let count = content.matches(old_str).count();
            let replacements = if replace_all { count } else { 1 };
            if let Some(expected) = args.get("expected_replacements").and_then(|n| n.as_u64()) {
                if replacements as u64 != expected {
                    return Err(format!(
                        "Replacement count safety check failed for '{}': expected {}, would replace {}.",
                        path, expected, replacements
                    ));
                }
            }

            if replace_all {
                let result = content.replace(old_str, new_string);
                fs::write(&file_path, &result)
                    .map_err(|e| format!("Failed to write file '{}': {}", path, e))?;
                return Ok(format!(
                    "Replaced all {} occurrence(s). {}",
                    count,
                    summarize_content_change(&file_path, &content, &result)
                ));
            } else {
                let result = content.replacen(old_str, new_string, 1);
                fs::write(&file_path, &result)
                    .map_err(|e| format!("Failed to write file '{}': {}", path, e))?;
                if count > 1 {
                    return Ok(format!(
                        "Replaced first occurrence. {} total occurrences existed; use replace_all=true if needed. {}",
                        count,
                        summarize_content_change(&file_path, &content, &result)
                    ));
                }
                return Ok(format!(
                    "Replaced the specified text. {}",
                    summarize_content_change(&file_path, &content, &result)
                ));
            }
        } else if let (Some(start), Some(end)) = (start_line, end_line) {
            // Line-range mode
            let lines: Vec<&str> = content.lines().collect();
            let total_lines = lines.len();

            if start == 0 || end == 0 {
                return Err(
                    "Line numbers are 1-indexed. Use start_line >= 1 and end_line >= 1."
                        .to_string(),
                );
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

        fs::write(&file_path, &new_content)
            .map_err(|e| format!("Failed to write file '{}': {}", path, e))?;

        Ok(format!(
            "Edited line range. {}",
            summarize_content_change(&file_path, &content, &new_content)
        ))
    }
}

pub struct ExecuteCommandTool;

impl Tool for ExecuteCommandTool {
    fn name(&self) -> &'static str {
        "run_shell_command"
    }

    fn description(&self) -> &'static str {
        "Execute a shell command on the system. Supports explicit cwd and timeout. Requires user approval for each execution unless always-allow was selected."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "The shell command to execute"
                },
                "cwd": {
                    "type": "string",
                    "description": "Working directory for the command. Defaults to the current process directory."
                },
                "timeout_ms": {
                    "type": "integer",
                    "description": "Timeout in milliseconds. Defaults to 120000."
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
        let cwd = args
            .get("cwd")
            .and_then(|c| c.as_str())
            .map(resolve_path)
            .transpose()?;
        let timeout_ms = args
            .get("timeout_ms")
            .and_then(|n| n.as_u64())
            .unwrap_or(120_000)
            .max(1);

        if let Some(cwd) = &cwd {
            if !cwd.is_dir() {
                return Err(format!("cwd is not a directory: {}", cwd.display()));
            }
        }

        // Skip prompt if user previously chose "always allow"
        if !AUTO_APPROVE_COMMANDS.load(Ordering::Relaxed) {
            // Request user approval
            println!("\n\x1b[33m> The agent wants to execute the following command:\x1b[0m");
            println!("\x1b[36m{}\x1b[0m", command_str);
            if let Some(cwd) = &cwd {
                println!("\x1b[33m> cwd:\x1b[0m {}", cwd.display());
            }
            print!("\x1b[33m> Do you approve this execution? [y/N/a]: \x1b[0m");
            io::stdout()
                .flush()
                .map_err(|e| format!("Failed to flush stdout: {}", e))?;

            let mut input = String::new();
            io::stdin()
                .read_line(&mut input)
                .map_err(|e| format!("Failed to read input: {}", e))?;

            match input.trim().to_lowercase().as_str() {
                "a" => AUTO_APPROVE_COMMANDS.store(true, Ordering::Relaxed),
                "y" => {}
                _ => return Err("User denied command execution.".to_string()),
            }
        }

        let mut command = Command::new("sh");
        command
            .arg("-lc")
            .arg(command_str)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(cwd) = &cwd {
            command.current_dir(cwd);
        }

        let mut child = command
            .spawn()
            .map_err(|e| format!("Failed to execute command: {}", e))?;

        let mut stdout = child
            .stdout
            .take()
            .ok_or("Failed to capture command stdout")?;
        let mut stderr = child
            .stderr
            .take()
            .ok_or("Failed to capture command stderr")?;
        let stdout_handle = std::thread::spawn(move || {
            let mut bytes = Vec::new();
            let _ = stdout.read_to_end(&mut bytes);
            bytes
        });
        let stderr_handle = std::thread::spawn(move || {
            let mut bytes = Vec::new();
            let _ = stderr.read_to_end(&mut bytes);
            bytes
        });

        let start = Instant::now();
        let timeout = Duration::from_millis(timeout_ms);
        let status;
        loop {
            if let Some(exit_status) = child
                .try_wait()
                .map_err(|e| format!("Failed to poll command: {}", e))?
            {
                status = exit_status;
                break;
            }
            if start.elapsed() >= timeout {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("Command timed out after {} ms.", timeout_ms));
            }
            std::thread::sleep(Duration::from_millis(25));
        }

        let stdout_bytes = stdout_handle
            .join()
            .map_err(|_| "Failed to join stdout reader thread".to_string())?;
        let stderr_bytes = stderr_handle
            .join()
            .map_err(|_| "Failed to join stderr reader thread".to_string())?;
        let stdout = String::from_utf8_lossy(&stdout_bytes);
        let stderr = String::from_utf8_lossy(&stderr_bytes);

        let cwd_display = cwd
            .as_ref()
            .map(|cwd| cwd.display().to_string())
            .unwrap_or_else(|| {
                std::env::current_dir()
                    .map(|cwd| cwd.display().to_string())
                    .unwrap_or_else(|_| "<unknown>".to_string())
            });
        let mut result = format!(
            "Exit status: {}\nWorking directory: {}\n",
            status, cwd_display
        );
        if !stdout.is_empty() {
            result.push_str(&format!("Stdout:\n{}\n", stdout));
        }
        if !stderr.is_empty() {
            result.push_str(&format!("Stderr:\n{}\n", stderr));
        }

        if stdout.is_empty() && stderr.is_empty() {
            result.push_str("(Command executed with no stdout/stderr)\n");
        }

        if !status.success() {
            result.push_str(&format!("\nCommand failed with exit code: {}", status));
        }

        Ok(result)
    }
}
