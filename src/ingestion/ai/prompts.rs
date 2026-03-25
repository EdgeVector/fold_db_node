//! Prompt templates for AI-powered schema analysis.
//!
//! All prompt content lives in `fold_db::llm_registry::prompts::ingestion`.
//! This module re-exports those constants so existing `use super::prompts::*`
//! imports continue to work without changing every call site.

pub use fold_db::llm_registry::prompts::ingestion::FIELD_DESCRIPTIONS_PROMPT;
pub use fold_db::llm_registry::prompts::ingestion::PROMPT_ACTIONS;
pub use fold_db::llm_registry::prompts::ingestion::PROMPT_HEADER;
