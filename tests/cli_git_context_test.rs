#![cfg(test)]

use clap::Parser;
use context_creator::cli::Config;

#[test]
fn test_git_context_flag_parsing() {
    // Test that --git-context flag is parsed correctly
    let args = vec!["context-creator", "--git-context", "."];
    let config = Config::parse_from(args);
    assert!(
        config.git_context,
        "git_context flag should be true when specified"
    );
}

#[test]
fn test_git_context_default_false() {
    // Test that git_context defaults to false
    let args = vec!["context-creator", "."];
    let config = Config::parse_from(args);
    assert!(
        !config.git_context,
        "git_context flag should default to false"
    );
}

#[test]
fn test_git_context_with_enhanced_context() {
    // Test combination with other flags
    let args = vec![
        "context-creator",
        "--git-context",
        "--enhanced-context",
        ".",
    ];
    let config = Config::parse_from(args);
    assert!(config.git_context, "git_context flag should be true");
    assert!(
        config.enhanced_context,
        "enhanced_context flag should be true"
    );
}

#[test]
fn test_git_context_depth_flag() {
    // Test that --git-context-depth flag is parsed correctly
    let args = vec![
        "context-creator",
        "--git-context",
        "--git-context-depth",
        "5",
        ".",
    ];
    let config = Config::parse_from(args);
    assert!(config.git_context, "git_context flag should be true");
    assert_eq!(config.git_context_depth, 5, "git_context_depth should be 5");
}

#[test]
fn test_git_context_depth_default() {
    // Test that git_context_depth defaults to 3
    let args = vec!["context-creator", "--git-context", "."];
    let config = Config::parse_from(args);
    assert!(config.git_context, "git_context flag should be true");
    assert_eq!(
        config.git_context_depth, 3,
        "git_context_depth should default to 3"
    );
}
