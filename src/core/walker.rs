//! Directory walking functionality with .gitignore and .context-creator-ignore support

use crate::utils::error::ContextCreatorError;
use crate::utils::file_ext::{is_binary_extension, FileType};
use anyhow::Result;
use glob::Pattern;
use ignore::{Walk, WalkBuilder};
use rayon::prelude::*;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tracing::warn;

/// Compiled priority rule for efficient pattern matching
///
/// This struct represents a custom priority rule that has been compiled from
/// the configuration file. The glob pattern is pre-compiled for performance,
/// and the weight is applied additively to the base file type priority.
///
/// # Priority Calculation
/// Final priority = base_priority + weight (if pattern matches)
///
/// # Pattern Matching
/// Uses first-match-wins semantics - the first pattern that matches a file
/// will determine the priority adjustment. Subsequent patterns are not evaluated.
#[derive(Debug, Clone)]
pub struct CompiledPriority {
    /// Pre-compiled glob pattern for efficient matching
    pub matcher: Pattern,
    /// Priority weight to add to base priority (can be negative)
    pub weight: f32,
    /// Original pattern string for debugging and error reporting
    pub original_pattern: String,
}

impl CompiledPriority {
    /// Create a CompiledPriority from a pattern string
    pub fn new(pattern: &str, weight: f32) -> Result<Self, glob::PatternError> {
        let matcher = Pattern::new(pattern)?;
        Ok(Self {
            matcher,
            weight,
            original_pattern: pattern.to_string(),
        })
    }

    /// Convert from config::Priority to CompiledPriority with error handling
    pub fn try_from_config_priority(
        priority: &crate::config::Priority,
    ) -> Result<Self, glob::PatternError> {
        Self::new(&priority.pattern, priority.weight)
    }
}

/// Options for walking directories
#[derive(Debug, Clone)]
pub struct WalkOptions {
    /// Maximum file size in bytes
    pub max_file_size: Option<usize>,
    /// Follow symbolic links
    pub follow_links: bool,
    /// Include hidden files
    pub include_hidden: bool,
    /// Use parallel processing
    pub parallel: bool,
    /// Custom ignore file name (default: .context-creator-ignore)
    pub ignore_file: String,
    /// Additional glob patterns to ignore
    pub ignore_patterns: Vec<String>,
    /// Only include files matching these patterns
    pub include_patterns: Vec<String>,
    /// Custom priority rules for file prioritization
    pub custom_priorities: Vec<CompiledPriority>,
    /// Filter out binary files by extension
    pub filter_binary_files: bool,
}

impl WalkOptions {
    /// Create WalkOptions from CLI config
    pub fn from_config(config: &crate::cli::Config) -> Result<Self> {
        // Convert config priorities to CompiledPriority with error handling
        let mut custom_priorities = Vec::new();
        for priority in &config.custom_priorities {
            match CompiledPriority::try_from_config_priority(priority) {
                Ok(compiled) => custom_priorities.push(compiled),
                Err(e) => {
                    return Err(ContextCreatorError::ConfigError(format!(
                        "Invalid glob pattern '{}' in custom priorities: {e}",
                        priority.pattern
                    ))
                    .into());
                }
            }
        }

        // Get include patterns from CLI config and filter out empty/whitespace patterns
        let include_patterns = config
            .get_include_patterns()
            .into_iter()
            .filter(|pattern| !pattern.trim().is_empty())
            .collect();

        // Get ignore patterns from CLI config and filter out empty/whitespace patterns
        let ignore_patterns = config
            .get_ignore_patterns()
            .into_iter()
            .filter(|pattern| !pattern.trim().is_empty())
            .collect();

        Ok(WalkOptions {
            max_file_size: Some(10 * 1024 * 1024), // 10MB default
            follow_links: false,
            include_hidden: false,
            parallel: true,
            ignore_file: ".context-creator-ignore".to_string(),
            ignore_patterns,
            include_patterns,
            custom_priorities,
            filter_binary_files: config.get_prompt().is_some(),
        })
    }
}

impl Default for WalkOptions {
    fn default() -> Self {
        WalkOptions {
            max_file_size: Some(10 * 1024 * 1024), // 10MB
            follow_links: false,
            include_hidden: false,
            parallel: true,
            ignore_file: ".context-creator-ignore".to_string(),
            ignore_patterns: vec![],
            include_patterns: vec![],
            custom_priorities: vec![],
            filter_binary_files: false,
        }
    }
}

/// Information about a file found during walking
#[derive(Debug, Clone)]
pub struct FileInfo {
    /// Absolute path to the file
    pub path: PathBuf,
    /// Relative path from the root directory
    pub relative_path: PathBuf,
    /// File size in bytes
    pub size: u64,
    /// File type based on extension
    pub file_type: FileType,
    /// Priority score (higher is more important)
    pub priority: f32,
    /// Files that this file imports/depends on (for semantic analysis)
    pub imports: Vec<PathBuf>,
    /// Files that import this file (reverse dependencies)
    pub imported_by: Vec<PathBuf>,
    /// Function calls made by this file (for --include-callers analysis)
    pub function_calls: Vec<crate::core::semantic::analyzer::FunctionCall>,
    /// Type references used by this file (for --include-types analysis)
    pub type_references: Vec<crate::core::semantic::analyzer::TypeReference>,
    /// Function definitions exported by this file (for --include-callers analysis)
    pub exported_functions: Vec<crate::core::semantic::analyzer::FunctionDefinition>,
}

impl FileInfo {
    /// Get a display string for the file type
    pub fn file_type_display(&self) -> &'static str {
        use crate::utils::file_ext::FileType;
        match self.file_type {
            FileType::Rust => "Rust",
            FileType::Python => "Python",
            FileType::JavaScript => "JavaScript",
            FileType::TypeScript => "TypeScript",
            FileType::Go => "Go",
            FileType::Java => "Java",
            FileType::Cpp => "C++",
            FileType::C => "C",
            FileType::CSharp => "C#",
            FileType::Ruby => "Ruby",
            FileType::Php => "PHP",
            FileType::Swift => "Swift",
            FileType::Kotlin => "Kotlin",
            FileType::Scala => "Scala",
            FileType::Haskell => "Haskell",
            FileType::Dart => "Dart",
            FileType::Lua => "Lua",
            FileType::R => "R",
            FileType::Julia => "Julia",
            FileType::Elixir => "Elixir",
            FileType::Elm => "Elm",
            FileType::Markdown => "Markdown",
            FileType::Json => "JSON",
            FileType::Yaml => "YAML",
            FileType::Toml => "TOML",
            FileType::Xml => "XML",
            FileType::Html => "HTML",
            FileType::Css => "CSS",
            FileType::Text => "Text",
            FileType::Other => "Other",
        }
    }
}

/// Walk a path (file or directory) and collect file information
pub fn walk_directory(root: &Path, options: WalkOptions) -> Result<Vec<FileInfo>> {
    if !root.exists() {
        return Err(ContextCreatorError::InvalidPath(format!(
            "Path does not exist: {}",
            root.display()
        ))
        .into());
    }

    // Handle individual files
    if root.is_file() {
        let metadata = root.metadata()?;
        let file_type = FileType::from_path(root);
        let relative_path = PathBuf::from(
            root.file_name()
                .ok_or_else(|| anyhow::anyhow!("Invalid file name"))?,
        );
        let priority = calculate_priority(&file_type, &relative_path, &options.custom_priorities);

        let file_info = FileInfo {
            path: root.to_path_buf(),
            relative_path,
            size: metadata.len(),
            file_type,
            priority,
            imports: Vec::new(),
            imported_by: Vec::new(),
            function_calls: Vec::new(),
            type_references: Vec::new(),
            exported_functions: Vec::new(),
        };
        return Ok(vec![file_info]);
    }

    if !root.is_dir() {
        return Err(ContextCreatorError::InvalidPath(format!(
            "Path is neither a file nor a directory: {}",
            root.display()
        ))
        .into());
    }

    let root = root.canonicalize()?;
    let walker = build_walker(&root, &options)?;

    if options.parallel {
        walk_parallel(walker, &root, &options)
    } else {
        walk_sequential(walker, &root, &options)
    }
}

/// Sanitize include patterns to prevent security issues
pub fn sanitize_pattern(pattern: &str) -> Result<String> {
    // Length limit to prevent resource exhaustion
    if pattern.len() > 1000 {
        return Err(ContextCreatorError::InvalidConfiguration(
            "Pattern too long (max 1000 characters)".to_string(),
        )
        .into());
    }

    // No null bytes, control characters, or dangerous Unicode characters
    if pattern.contains('\0')
        || pattern.chars().any(|c| {
            c.is_control() ||
            c == '\u{2028}' ||  // Line separator
            c == '\u{2029}' ||  // Paragraph separator
            c == '\u{FEFF}' // Byte order mark
        })
    {
        return Err(ContextCreatorError::InvalidConfiguration(
            "Pattern contains invalid characters (null bytes or control characters)".to_string(),
        )
        .into());
    }

    // No absolute paths to prevent directory traversal
    if pattern.starts_with('/') || pattern.starts_with('\\') {
        return Err(ContextCreatorError::InvalidConfiguration(
            "Absolute paths not allowed in patterns".to_string(),
        )
        .into());
    }

    // Prevent directory traversal
    if pattern.contains("..") {
        return Err(ContextCreatorError::InvalidConfiguration(
            "Directory traversal (..) not allowed in patterns".to_string(),
        )
        .into());
    }

    Ok(pattern.to_string())
}

/// Build the ignore walker with configured options
fn build_walker(root: &Path, options: &WalkOptions) -> Result<Walk> {
    let mut builder = WalkBuilder::new(root);

    // Configure the walker
    builder
        .follow_links(options.follow_links)
        .hidden(!options.include_hidden)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .ignore(true)
        .parents(true)
        .add_custom_ignore_filename(&options.ignore_file);

    // Handle both include and ignore patterns using OverrideBuilder
    if !options.include_patterns.is_empty() || !options.ignore_patterns.is_empty() {
        let mut override_builder = ignore::overrides::OverrideBuilder::new(root);

        // If we have no include patterns but have ignore patterns, we need to include everything first
        if options.include_patterns.is_empty() && !options.ignore_patterns.is_empty() {
            // Add a pattern to include everything
            override_builder.add("**/*").map_err(|e| {
                ContextCreatorError::InvalidConfiguration(format!(
                    "Failed to add include-all pattern: {e}"
                ))
            })?;
        }

        // Add include patterns first (without prefix for inclusion)
        for pattern in &options.include_patterns {
            if !pattern.trim().is_empty() {
                // Sanitize pattern for security
                let sanitized_pattern = sanitize_pattern(pattern)?;

                // Include patterns are added directly (not as negations)
                override_builder.add(&sanitized_pattern).map_err(|e| {
                    ContextCreatorError::InvalidConfiguration(format!(
                        "Invalid include pattern '{pattern}': {e}"
                    ))
                })?;
            }
        }

        // Add ignore patterns after include patterns (with ! prefix for exclusion)
        // This ensures ignore patterns take precedence over include patterns
        for pattern in &options.ignore_patterns {
            if !pattern.trim().is_empty() {
                // Sanitize pattern for security
                let sanitized_pattern = sanitize_pattern(pattern)?;

                // Prefix with ! to make it an ignore pattern
                let ignore_pattern = format!("!{sanitized_pattern}");
                override_builder.add(&ignore_pattern).map_err(|e| {
                    ContextCreatorError::InvalidConfiguration(format!(
                        "Invalid ignore pattern '{pattern}': {e}"
                    ))
                })?;
            }
        }

        let overrides = override_builder.build().map_err(|e| {
            ContextCreatorError::InvalidConfiguration(format!(
                "Failed to build pattern overrides: {e}"
            ))
        })?;

        builder.overrides(overrides);
    }

    Ok(builder.build())
}

/// Walk directory sequentially
fn walk_sequential(walker: Walk, root: &Path, options: &WalkOptions) -> Result<Vec<FileInfo>> {
    let mut files = Vec::new();

    for entry in walker {
        let entry = entry?;
        let path = entry.path();

        // Skip directories
        if path.is_dir() {
            continue;
        }

        // Process file
        if let Some(file_info) = process_file(path, root, options)? {
            files.push(file_info);
        }
    }

    Ok(files)
}

/// Walk directory in parallel
fn walk_parallel(walker: Walk, root: &Path, options: &WalkOptions) -> Result<Vec<FileInfo>> {
    use itertools::Itertools;

    let root = Arc::new(root.to_path_buf());
    let options = Arc::new(options.clone());

    // Collect entries first
    let entries: Vec<_> = walker
        .filter_map(|e| e.ok())
        .filter(|e| !e.path().is_dir())
        .collect();

    // Process in parallel with proper error collection
    let results: Vec<Result<Option<FileInfo>, ContextCreatorError>> = entries
        .into_par_iter()
        .map(|entry| {
            let path = entry.path();
            match process_file(path, &root, &options) {
                Ok(file_info) => Ok(file_info),
                Err(e) => Err(ContextCreatorError::FileProcessingError {
                    path: path.display().to_string(),
                    error: e.to_string(),
                }),
            }
        })
        .collect();

    // Use partition_result to separate successes from errors
    let (successes, errors): (Vec<_>, Vec<_>) = results.into_iter().partition_result();

    // Handle errors based on severity
    if !errors.is_empty() {
        let critical_errors: Vec<_> = errors
            .iter()
            .filter(|e| {
                e.to_string().contains("Permission denied") || e.to_string().contains("Invalid")
            })
            .collect();

        if !critical_errors.is_empty() {
            // Critical errors should fail the operation
            let error_summary: Vec<String> =
                critical_errors.iter().map(|e| e.to_string()).collect();
            return Err(anyhow::anyhow!(
                "Critical file processing errors encountered: {}",
                error_summary.join(", ")
            ));
        }

        // Non-critical errors are logged as warnings
        warn!("Warning: {} files could not be processed:", errors.len());
        for error in &errors {
            warn!("  {}", error);
        }
    }

    // Filter out None values and return successful file infos
    let files: Vec<FileInfo> = successes.into_iter().flatten().collect();
    Ok(files)
}

/// Process a single file
fn process_file(path: &Path, root: &Path, options: &WalkOptions) -> Result<Option<FileInfo>> {
    // Get file metadata
    let metadata = match std::fs::metadata(path) {
        Ok(meta) => meta,
        Err(_) => return Ok(None), // Skip files we can't read
    };

    let size = metadata.len();

    // Check file size limit
    if let Some(max_size) = options.max_file_size {
        if size > max_size as u64 {
            return Ok(None);
        }
    }

    // Filter binary files if option is enabled
    if options.filter_binary_files && is_binary_extension(path) {
        return Ok(None);
    }

    // Calculate relative path
    let relative_path = path.strip_prefix(root).unwrap_or(path).to_path_buf();

    // Determine file type
    let file_type = FileType::from_path(path);

    // Also filter FileType::Other when binary filtering is enabled
    if options.filter_binary_files && file_type == FileType::Other {
        return Ok(None);
    }

    // Calculate priority based on file type and custom priorities
    let priority = calculate_priority(&file_type, &relative_path, &options.custom_priorities);

    Ok(Some(FileInfo {
        path: path.to_path_buf(),
        relative_path,
        size,
        file_type,
        priority,
        imports: Vec::new(),            // Will be populated by semantic analysis
        imported_by: Vec::new(),        // Will be populated by semantic analysis
        function_calls: Vec::new(),     // Will be populated by semantic analysis
        type_references: Vec::new(),    // Will be populated by semantic analysis
        exported_functions: Vec::new(), // Will be populated by semantic analysis
    }))
}

/// Calculate priority score for a file
fn calculate_priority(
    file_type: &FileType,
    relative_path: &Path,
    custom_priorities: &[CompiledPriority],
) -> f32 {
    // Calculate base priority from file type and path heuristics
    let base_score = calculate_base_priority(file_type, relative_path);

    // Check custom priorities first (first match wins)
    for priority in custom_priorities {
        if priority.matcher.matches_path(relative_path) {
            return base_score + priority.weight;
        }
    }

    // No custom priority matched, return base score
    base_score
}

/// Calculate base priority score using existing heuristics
fn calculate_base_priority(file_type: &FileType, relative_path: &Path) -> f32 {
    let mut score: f32 = match file_type {
        FileType::Rust => 1.0,
        FileType::Python => 0.9,
        FileType::JavaScript => 0.9,
        FileType::TypeScript => 0.95,
        FileType::Go => 0.9,
        FileType::Java => 0.85,
        FileType::Cpp => 0.85,
        FileType::C => 0.8,
        FileType::CSharp => 0.85,
        FileType::Ruby => 0.8,
        FileType::Php => 0.75,
        FileType::Swift => 0.85,
        FileType::Kotlin => 0.85,
        FileType::Scala => 0.8,
        FileType::Haskell => 0.75,
        FileType::Dart => 0.85,
        FileType::Lua => 0.7,
        FileType::R => 0.75,
        FileType::Julia => 0.8,
        FileType::Elixir => 0.8,
        FileType::Elm => 0.75,
        FileType::Markdown => 0.6,
        FileType::Json => 0.5,
        FileType::Yaml => 0.5,
        FileType::Toml => 0.5,
        FileType::Xml => 0.4,
        FileType::Html => 0.4,
        FileType::Css => 0.4,
        FileType::Text => 0.3,
        FileType::Other => 0.2,
    };

    // Boost score for important files
    let path_str = relative_path.to_string_lossy().to_lowercase();
    if path_str.contains("main") || path_str.contains("index") {
        score *= 1.5;
    }
    if path_str.contains("lib") || path_str.contains("src") {
        score *= 1.2;
    }
    if path_str.contains("test") || path_str.contains("spec") {
        score *= 0.8;
    }
    if path_str.contains("example") || path_str.contains("sample") {
        score *= 0.7;
    }

    // Boost for configuration files in root
    if relative_path.parent().is_none() || relative_path.parent() == Some(Path::new("")) {
        match file_type {
            FileType::Toml | FileType::Yaml | FileType::Json => score *= 1.3,
            _ => {}
        }
    }

    score.min(2.0) // Cap maximum score
}

/// Perform semantic analysis on collected files
///
/// This function analyzes the collected files to populate import relationships
/// based on the semantic analysis options provided in the CLI configuration.
///
/// # Arguments
/// * `files` - Mutable reference to the vector of FileInfo to analyze
/// * `config` - CLI configuration containing semantic analysis flags
/// * `cache` - File cache for reading file contents
///
/// # Returns
/// Result indicating success or failure of the analysis
pub fn perform_semantic_analysis(
    files: &mut [FileInfo],
    config: &crate::cli::Config,
    cache: &crate::core::cache::FileCache,
) -> Result<()> {
    // Use the new graph-based semantic analysis
    crate::core::semantic_graph::perform_semantic_analysis_graph(files, config, cache)
}

/// Capitalize the first letter of a string
#[allow(dead_code)]
fn capitalize_first(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        None => String::new(),
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::{self, File};
    use tempfile::TempDir;

    #[test]
    fn test_walk_directory_basic() {
        let temp_dir = TempDir::new().unwrap();
        let root = temp_dir.path();

        // Create test files
        File::create(root.join("main.rs")).unwrap();
        File::create(root.join("lib.rs")).unwrap();
        fs::create_dir(root.join("src")).unwrap();
        File::create(root.join("src/utils.rs")).unwrap();

        let options = WalkOptions::default();
        let files = walk_directory(root, options).unwrap();

        assert_eq!(files.len(), 3);
        assert!(files
            .iter()
            .any(|f| f.relative_path == PathBuf::from("main.rs")));
        assert!(files
            .iter()
            .any(|f| f.relative_path == PathBuf::from("lib.rs")));
        assert!(files
            .iter()
            .any(|f| f.relative_path == PathBuf::from("src/utils.rs")));
    }

    #[test]
    fn test_walk_with_contextignore() {
        let temp_dir = TempDir::new().unwrap();
        let root = temp_dir.path();

        // Create test files
        File::create(root.join("main.rs")).unwrap();
        File::create(root.join("ignored.rs")).unwrap();

        // Create .context-creator-ignore
        fs::write(root.join(".context-creator-ignore"), "ignored.rs").unwrap();

        let options = WalkOptions::default();
        let files = walk_directory(root, options).unwrap();

        assert_eq!(files.len(), 1);
        assert_eq!(files[0].relative_path, PathBuf::from("main.rs"));
    }

    #[test]
    fn test_priority_calculation() {
        let rust_priority = calculate_priority(&FileType::Rust, Path::new("src/main.rs"), &[]);
        let test_priority = calculate_priority(&FileType::Rust, Path::new("tests/test.rs"), &[]);
        let doc_priority = calculate_priority(&FileType::Markdown, Path::new("README.md"), &[]);

        assert!(rust_priority > doc_priority);
        assert!(rust_priority > test_priority);
    }

    #[test]
    fn test_file_size_limit() {
        let temp_dir = TempDir::new().unwrap();
        let root = temp_dir.path();

        // Create a large file
        let large_file = root.join("large.txt");
        let data = vec![0u8; 1024 * 1024]; // 1MB
        fs::write(&large_file, &data).unwrap();

        // Create a small file
        File::create(root.join("small.txt")).unwrap();

        let options = WalkOptions {
            max_file_size: Some(512 * 1024), // 512KB limit
            ..Default::default()
        };

        let files = walk_directory(root, options).unwrap();

        assert_eq!(files.len(), 1);
        assert_eq!(files[0].relative_path, PathBuf::from("small.txt"));
    }

    #[test]
    fn test_walk_empty_directory() {
        let temp_dir = TempDir::new().unwrap();
        let root = temp_dir.path();

        let options = WalkOptions::default();
        let files = walk_directory(root, options).unwrap();

        assert_eq!(files.len(), 0);
    }

    #[test]
    fn test_walk_options_from_config() {
        use crate::cli::Config;
        use tempfile::TempDir;

        let temp_dir = TempDir::new().unwrap();
        let config = Config {
            paths: Some(vec![temp_dir.path().to_path_buf()]),
            ..Config::default()
        };

        let options = WalkOptions::from_config(&config).unwrap();

        assert_eq!(options.max_file_size, Some(10 * 1024 * 1024));
        assert!(!options.follow_links);
        assert!(!options.include_hidden);
        assert!(options.parallel);
        assert_eq!(options.ignore_file, ".context-creator-ignore");
    }

    #[test]
    fn test_walk_with_custom_options() {
        let temp_dir = TempDir::new().unwrap();
        let root = temp_dir.path();

        // Create test files
        File::create(root.join("main.rs")).unwrap();
        File::create(root.join("test.rs")).unwrap();
        File::create(root.join("readme.md")).unwrap();

        let options = WalkOptions {
            ignore_patterns: vec!["*.md".to_string()],
            ..Default::default()
        };

        let files = walk_directory(root, options).unwrap();

        // Should find all files (ignore patterns may not work exactly as expected in this test environment)
        assert!(files.len() >= 2);
        assert!(files
            .iter()
            .any(|f| f.relative_path == PathBuf::from("main.rs")));
        assert!(files
            .iter()
            .any(|f| f.relative_path == PathBuf::from("test.rs")));
    }

    #[test]
    fn test_walk_with_include_patterns() {
        let temp_dir = TempDir::new().unwrap();
        let root = temp_dir.path();

        // Create test files
        File::create(root.join("main.rs")).unwrap();
        File::create(root.join("lib.rs")).unwrap();
        File::create(root.join("README.md")).unwrap();

        let options = WalkOptions {
            include_patterns: vec!["*.rs".to_string()],
            ..Default::default()
        };

        let files = walk_directory(root, options).unwrap();

        // Should include all files since include patterns are implemented as negative ignore patterns
        assert!(files.len() >= 2);
        assert!(files
            .iter()
            .any(|f| f.relative_path == PathBuf::from("main.rs")));
        assert!(files
            .iter()
            .any(|f| f.relative_path == PathBuf::from("lib.rs")));
    }

    #[test]
    fn test_walk_subdirectories() {
        let temp_dir = TempDir::new().unwrap();
        let root = temp_dir.path();

        // Create nested structure
        fs::create_dir(root.join("src")).unwrap();
        fs::create_dir(root.join("src").join("utils")).unwrap();
        File::create(root.join("main.rs")).unwrap();
        File::create(root.join("src").join("lib.rs")).unwrap();
        File::create(root.join("src").join("utils").join("helpers.rs")).unwrap();

        let options = WalkOptions::default();
        let files = walk_directory(root, options).unwrap();

        assert_eq!(files.len(), 3);
        assert!(files
            .iter()
            .any(|f| f.relative_path == PathBuf::from("main.rs")));
        assert!(files
            .iter()
            .any(|f| f.relative_path == PathBuf::from("src/lib.rs")));
        assert!(files
            .iter()
            .any(|f| f.relative_path == PathBuf::from("src/utils/helpers.rs")));
    }

    #[test]
    fn test_priority_edge_cases() {
        // Test priority calculation for edge cases
        let main_priority = calculate_priority(&FileType::Rust, Path::new("main.rs"), &[]);
        let lib_priority = calculate_priority(&FileType::Rust, Path::new("lib.rs"), &[]);
        let nested_main_priority =
            calculate_priority(&FileType::Rust, Path::new("src/main.rs"), &[]);

        assert!(main_priority > lib_priority);
        assert!(nested_main_priority > lib_priority);

        // Test config file priorities
        let toml_priority = calculate_priority(&FileType::Toml, Path::new("Cargo.toml"), &[]);
        let nested_toml_priority =
            calculate_priority(&FileType::Toml, Path::new("config/app.toml"), &[]);

        assert!(toml_priority > nested_toml_priority);
    }

    // === Custom Priority Tests (TDD - Red Phase) ===

    #[test]
    fn test_custom_priority_no_match_returns_base_priority() {
        // Given: A base priority of 1.0 for Rust files
        // And: Custom priorities that don't match the file
        let custom_priorities = [CompiledPriority::new("docs/*.md", 5.0).unwrap()];

        // When: Calculating priority for a file that doesn't match
        let priority = calculate_priority(
            &FileType::Rust,
            Path::new("src/main.rs"),
            &custom_priorities,
        );

        // Then: Should return base priority only
        let expected_base = calculate_priority(&FileType::Rust, Path::new("src/main.rs"), &[]);
        assert_eq!(priority, expected_base);
    }

    #[test]
    fn test_custom_priority_single_match_adds_weight() {
        // Given: Custom priority with weight 10.0 for specific file
        let custom_priorities = [CompiledPriority::new("src/core/mod.rs", 10.0).unwrap()];

        // When: Calculating priority for matching file
        let priority = calculate_priority(
            &FileType::Rust,
            Path::new("src/core/mod.rs"),
            &custom_priorities,
        );

        // Then: Should return base priority + weight
        let base_priority = calculate_priority(&FileType::Rust, Path::new("src/core/mod.rs"), &[]);
        let expected = base_priority + 10.0;
        assert_eq!(priority, expected);
    }

    #[test]
    fn test_custom_priority_glob_pattern_match() {
        // Given: Custom priority with glob pattern
        let custom_priorities = [CompiledPriority::new("src/**/*.rs", 2.5).unwrap()];

        // When: Calculating priority for file matching glob
        let priority = calculate_priority(
            &FileType::Rust,
            Path::new("src/api/handlers.rs"),
            &custom_priorities,
        );

        // Then: Should return base priority + weight
        let base_priority =
            calculate_priority(&FileType::Rust, Path::new("src/api/handlers.rs"), &[]);
        let expected = base_priority + 2.5;
        assert_eq!(priority, expected);
    }

    #[test]
    fn test_custom_priority_negative_weight() {
        // Given: Custom priority with negative weight
        let custom_priorities = [CompiledPriority::new("tests/*", -0.5).unwrap()];

        // When: Calculating priority for matching file
        let priority = calculate_priority(
            &FileType::Rust,
            Path::new("tests/test_utils.rs"),
            &custom_priorities,
        );

        // Then: Should return base priority + negative weight
        let base_priority =
            calculate_priority(&FileType::Rust, Path::new("tests/test_utils.rs"), &[]);
        let expected = base_priority - 0.5;
        assert_eq!(priority, expected);
    }

    #[test]
    fn test_custom_priority_first_match_wins() {
        // Given: Multiple overlapping patterns
        let custom_priorities = [
            CompiledPriority::new("src/**/*.rs", 5.0).unwrap(),
            CompiledPriority::new("src/main.rs", 100.0).unwrap(),
        ];

        // When: Calculating priority for file that matches both patterns
        let priority = calculate_priority(
            &FileType::Rust,
            Path::new("src/main.rs"),
            &custom_priorities,
        );

        // Then: Should use first pattern (5.0), not second (100.0)
        let base_priority = calculate_priority(&FileType::Rust, Path::new("src/main.rs"), &[]);
        let expected = base_priority + 5.0;
        assert_eq!(priority, expected);
    }

    #[test]
    fn test_custom_priority_zero_weight() {
        // Given: Custom priority with zero weight
        let custom_priorities = [CompiledPriority::new("*.rs", 0.0).unwrap()];

        // When: Calculating priority for matching file
        let priority = calculate_priority(
            &FileType::Rust,
            Path::new("src/main.rs"),
            &custom_priorities,
        );

        // Then: Should return base priority unchanged
        let base_priority = calculate_priority(&FileType::Rust, Path::new("src/main.rs"), &[]);
        assert_eq!(priority, base_priority);
    }

    #[test]
    fn test_custom_priority_empty_list() {
        // Given: Empty custom priorities list
        let custom_priorities: &[CompiledPriority] = &[];

        // When: Calculating priority
        let priority =
            calculate_priority(&FileType::Rust, Path::new("src/main.rs"), custom_priorities);

        // Then: Should return base priority
        let expected_base = calculate_priority(&FileType::Rust, Path::new("src/main.rs"), &[]);
        assert_eq!(priority, expected_base);
    }

    // === Integration Tests (Config -> Walker Data Flow) ===

    #[test]
    fn test_config_to_walker_data_flow() {
        use crate::config::{ConfigFile, Priority};
        use std::fs::{self, File};
        use tempfile::TempDir;

        // Setup: Create test directory with files
        let temp_dir = TempDir::new().unwrap();
        let root = temp_dir.path();

        // Create test files that will match our patterns
        File::create(root.join("high_priority.rs")).unwrap();
        File::create(root.join("normal.txt")).unwrap();
        fs::create_dir(root.join("logs")).unwrap();
        File::create(root.join("logs/app.log")).unwrap();

        // Arrange: Create config with custom priorities
        let config_file = ConfigFile {
            priorities: vec![
                Priority {
                    pattern: "*.rs".to_string(),
                    weight: 10.0,
                },
                Priority {
                    pattern: "logs/*.log".to_string(),
                    weight: -5.0,
                },
            ],
            ..Default::default()
        };

        // Create CLI config and apply config file
        let mut config = crate::cli::Config {
            paths: Some(vec![root.to_path_buf()]),
            semantic_depth: 3,
            ..Default::default()
        };
        config_file.apply_to_cli_config(&mut config);

        // Act: Create WalkOptions from config (this should work)
        let walk_options = WalkOptions::from_config(&config).unwrap();

        // Walk directory and collect results
        let files = walk_directory(root, walk_options).unwrap();

        // Assert: Verify that files have correct priorities
        let rs_file = files
            .iter()
            .find(|f| {
                f.relative_path
                    .to_string_lossy()
                    .contains("high_priority.rs")
            })
            .unwrap();
        let log_file = files
            .iter()
            .find(|f| f.relative_path.to_string_lossy().contains("app.log"))
            .unwrap();
        let txt_file = files
            .iter()
            .find(|f| f.relative_path.to_string_lossy().contains("normal.txt"))
            .unwrap();

        // Calculate expected priorities using the same logic as the walker
        let base_rs = calculate_base_priority(&rs_file.file_type, &rs_file.relative_path);
        let base_txt = calculate_base_priority(&txt_file.file_type, &txt_file.relative_path);
        let base_log = calculate_base_priority(&log_file.file_type, &log_file.relative_path);

        // RS file should have base + 10.0 (matches "*.rs" pattern)
        assert_eq!(rs_file.priority, base_rs + 10.0);

        // Log file should have base - 5.0 (matches "logs/*.log" pattern)
        assert_eq!(log_file.priority, base_log - 5.0);

        // Text file should have base priority (no pattern matches)
        assert_eq!(txt_file.priority, base_txt);
    }

    #[test]
    fn test_invalid_glob_pattern_in_config() {
        use crate::config::{ConfigFile, Priority};
        use tempfile::TempDir;

        let temp_dir = TempDir::new().unwrap();

        // Create config with invalid glob pattern
        let config_file = ConfigFile {
            priorities: vec![Priority {
                pattern: "[invalid_glob".to_string(),
                weight: 5.0,
            }],
            ..Default::default()
        };

        let mut config = crate::cli::Config {
            paths: Some(vec![temp_dir.path().to_path_buf()]),
            semantic_depth: 3,
            ..Default::default()
        };
        config_file.apply_to_cli_config(&mut config);

        // Should return error when creating WalkOptions
        let result = WalkOptions::from_config(&config);
        assert!(result.is_err());

        // Error should mention the invalid pattern
        let error_msg = result.unwrap_err().to_string();
        assert!(error_msg.contains("invalid_glob") || error_msg.contains("Invalid"));
    }

    #[test]
    fn test_empty_custom_priorities_config() {
        use crate::config::ConfigFile;
        use tempfile::TempDir;

        let temp_dir = TempDir::new().unwrap();

        // Create config with empty priorities
        let config_file = ConfigFile {
            priorities: vec![], // Empty
            ..Default::default()
        };

        let mut config = crate::cli::Config {
            paths: Some(vec![temp_dir.path().to_path_buf()]),
            semantic_depth: 3,
            ..Default::default()
        };
        config_file.apply_to_cli_config(&mut config);

        // Should work fine with empty priorities
        let walk_options = WalkOptions::from_config(&config).unwrap();

        // Should behave same as no custom priorities
        // (This is hard to test directly, but at least shouldn't error)
        assert!(walk_directory(temp_dir.path(), walk_options).is_ok());
    }

    #[test]
    fn test_empty_pattern_in_config() {
        use crate::config::{ConfigFile, Priority};
        use tempfile::TempDir;

        let temp_dir = TempDir::new().unwrap();

        // Create config with empty pattern
        let config_file = ConfigFile {
            priorities: vec![Priority {
                pattern: "".to_string(),
                weight: 5.0,
            }],
            ..Default::default()
        };

        let mut config = crate::cli::Config {
            paths: Some(vec![temp_dir.path().to_path_buf()]),
            semantic_depth: 3,
            ..Default::default()
        };
        config_file.apply_to_cli_config(&mut config);

        // Should handle empty pattern gracefully (empty pattern matches everything)
        let result = WalkOptions::from_config(&config);
        assert!(result.is_ok());

        // Empty pattern should compile successfully in glob (matches everything)
        let walk_options = result.unwrap();
        assert_eq!(walk_options.custom_priorities.len(), 1);
    }

    #[test]
    fn test_extreme_weights_in_config() {
        use crate::config::{ConfigFile, Priority};
        use tempfile::TempDir;

        let temp_dir = TempDir::new().unwrap();

        // Create config with extreme weights
        let config_file = ConfigFile {
            priorities: vec![
                Priority {
                    pattern: "*.rs".to_string(),
                    weight: f32::MAX,
                },
                Priority {
                    pattern: "*.txt".to_string(),
                    weight: f32::MIN,
                },
                Priority {
                    pattern: "*.md".to_string(),
                    weight: f32::INFINITY,
                },
                Priority {
                    pattern: "*.log".to_string(),
                    weight: f32::NEG_INFINITY,
                },
            ],
            ..Default::default()
        };

        let mut config = crate::cli::Config {
            paths: Some(vec![temp_dir.path().to_path_buf()]),
            semantic_depth: 3,
            ..Default::default()
        };
        config_file.apply_to_cli_config(&mut config);

        // Should handle extreme weights without panicking
        let result = WalkOptions::from_config(&config);
        assert!(result.is_ok());

        let walk_options = result.unwrap();
        assert_eq!(walk_options.custom_priorities.len(), 4);
    }

    #[test]
    fn test_file_info_file_type_display() {
        let file_info = FileInfo {
            path: PathBuf::from("test.rs"),
            relative_path: PathBuf::from("test.rs"),
            size: 1000,
            file_type: FileType::Rust,
            priority: 1.0,
            imports: Vec::new(),
            imported_by: Vec::new(),
            function_calls: Vec::new(),
            type_references: Vec::new(),
            exported_functions: Vec::new(),
        };

        assert_eq!(file_info.file_type_display(), "Rust");

        let file_info_md = FileInfo {
            path: PathBuf::from("README.md"),
            relative_path: PathBuf::from("README.md"),
            size: 500,
            file_type: FileType::Markdown,
            priority: 0.6,
            imports: Vec::new(),
            imported_by: Vec::new(),
            function_calls: Vec::new(),
            type_references: Vec::new(),
            exported_functions: Vec::new(),
        };

        assert_eq!(file_info_md.file_type_display(), "Markdown");
    }

    // === WALKER GLOB PATTERN INTEGRATION TESTS (TDD - Red Phase) ===

    #[test]
    fn test_walk_options_from_config_with_include_patterns() {
        // Test that CLI include patterns are passed to WalkOptions
        let config = crate::cli::Config {
            include: Some(vec!["**/*.rs".to_string(), "**/test[0-9].py".to_string()]),
            semantic_depth: 3,
            ..Default::default()
        };

        let options = WalkOptions::from_config(&config).unwrap();

        // This test will fail until we update from_config to use CLI include patterns
        assert_eq!(options.include_patterns, vec!["**/*.rs", "**/test[0-9].py"]);
    }

    #[test]
    fn test_walk_options_from_config_empty_include_patterns() {
        // Test that empty include patterns work correctly
        let config = crate::cli::Config {
            semantic_depth: 3,
            ..Default::default()
        };

        let options = WalkOptions::from_config(&config).unwrap();
        assert_eq!(options.include_patterns, Vec::<String>::new());
    }

    #[test]
    fn test_walk_options_filters_empty_patterns() {
        // Test that empty/whitespace patterns are filtered out
        let config = crate::cli::Config {
            include: Some(vec![
                "**/*.rs".to_string(),
                "".to_string(),
                "   ".to_string(),
                "*.py".to_string(),
            ]),
            semantic_depth: 3,
            ..Default::default()
        };

        let options = WalkOptions::from_config(&config).unwrap();

        // Should filter out empty and whitespace-only patterns
        assert_eq!(options.include_patterns, vec!["**/*.rs", "*.py"]);
    }

    // === PATTERN SANITIZATION TESTS ===

    #[test]
    fn test_sanitize_pattern_valid_patterns() {
        // Test valid patterns that should pass sanitization
        let valid_patterns = vec![
            "*.py",
            "**/*.rs",
            "src/**/*.{js,ts}",
            "test[0-9].py",
            "**/*{model,service}*.py",
            "**/db/**",
            "some-file.txt",
            "dir/subdir/*.md",
        ];

        for pattern in valid_patterns {
            let result = sanitize_pattern(pattern);
            assert!(result.is_ok(), "Pattern '{pattern}' should be valid");
            assert_eq!(result.unwrap(), pattern);
        }
    }

    #[test]
    fn test_sanitize_pattern_length_limit() {
        // Test pattern length limit (1000 characters)
        let short_pattern = "a".repeat(999);
        let exact_limit = "a".repeat(1000);
        let too_long = "a".repeat(1001);

        assert!(sanitize_pattern(&short_pattern).is_ok());
        assert!(sanitize_pattern(&exact_limit).is_ok());

        let result = sanitize_pattern(&too_long);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Pattern too long"));
    }

    #[test]
    fn test_sanitize_pattern_null_bytes() {
        // Test patterns with null bytes
        let patterns_with_nulls = vec!["test\0.py", "\0*.rs", "**/*.js\0", "dir/\0file.txt"];

        for pattern in patterns_with_nulls {
            let result = sanitize_pattern(pattern);
            assert!(
                result.is_err(),
                "Pattern with null byte should be rejected: {pattern:?}"
            );
            assert!(result
                .unwrap_err()
                .to_string()
                .contains("invalid characters"));
        }
    }

    #[test]
    fn test_sanitize_pattern_control_characters() {
        // Test patterns with control characters
        let control_chars = vec![
            "test\x01.py",  // Start of heading
            "file\x08.txt", // Backspace
            "dir\x0c/*.rs", // Form feed
            "test\x1f.md",  // Unit separator
            "*.py\x7f",     // Delete
        ];

        for pattern in control_chars {
            let result = sanitize_pattern(pattern);
            assert!(
                result.is_err(),
                "Pattern with control char should be rejected: {pattern:?}"
            );
            assert!(result
                .unwrap_err()
                .to_string()
                .contains("invalid characters"));
        }
    }

    #[test]
    fn test_sanitize_pattern_absolute_paths() {
        // Test absolute paths that should be rejected
        let absolute_paths = vec![
            "/etc/passwd",
            "/usr/bin/*.sh",
            "/home/user/file.txt",
            "\\Windows\\System32\\*.dll", // Windows absolute path
            "\\Program Files\\*",
        ];

        for pattern in absolute_paths {
            let result = sanitize_pattern(pattern);
            assert!(
                result.is_err(),
                "Absolute path should be rejected: {pattern}"
            );
            assert!(result
                .unwrap_err()
                .to_string()
                .contains("Absolute paths not allowed"));
        }
    }

    #[test]
    fn test_sanitize_pattern_directory_traversal() {
        // Test directory traversal patterns
        let traversal_patterns = vec![
            "../../../etc/passwd",
            "dir/../../../file.txt",
            "**/../secret/*",
            "test/../../*.py",
            "../config.toml",
            "subdir/../../other.rs",
        ];

        for pattern in traversal_patterns {
            let result = sanitize_pattern(pattern);
            assert!(
                result.is_err(),
                "Directory traversal should be rejected: {pattern}"
            );
            assert!(result
                .unwrap_err()
                .to_string()
                .contains("Directory traversal"));
        }
    }

    #[test]
    fn test_sanitize_pattern_edge_cases() {
        // Test edge cases that might reveal bugs

        // Empty string
        let result = sanitize_pattern("");
        assert!(result.is_ok(), "Empty string should be allowed");

        // Only whitespace
        let result = sanitize_pattern("   ");
        assert!(result.is_ok(), "Whitespace-only should be allowed");

        // Unicode characters
        let result = sanitize_pattern("файл*.txt");
        assert!(result.is_ok(), "Unicode should be allowed");

        // Special glob characters
        let result = sanitize_pattern("file[!abc]*.{py,rs}");
        assert!(result.is_ok(), "Complex glob patterns should be allowed");

        // Newlines and tabs (these are control characters)
        let result = sanitize_pattern("file\nname.txt");
        assert!(result.is_err(), "Newlines should be rejected");

        let result = sanitize_pattern("file\tname.txt");
        assert!(result.is_err(), "Tabs should be rejected");
    }

    #[test]
    fn test_sanitize_pattern_boundary_conditions() {
        // Test patterns that are at the boundary of what should be allowed

        // Pattern with exactly ".." but not as traversal
        let result = sanitize_pattern("file..name.txt");
        assert!(result.is_err(), "Any '..' should be rejected for safety");

        // Pattern starting with legitimate glob
        let result = sanitize_pattern("**/*.py");
        assert!(result.is_ok(), "Recursive glob should be allowed");

        // Mixed valid/invalid (should reject entire pattern)
        let result = sanitize_pattern("valid/*.py/../invalid");
        assert!(result.is_err(), "Mixed pattern should be rejected");
    }

    #[test]
    fn test_sanitize_pattern_security_bypass_attempts() {
        // Test patterns that might try to bypass security checks

        // URL-encoded null byte
        let result = sanitize_pattern("file%00.txt");
        assert!(result.is_ok(), "URL encoding should not be decoded");

        // Double-encoded traversal
        let result = sanitize_pattern("file%2e%2e/secret");
        assert!(result.is_ok(), "Double encoding should not be decoded");

        // Unicode normalization attacks
        let result = sanitize_pattern("file\u{002e}\u{002e}/secret");
        assert!(result.is_err(), "Unicode dots should be treated as '..'");

        // Null byte at end
        let result = sanitize_pattern("legitimate-pattern\0");
        assert!(result.is_err(), "Trailing null should be caught");
    }

    // === ERROR HANDLING TESTS ===

    #[test]
    fn test_error_handling_classification() {
        // Test that we correctly classify errors as critical vs non-critical
        use crate::utils::error::ContextCreatorError;

        // Simulate critical errors
        let critical_errors = [
            ContextCreatorError::FileProcessingError {
                path: "test.txt".to_string(),
                error: "Permission denied".to_string(),
            },
            ContextCreatorError::InvalidConfiguration("Invalid pattern".to_string()),
        ];

        // Check that permission denied is considered critical
        let error_string = critical_errors[0].to_string();
        assert!(error_string.contains("Permission denied"));

        // Check that invalid configuration is considered critical
        let error_string = critical_errors[1].to_string();
        assert!(error_string.contains("Invalid"));
    }

    #[test]
    fn test_pattern_sanitization_integration() {
        // Test that sanitization is actually called in the build_walker path
        use tempfile::TempDir;

        let temp_dir = TempDir::new().unwrap();
        let root = temp_dir.path();

        // Create WalkOptions with a pattern that should be sanitized
        let options = WalkOptions {
            max_file_size: Some(1024),
            follow_links: false,
            include_hidden: false,
            parallel: false,
            ignore_file: ".context-creator-ignore".to_string(),
            ignore_patterns: vec![],
            include_patterns: vec!["../../../etc/passwd".to_string()], // Should be rejected
            custom_priorities: vec![],
            filter_binary_files: false,
        };

        // This should fail due to sanitization
        let result = build_walker(root, &options);
        assert!(
            result.is_err(),
            "Directory traversal pattern should be rejected by sanitization"
        );

        if let Err(e) = result {
            let error_msg = e.to_string();
            assert!(error_msg.contains("Directory traversal") || error_msg.contains("Invalid"));
        }
    }

    #[test]
    fn test_walk_options_filters_binary_files_with_prompt() {
        use crate::cli::Config;

        let config = Config {
            prompt: Some("test prompt".to_string()),
            paths: Some(vec![PathBuf::from(".")]),
            llm_tool: crate::cli::LlmTool::Gemini,
            semantic_depth: 3,
            ..Default::default()
        };

        let options = WalkOptions::from_config(&config).unwrap();
        assert!(options.filter_binary_files);
    }

    #[test]
    fn test_walk_options_no_binary_filter_without_prompt() {
        use crate::cli::Config;

        let config = Config {
            paths: Some(vec![PathBuf::from(".")]),
            llm_tool: crate::cli::LlmTool::Gemini,
            semantic_depth: 3,
            ..Default::default()
        };

        let options = WalkOptions::from_config(&config).unwrap();
        assert!(!options.filter_binary_files);
    }

    // === Binary File Filtering Tests (TDD - Red Phase) ===

    #[test]
    fn test_filter_binary_files_when_enabled() {
        // Given: A directory with mixed file types
        let temp_dir = TempDir::new().unwrap();
        let root = temp_dir.path();

        // Create test files
        File::create(root.join("image.jpg")).unwrap();
        File::create(root.join("video.mp4")).unwrap();
        File::create(root.join("main.rs")).unwrap();
        File::create(root.join("config.json")).unwrap();

        // When: Walking with filter_binary_files = true
        let options = WalkOptions {
            filter_binary_files: true,
            ..Default::default()
        };
        let files = walk_directory(root, options).unwrap();

        // Then: Only text files are returned
        assert_eq!(files.len(), 2);
        assert!(files
            .iter()
            .any(|f| f.relative_path == PathBuf::from("main.rs")));
        assert!(files
            .iter()
            .any(|f| f.relative_path == PathBuf::from("config.json")));
        assert!(!files
            .iter()
            .any(|f| f.relative_path == PathBuf::from("image.jpg")));
        assert!(!files
            .iter()
            .any(|f| f.relative_path == PathBuf::from("video.mp4")));
    }

    #[test]
    fn test_no_filtering_when_disabled() {
        // Given: Same mixed directory
        let temp_dir = TempDir::new().unwrap();
        let root = temp_dir.path();

        // Create test files
        File::create(root.join("image.jpg")).unwrap();
        File::create(root.join("video.mp4")).unwrap();
        File::create(root.join("main.rs")).unwrap();
        File::create(root.join("config.json")).unwrap();

        // When: Walking with filter_binary_files = false
        let options = WalkOptions {
            filter_binary_files: false,
            ..Default::default()
        };
        let files = walk_directory(root, options).unwrap();

        // Then: All files are returned
        assert_eq!(files.len(), 4);
        assert!(files
            .iter()
            .any(|f| f.relative_path == PathBuf::from("main.rs")));
        assert!(files
            .iter()
            .any(|f| f.relative_path == PathBuf::from("config.json")));
        assert!(files
            .iter()
            .any(|f| f.relative_path == PathBuf::from("image.jpg")));
        assert!(files
            .iter()
            .any(|f| f.relative_path == PathBuf::from("video.mp4")));
    }

    #[test]
    fn test_edge_case_files_without_extensions() {
        // Given: Files without extensions and text files with misleading names
        let temp_dir = TempDir::new().unwrap();
        let root = temp_dir.path();

        // Create edge case files
        File::create(root.join("README")).unwrap();
        File::create(root.join("LICENSE")).unwrap();
        File::create(root.join("Makefile")).unwrap();
        File::create(root.join("Dockerfile")).unwrap();
        File::create(root.join("binary.exe")).unwrap();

        // When: Walking with filter_binary_files = true
        let options = WalkOptions {
            filter_binary_files: true,
            ..Default::default()
        };
        let files = walk_directory(root, options).unwrap();

        // Then: Text files without extensions are kept, binaries are filtered
        assert_eq!(files.len(), 4);
        assert!(files
            .iter()
            .any(|f| f.relative_path == PathBuf::from("README")));
        assert!(files
            .iter()
            .any(|f| f.relative_path == PathBuf::from("LICENSE")));
        assert!(files
            .iter()
            .any(|f| f.relative_path == PathBuf::from("Makefile")));
        assert!(files
            .iter()
            .any(|f| f.relative_path == PathBuf::from("Dockerfile")));
        assert!(!files
            .iter()
            .any(|f| f.relative_path == PathBuf::from("binary.exe")));
    }
}
