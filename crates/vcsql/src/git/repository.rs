//! Git repository wrapper for vcsql.

use crate::error::{Result, VcsqlError};
use git2::{BranchType, Commit, Reference, Repository};
use std::path::Path;

/// A wrapper around a Git repository providing simplified access to Git data.
///
/// `GitRepo` handles repository discovery and provides methods for accessing
/// commits, branches, and other Git objects needed by the SQL engine.
///
/// # Example
///
/// ```no_run
/// use vcsql::GitRepo;
///
/// let repo = GitRepo::open(".")?;
/// println!("Repository at: {}", repo.path());
/// # Ok::<(), vcsql::VcsqlError>(())
/// ```
pub struct GitRepo {
    repo: Repository,
    path: String,
}

impl GitRepo {
    /// Opens a Git repository at the given path.
    ///
    /// Uses `git2::Repository::discover` to find the repository root,
    /// supporting nested directories within a repository.
    ///
    /// # Errors
    ///
    /// Returns `VcsqlError::RepoNotFound` if no Git repository is found.
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self> {
        let path_ref = path.as_ref();
        let repo = Repository::discover(path_ref).map_err(|e| {
            if e.code() == git2::ErrorCode::NotFound {
                VcsqlError::RepoNotFound(path_ref.display().to_string())
            } else {
                VcsqlError::Git(e)
            }
        })?;

        let workdir = repo
            .workdir()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| repo.path().display().to_string());

        Ok(Self {
            repo,
            path: workdir,
        })
    }

    /// Returns the working directory path of the repository.
    pub fn path(&self) -> &str {
        &self.path
    }

    /// Returns a reference to the underlying `git2::Repository`.
    pub fn inner(&self) -> &Repository {
        &self.repo
    }

    /// Returns a mutable reference to the underlying `git2::Repository`.
    pub fn inner_mut(&mut self) -> &mut Repository {
        &mut self.repo
    }

    pub fn head(&self) -> Result<Reference<'_>> {
        Ok(self.repo.head()?)
    }

    pub fn head_commit(&self) -> Result<Commit<'_>> {
        let head = self.head()?;
        let commit = head.peel_to_commit()?;
        Ok(commit)
    }

    pub fn walk_commits(&self) -> Result<impl Iterator<Item = Result<Commit<'_>>>> {
        let mut revwalk = self.repo.revwalk()?;
        revwalk.push_head()?;
        revwalk.set_sorting(git2::Sort::TIME | git2::Sort::TOPOLOGICAL)?;

        Ok(revwalk.map(move |oid_result| match oid_result {
            Ok(oid) => self
                .repo
                .find_commit(oid)
                .map_err(VcsqlError::Git),
            Err(e) => Err(VcsqlError::Git(e)),
        }))
    }

    pub fn branches(&self, branch_type: Option<BranchType>) -> Result<git2::Branches<'_>> {
        Ok(self.repo.branches(branch_type)?)
    }

    pub fn is_head_detached(&self) -> bool {
        self.repo.head_detached().unwrap_or(false)
    }

    pub fn graph_ahead_behind(
        &self,
        local: git2::Oid,
        upstream: git2::Oid,
    ) -> Result<(usize, usize)> {
        Ok(self.repo.graph_ahead_behind(local, upstream)?)
    }
}
