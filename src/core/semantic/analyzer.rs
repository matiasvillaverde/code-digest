//! Base trait and types for language-specific semantic analyzers

use crate::utils::error::ContextCreatorError;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// Result type for semantic analysis operations
pub type SemanticResult<T> = Result<T, ContextCreatorError>;

/// Context information for semantic analysis
#[derive(Debug, Clone)]
pub struct SemanticContext {
    /// Current file being analyzed
    pub current_file: PathBuf,
    /// Base directory for the project
    pub base_dir: PathBuf,
    /// Current depth in dependency traversal
    pub current_depth: usize,
    /// Maximum allowed depth
    pub max_depth: usize,
    /// Files already visited (for cycle detection)
    pub visited_files: HashSet<PathBuf>,
}

impl SemanticContext {
    /// Create a new semantic context
    pub fn new(current_file: PathBuf, base_dir: PathBuf, max_depth: usize) -> Self {
        Self {
            current_file,
            base_dir,
            current_depth: 0,
            max_depth,
            visited_files: HashSet::new(),
        }
    }

    /// Check if we've reached maximum depth
    pub fn at_max_depth(&self) -> bool {
        self.current_depth >= self.max_depth
    }

    /// Create a child context for analyzing a dependency
    pub fn child_context(&self, file: PathBuf) -> Option<Self> {
        if self.at_max_depth() || self.visited_files.contains(&file) {
            return None;
        }

        let mut child = self.clone();
        child.current_file = file.clone();
        child.current_depth += 1;
        child.visited_files.insert(file);
        Some(child)
    }
}

/// Information about an import statement
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Import {
    /// The module/package being imported
    pub module: String,
    /// Specific items imported (if any)
    pub items: Vec<String>,
    /// Whether this is a relative import
    pub is_relative: bool,
    /// Line number where import appears
    pub line: usize,
}

/// Information about a function call
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct FunctionCall {
    /// Name of the function being called
    pub name: String,
    /// Module the function comes from (if known)
    pub module: Option<String>,
    /// Line number where call appears
    pub line: usize,
}

/// Information about a function definition
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct FunctionDefinition {
    /// Name of the function
    pub name: String,
    /// Whether the function is exported/public
    pub is_exported: bool,
    /// Line number where function is defined
    pub line: usize,
}

/// Information about a type reference
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TypeReference {
    /// Name of the type
    pub name: String,
    /// Module the type comes from (if known)
    pub module: Option<String>,
    /// Line number where reference appears
    pub line: usize,
    /// Path to the file that defines this type
    pub definition_path: Option<PathBuf>,
    /// Whether this type is from an external dependency
    pub is_external: bool,
    /// External package name and version (e.g., "serde v1.0.197")
    pub external_package: Option<String>,
}

/// Results from semantic analysis
#[derive(Debug, Default, Clone)]
pub struct AnalysisResult {
    /// Import statements found
    pub imports: Vec<Import>,
    /// Function calls found
    pub function_calls: Vec<FunctionCall>,
    /// Type references found
    pub type_references: Vec<TypeReference>,
    /// Function definitions found
    pub exported_functions: Vec<FunctionDefinition>,
    /// Errors encountered during analysis (non-fatal)
    pub errors: Vec<String>,
}

/// Base trait for language-specific analyzers
pub trait LanguageAnalyzer: Send + Sync {
    /// Get the language name
    fn language_name(&self) -> &'static str;

    /// Analyze a file and extract semantic information
    fn analyze_file(
        &self,
        path: &Path,
        content: &str,
        context: &SemanticContext,
    ) -> SemanticResult<AnalysisResult>;

    /// Parse and analyze imports from the file
    fn analyze_imports(
        &self,
        content: &str,
        context: &SemanticContext,
    ) -> SemanticResult<Vec<Import>> {
        // Default implementation - languages can override
        let result = self.analyze_file(&context.current_file, content, context)?;
        Ok(result.imports)
    }

    /// Parse and analyze function calls from the file
    fn analyze_function_calls(
        &self,
        content: &str,
        context: &SemanticContext,
    ) -> SemanticResult<Vec<FunctionCall>> {
        // Default implementation - languages can override
        let result = self.analyze_file(&context.current_file, content, context)?;
        Ok(result.function_calls)
    }

    /// Parse and analyze type references from the file
    fn analyze_type_references(
        &self,
        content: &str,
        context: &SemanticContext,
    ) -> SemanticResult<Vec<TypeReference>> {
        // Default implementation - languages can override
        let result = self.analyze_file(&context.current_file, content, context)?;
        Ok(result.type_references)
    }

    /// Check if this analyzer can handle the given file extension
    fn can_handle_extension(&self, extension: &str) -> bool;

    /// Get file extensions this analyzer handles
    fn supported_extensions(&self) -> Vec<&'static str>;

    /// Resolve a type reference to its definition file
    /// Returns None if the type cannot be resolved or is external
    fn resolve_type_definition(
        &self,
        _type_ref: &TypeReference,
        _context: &SemanticContext,
    ) -> Option<PathBuf> {
        // Default implementation returns None
        // Languages should override this to provide type resolution
        None
    }
}
