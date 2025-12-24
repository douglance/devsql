//! SQL query engine for Git repositories.

use crate::error::{Result, VcsqlError};
use crate::git::GitRepo;
use crate::providers::{
    BlameProvider, BranchesProvider, CommitParentsProvider, CommitsProvider, ConfigProvider,
    DiffFilesProvider, DiffsProvider, HooksProvider, NotesProvider, Provider, ReflogProvider,
    RefsProvider, RemotesProvider, StashesProvider, StatusProvider, SubmodulesProvider,
    TagsProvider, WorktreesProvider,
};
use crate::sql::schema::{get_table_info, TABLES};
use regex::Regex;
use rusqlite::{Connection, Row};
use serde_json::{Map, Value};
use std::collections::HashSet;

/// The SQL query engine that executes queries against Git repository data.
///
/// `SqlEngine` uses an in-memory SQLite database to store Git data and execute
/// SQL queries. Tables are lazily loaded based on query requirements.
///
/// # Example
///
/// ```no_run
/// use vcsql::{GitRepo, SqlEngine};
///
/// let mut repo = GitRepo::open(".")?;
/// let mut engine = SqlEngine::new()?;
///
/// // Load only tables referenced in the query
/// engine.load_tables_for_query("SELECT * FROM commits LIMIT 10", &mut repo)?;
///
/// // Execute the query
/// let result = engine.execute("SELECT * FROM commits LIMIT 10")?;
/// println!("Columns: {:?}", result.columns);
/// # Ok::<(), vcsql::VcsqlError>(())
/// ```
pub struct SqlEngine {
    conn: Connection,
    loaded_tables: HashSet<String>,
}

impl SqlEngine {
    /// Creates a new SQL engine with an empty in-memory database.
    pub fn new() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        Ok(Self {
            conn,
            loaded_tables: HashSet::new(),
        })
    }

    /// Extracts table names referenced in a SQL query.
    ///
    /// Parses the query for FROM, JOIN, INTO, and UPDATE clauses to identify
    /// which tables need to be loaded.
    pub fn extract_table_names(query: &str) -> HashSet<String> {
        let mut tables = HashSet::new();

        let table_names: Vec<&str> = TABLES.iter().map(|t| t.name).collect();

        let pattern = r"(?i)\b(FROM|JOIN|INTO|UPDATE)\s+(\w+)";
        let re = Regex::new(pattern).unwrap();

        for cap in re.captures_iter(query) {
            if let Some(table_match) = cap.get(2) {
                let table_name = table_match.as_str().to_lowercase();
                if table_names.contains(&table_name.as_str()) {
                    tables.insert(table_name);
                }
            }
        }

        // Also check for table aliases like "commits c"
        for table in &table_names {
            let pattern = format!(r"(?i)\b{}\b", regex::escape(table));
            if Regex::new(&pattern).unwrap().is_match(query) {
                tables.insert(table.to_string());
            }
        }

        tables
    }

    /// Loads a single table's data from the repository into the database.
    ///
    /// Tables are cached after first load - subsequent calls for the same table are no-ops.
    pub fn load_table(&mut self, table_name: &str, repo: &mut GitRepo) -> Result<()> {
        if self.loaded_tables.contains(table_name) {
            return Ok(());
        }

        let table_info = get_table_info(table_name)
            .ok_or_else(|| VcsqlError::TableNotFound(table_name.to_string()))?;

        self.conn.execute(table_info.create_sql, [])?;

        let provider: Box<dyn Provider> = match table_name {
            "commits" => Box::new(CommitsProvider),
            "commit_parents" => Box::new(CommitParentsProvider),
            "branches" => Box::new(BranchesProvider),
            "tags" => Box::new(TagsProvider),
            "refs" => Box::new(RefsProvider),
            "stashes" => Box::new(StashesProvider),
            "reflog" => Box::new(ReflogProvider),
            "diffs" => Box::new(DiffsProvider),
            "diff_files" => Box::new(DiffFilesProvider),
            "blame" => Box::new(BlameProvider::new(None)),
            "config" => Box::new(ConfigProvider),
            "remotes" => Box::new(RemotesProvider),
            "submodules" => Box::new(SubmodulesProvider),
            "status" => Box::new(StatusProvider),
            "worktrees" => Box::new(WorktreesProvider),
            "hooks" => Box::new(HooksProvider),
            "notes" => Box::new(NotesProvider),
            _ => return Err(VcsqlError::TableNotFound(table_name.to_string())),
        };

        provider.populate(&self.conn, repo)?;
        self.loaded_tables.insert(table_name.to_string());

        Ok(())
    }

    /// Loads all tables referenced in a query from the repository.
    ///
    /// Analyzes the query to determine which tables are needed, then loads each one.
    pub fn load_tables_for_query(&mut self, query: &str, repo: &mut GitRepo) -> Result<()> {
        let tables = Self::extract_table_names(query);
        for table in tables {
            self.load_table(&table, repo)?;
        }
        Ok(())
    }

    /// Executes a SQL query and returns the results.
    ///
    /// The query can use any SQL features supported by SQLite, including JOINs,
    /// CTEs, window functions, and aggregations.
    pub fn execute(&self, query: &str) -> Result<QueryResult> {
        let mut stmt = self.conn.prepare(query)?;

        let column_names: Vec<String> = stmt.column_names().iter().map(|s| s.to_string()).collect();

        let rows: Vec<Vec<Value>> = stmt
            .query_map([], |row| Ok(row_to_values(row, column_names.len())))?
            .filter_map(|r| r.ok())
            .collect();

        Ok(QueryResult {
            columns: column_names,
            rows,
        })
    }
}

fn row_to_values(row: &Row, col_count: usize) -> Vec<Value> {
    (0..col_count)
        .map(|i| {
            if let Ok(v) = row.get::<_, Option<i64>>(i) {
                match v {
                    Some(n) => Value::Number(n.into()),
                    None => Value::Null,
                }
            } else if let Ok(v) = row.get::<_, Option<f64>>(i) {
                match v {
                    Some(n) => {
                        if let Some(num) = serde_json::Number::from_f64(n) {
                            Value::Number(num)
                        } else {
                            Value::String(n.to_string())
                        }
                    }
                    None => Value::Null,
                }
            } else if let Ok(v) = row.get::<_, Option<String>>(i) {
                match v {
                    Some(s) => Value::String(s),
                    None => Value::Null,
                }
            } else {
                Value::Null
            }
        })
        .collect()
}

/// The result of a SQL query execution.
#[derive(Debug)]
pub struct QueryResult {
    /// Column names from the query.
    pub columns: Vec<String>,
    /// Row data as JSON values.
    pub rows: Vec<Vec<Value>>,
}

impl QueryResult {
    /// Returns true if the result contains no rows.
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// Returns the number of rows in the result.
    pub fn row_count(&self) -> usize {
        self.rows.len()
    }

    /// Converts the result to a JSON array of objects.
    ///
    /// Each row becomes a JSON object with column names as keys.
    pub fn to_json_array(&self) -> Vec<Value> {
        self.rows
            .iter()
            .map(|row| {
                let mut obj = Map::new();
                for (i, col) in self.columns.iter().enumerate() {
                    obj.insert(col.clone(), row.get(i).cloned().unwrap_or(Value::Null));
                }
                Value::Object(obj)
            })
            .collect()
    }
}
