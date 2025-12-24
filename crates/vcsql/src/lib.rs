//! # vcsql
//!
//! Query Git repositories using SQL.
//!
//! VCSQL provides a SQL interface to Git repositories, allowing you to query
//! commit history, branches, tags, diffs, blame, and more using standard SQL syntax.
//!
//! ## Quick Start
//!
//! ```no_run
//! use vcsql::{GitRepo, SqlEngine, Result};
//!
//! fn main() -> Result<()> {
//!     let mut repo = GitRepo::open(".")?;
//!     let mut engine = SqlEngine::new()?;
//!
//!     engine.load_tables_for_query("SELECT * FROM commits LIMIT 5", &mut repo)?;
//!     let result = engine.execute("SELECT * FROM commits LIMIT 5")?;
//!
//!     println!("Found {} commits", result.row_count());
//!     Ok(())
//! }
//! ```
//!
//! ## Available Tables
//!
//! VCSQL provides 17 queryable tables organized by category:
//!
//! - **Core**: `commits`, `commit_parents`
//! - **References**: `branches`, `tags`, `refs`, `stashes`, `reflog`
//! - **Changes**: `diffs`, `diff_files`, `blame`
//! - **Configuration**: `config`, `remotes`, `submodules`
//! - **Working Directory**: `status`, `worktrees`
//! - **Operational**: `hooks`, `notes`
//!
//! See [`TABLES`] for detailed schema information.

pub mod cli;
pub mod error;
pub mod git;
pub mod providers;
pub mod sql;

pub use cli::{Args, Command, OutputFormat};
pub use error::{Result, VcsqlError};
pub use git::GitRepo;
pub use sql::{SqlEngine, TableInfo, TABLES};
