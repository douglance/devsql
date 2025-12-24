use crate::error::Result;
use crate::git::GitRepo;
use crate::providers::Provider;
use rusqlite::Connection;

pub struct ConfigProvider;

impl Provider for ConfigProvider {
    fn table_name(&self) -> &'static str {
        "config"
    }

    fn populate(&self, conn: &Connection, repo: &mut GitRepo) -> Result<()> {
        let mut stmt = conn.prepare(
            r#"
            INSERT INTO config (
                level, section, subsection, key, name, value, repo
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
            "#,
        )?;

        let repo_path = repo.path().to_string();
        let git_repo = repo.inner();

        // Get repository config (includes local, global, system)
        if let Ok(config) = git_repo.config() {
            if let Ok(mut entries) = config.entries(None) {
                while let Some(entry) = entries.next() {
                    if let Ok(entry) = entry {
                        if let (Some(name), Some(value)) = (entry.name(), entry.value()) {
                            let level = match entry.level() {
                                git2::ConfigLevel::ProgramData => "programdata",
                                git2::ConfigLevel::System => "system",
                                git2::ConfigLevel::XDG => "xdg",
                                git2::ConfigLevel::Global => "global",
                                git2::ConfigLevel::Local => "local",
                                git2::ConfigLevel::Worktree => "worktree",
                                git2::ConfigLevel::App => "app",
                                git2::ConfigLevel::Highest => "highest",
                            };

                            let (section, subsection, key) = parse_config_name(name);

                            stmt.execute((
                                level,
                                &section,
                                &subsection,
                                &key,
                                name,
                                value,
                                &repo_path,
                            ))?;
                        }
                    }
                }
            }
        }

        Ok(())
    }
}

fn parse_config_name(name: &str) -> (String, Option<String>, String) {
    let parts: Vec<&str> = name.split('.').collect();
    match parts.len() {
        1 => (parts[0].to_string(), None, String::new()),
        2 => (parts[0].to_string(), None, parts[1].to_string()),
        3 => (
            parts[0].to_string(),
            Some(parts[1].to_string()),
            parts[2].to_string(),
        ),
        _ => {
            // Handle cases like remote.origin.url or branch.main.remote
            let section = parts[0].to_string();
            let key = parts[parts.len() - 1].to_string();
            let subsection = parts[1..parts.len() - 1].join(".");
            (section, Some(subsection), key)
        }
    }
}
