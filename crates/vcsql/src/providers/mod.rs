mod blame;
mod branches;
mod commit_parents;
mod commits;
mod config;
mod diff_files;
mod diffs;
mod hooks;
mod notes;
mod reflog;
mod refs;
mod remotes;
mod stashes;
mod status;
mod submodules;
mod tags;
mod worktrees;

pub use blame::BlameProvider;
pub use branches::BranchesProvider;
pub use commit_parents::CommitParentsProvider;
pub use commits::CommitsProvider;
pub use config::ConfigProvider;
pub use diff_files::DiffFilesProvider;
pub use diffs::DiffsProvider;
pub use hooks::HooksProvider;
pub use notes::NotesProvider;
pub use reflog::ReflogProvider;
pub use refs::RefsProvider;
pub use remotes::RemotesProvider;
pub use stashes::StashesProvider;
pub use status::StatusProvider;
pub use submodules::SubmodulesProvider;
pub use tags::TagsProvider;
pub use worktrees::WorktreesProvider;

use crate::error::Result;
use crate::git::GitRepo;
use rusqlite::Connection;

pub trait Provider {
    fn table_name(&self) -> &'static str;
    fn populate(&self, conn: &Connection, repo: &mut GitRepo) -> Result<()>;
}
