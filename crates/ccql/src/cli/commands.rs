use crate::cli::output::{create_table, truncate_string, OutputFormat, OutputWriter};
use crate::config::Config;
use crate::datasources::{HistoryDataSource, StatsDataSource, TodoDataSource, TranscriptDataSource};
use crate::error::Result;
use crate::models::TodoStatus;
use crate::query::QueryEngine;
use crate::search::SearchEngine;
use crate::sql::{SqlEngine, SqlOptions};
use comfy_table::Cell;

pub async fn prompts(
    config: &Config,
    session: Option<String>,
    project: Option<String>,
    since: Option<String>,
    until: Option<String>,
    limit: Option<usize>,
    format: OutputFormat,
) -> Result<()> {
    let history = HistoryDataSource::new(config.clone());
    let mut entries = history.filter_prompts().await?;

    if let Some(ref proj) = project {
        entries.retain(|e| e.project.as_ref().map(|p| p.contains(proj)).unwrap_or(false));
    }

    if let Some(ref sess) = session {
        entries.retain(|e| e.session_id.as_ref().map(|s| s.contains(sess)).unwrap_or(false));
    }

    if let Some(ref since_str) = since {
        if let Ok(since_date) = chrono::NaiveDate::parse_from_str(since_str, "%Y-%m-%d") {
            let since_ts = since_date.and_hms_opt(0, 0, 0).unwrap().and_utc().timestamp_millis();
            entries.retain(|e| e.timestamp >= since_ts);
        }
    }

    if let Some(ref until_str) = until {
        if let Ok(until_date) = chrono::NaiveDate::parse_from_str(until_str, "%Y-%m-%d") {
            let until_ts = until_date.and_hms_opt(23, 59, 59).unwrap().and_utc().timestamp_millis();
            entries.retain(|e| e.timestamp <= until_ts);
        }
    }

    entries.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));

    if let Some(limit) = limit {
        entries.truncate(limit);
    }

    let mut writer = OutputWriter::new(std::io::stdout(), format);

    match format {
        OutputFormat::Json => {
            writer.write_json(&entries)?;
        }
        OutputFormat::Raw | OutputFormat::Jsonl => {
            for entry in &entries {
                writer.write_json(entry)?;
            }
        }
        OutputFormat::Table => {
            let mut table = create_table();
            table.set_header(vec!["Time", "Project", "Prompt"]);

            for entry in &entries {
                table.add_row(vec![
                    Cell::new(entry.formatted_time()),
                    Cell::new(entry.project_name().unwrap_or("-")),
                    Cell::new(truncate_string(&entry.display, 80)),
                ]);
            }

            writer.write_table(table)?;
            writer.writeln(&format!("\nTotal: {} prompts", entries.len()))?;
        }
    }

    Ok(())
}

pub async fn query(
    config: &Config,
    query_str: &str,
    source: &str,
    file_pattern: Option<String>,
    format: OutputFormat,
) -> Result<()> {
    let engine = QueryEngine::new();

    let data = match source {
        "history" => {
            let ds = HistoryDataSource::new(config.clone());
            ds.load_raw().await?
        }
        "transcripts" => {
            let ds = TranscriptDataSource::new(config.clone());
            let sessions = ds.load_all_sessions().await?;
            let mut all = Vec::new();
            for (session_id, entries) in sessions {
                if let Some(ref pattern) = file_pattern {
                    if !session_id.contains(pattern) {
                        continue;
                    }
                }
                all.extend(entries);
            }
            all
        }
        "stats" => {
            let ds = StatsDataSource::new(config.clone());
            vec![ds.load_raw().await?]
        }
        "todos" => {
            let ds = TodoDataSource::new(config.clone());
            let files = ds.load_all().await?;
            files
                .into_iter()
                .map(|f| {
                    serde_json::json!({
                        "workspace_id": f.workspace_id,
                        "agent_id": f.agent_id,
                        "todos": f.todos
                    })
                })
                .collect()
        }
        _ => {
            return Err(crate::error::Error::DataSource(format!(
                "Unknown source: {}. Use: history, transcripts, stats, todos",
                source
            )));
        }
    };

    let results = engine.execute_on_array(query_str, data)?;

    let mut writer = OutputWriter::new(std::io::stdout(), format);

    match format {
        OutputFormat::Json => {
            writer.write_json(&results)?;
        }
        OutputFormat::Raw | OutputFormat::Jsonl => {
            for result in &results {
                writer.write_json(result)?;
            }
        }
        OutputFormat::Table => {
            for result in &results {
                let json = serde_json::to_string_pretty(result)?;
                writer.writeln(&json)?;
            }
        }
    }

    Ok(())
}

pub async fn sessions(
    config: &Config,
    _detailed: bool,
    _project: Option<String>,
    sort_by: &str,
    format: OutputFormat,
) -> Result<()> {
    let ds = TranscriptDataSource::new(config.clone());
    let mut sessions = ds.list_sessions()?;

    match sort_by {
        "time" => sessions.sort_by(|a, b| b.modified.cmp(&a.modified)),
        "size" => sessions.sort_by(|a, b| b.size_bytes.cmp(&a.size_bytes)),
        _ => {}
    }

    let mut writer = OutputWriter::new(std::io::stdout(), format);

    match format {
        OutputFormat::Json => {
            let data: Vec<_> = sessions
                .iter()
                .map(|s| {
                    serde_json::json!({
                        "session_id": s.session_id,
                        "size": s.size_bytes,
                        "modified": s.formatted_time()
                    })
                })
                .collect();
            writer.write_json(&data)?;
        }
        OutputFormat::Raw | OutputFormat::Jsonl => {
            for session in &sessions {
                writer.write_json(&serde_json::json!({
                    "session_id": session.session_id,
                    "size": session.size_bytes,
                    "modified": session.formatted_time()
                }))?;
            }
        }
        OutputFormat::Table => {
            let mut table = create_table();
            table.set_header(vec!["Session ID", "Size", "Modified"]);

            for session in &sessions {
                table.add_row(vec![
                    Cell::new(&session.session_id),
                    Cell::new(session.size_human()),
                    Cell::new(session.formatted_time()),
                ]);
            }

            writer.write_table(table)?;
            writer.writeln(&format!("\nTotal: {} sessions", sessions.len()))?;
        }
    }

    Ok(())
}

pub async fn stats(
    config: &Config,
    group_by: &str,
    _since: Option<String>,
    _until: Option<String>,
    format: OutputFormat,
) -> Result<()> {
    let ds = StatsDataSource::new(config.clone());
    let stats = ds.load().await?;

    let mut writer = OutputWriter::new(std::io::stdout(), format);

    match format {
        OutputFormat::Json => {
            writer.write_json(&stats)?;
        }
        OutputFormat::Raw | OutputFormat::Jsonl => {
            writer.write_json(&stats)?;
        }
        OutputFormat::Table => {
            writer.writeln("=== Claude Code Usage Statistics ===\n")?;

            writer.writeln(&format!("Total Messages: {}", stats.total_messages))?;
            writer.writeln(&format!("Total Sessions: {}", stats.total_sessions))?;
            writer.writeln(&format!("Total Tokens: {}", stats.total_tokens()))?;
            writer.writeln(&format!("First Session: {}", stats.first_session_date))?;
            writer.writeln(&format!("Last Computed: {}", stats.last_computed_date))?;

            if let Some(ref longest) = stats.longest_session {
                writer.writeln(&format!(
                    "\nLongest Session: {} ({} messages)",
                    longest.session_id, longest.message_count
                ))?;
            }

            writer.writeln("\n--- Model Usage ---")?;
            let mut model_table = create_table();
            model_table.set_header(vec!["Model", "Input Tokens", "Output Tokens"]);

            for (model, usage) in &stats.model_usage {
                model_table.add_row(vec![
                    Cell::new(model),
                    Cell::new(usage.input_tokens),
                    Cell::new(usage.output_tokens),
                ]);
            }
            writer.write_table(model_table)?;

            if group_by == "date" {
                writer.writeln("\n--- Daily Activity (last 10 days) ---")?;
                let mut daily_table = create_table();
                daily_table.set_header(vec!["Date", "Messages", "Sessions", "Tool Calls"]);

                for activity in stats.daily_activity.iter().rev().take(10) {
                    daily_table.add_row(vec![
                        Cell::new(&activity.date),
                        Cell::new(activity.message_count),
                        Cell::new(activity.session_count),
                        Cell::new(activity.tool_call_count),
                    ]);
                }
                writer.write_table(daily_table)?;
            }
        }
    }

    Ok(())
}

pub async fn search(
    config: &Config,
    term: &str,
    scope: &str,
    case_sensitive: bool,
    is_regex: bool,
    _before_context: usize,
    _after_context: usize,
    format: OutputFormat,
) -> Result<()> {
    let engine = SearchEngine::new(term, case_sensitive, is_regex)?;

    let mut results = Vec::new();

    // Search in history
    if scope == "all" || scope == "prompts" {
        let history = HistoryDataSource::new(config.clone());
        let entries = history.load_all().await?;

        for entry in entries {
            if engine.matches(&entry.display) {
                results.push(serde_json::json!({
                    "source": "history",
                    "timestamp": entry.timestamp,
                    "project": entry.project,
                    "content": entry.display
                }));
            }
        }
    }

    // Search in transcripts
    if scope == "all" || scope == "transcripts" {
        let transcripts = TranscriptDataSource::new(config.clone());
        let sessions = transcripts.load_all_sessions().await?;

        for (session_id, entries) in sessions {
            for (idx, entry) in entries.iter().enumerate() {
                if engine.find_in_json(entry) {
                    results.push(serde_json::json!({
                        "source": "transcript",
                        "session_id": session_id,
                        "entry_index": idx,
                        "content": entry
                    }));
                }
            }
        }
    }

    let mut writer = OutputWriter::new(std::io::stdout(), format);

    match format {
        OutputFormat::Json => {
            writer.write_json(&results)?;
        }
        OutputFormat::Raw | OutputFormat::Jsonl => {
            for result in &results {
                writer.write_json(result)?;
            }
        }
        OutputFormat::Table => {
            let mut table = create_table();
            table.set_header(vec!["Source", "Location", "Match"]);

            for result in &results {
                let source = result["source"].as_str().unwrap_or("-");
                let location = if source == "history" {
                    result["project"]
                        .as_str()
                        .unwrap_or("-")
                        .to_string()
                } else {
                    format!(
                        "{}:{}",
                        result["session_id"].as_str().unwrap_or("-"),
                        result["entry_index"].as_u64().unwrap_or(0)
                    )
                };
                let content = if source == "history" {
                    result["content"].as_str().unwrap_or("").to_string()
                } else {
                    truncate_string(&result["content"].to_string(), 60)
                };

                table.add_row(vec![
                    Cell::new(source),
                    Cell::new(&location),
                    Cell::new(truncate_string(&content, 60)),
                ]);
            }

            writer.write_table(table)?;
            writer.writeln(&format!("\nFound: {} matches", results.len()))?;
        }
    }

    Ok(())
}

pub async fn todos(
    config: &Config,
    status: Option<TodoStatus>,
    agent: Option<String>,
    format: OutputFormat,
) -> Result<()> {
    let ds = TodoDataSource::new(config.clone());
    let mut files = ds.load_all().await?;

    if let Some(ref agent_filter) = agent {
        files.retain(|f| f.agent_id.contains(agent_filter));
    }

    let mut writer = OutputWriter::new(std::io::stdout(), format);

    match format {
        OutputFormat::Json => {
            let data: Vec<_> = files
                .iter()
                .map(|f| {
                    serde_json::json!({
                        "workspace_id": f.workspace_id,
                        "agent_id": f.agent_id,
                        "todos": f.todos
                    })
                })
                .collect();
            writer.write_json(&data)?;
        }
        OutputFormat::Raw | OutputFormat::Jsonl => {
            for file in &files {
                writer.write_json(&serde_json::json!({
                    "workspace_id": file.workspace_id,
                    "agent_id": file.agent_id,
                    "todos": file.todos
                }))?;
            }
        }
        OutputFormat::Table => {
            let mut table = create_table();
            table.set_header(vec!["Agent", "Status", "Task"]);

            let mut total = 0;
            for file in &files {
                for todo in &file.todos {
                    if let Some(ref status_filter) = status {
                        if &todo.status != status_filter {
                            continue;
                        }
                    }
                    total += 1;
                    table.add_row(vec![
                        Cell::new(truncate_string(&file.agent_id, 12)),
                        Cell::new(todo.status.to_string()),
                        Cell::new(truncate_string(&todo.content, 60)),
                    ]);
                }
            }

            writer.write_table(table)?;
            writer.writeln(&format!("\nTotal: {} todos", total))?;
        }
    }

    Ok(())
}

pub async fn duplicates(
    config: &Config,
    threshold: f64,
    min_count: usize,
    limit: usize,
    show_variants: bool,
    sort: &str,
    min_length: usize,
    format: OutputFormat,
) -> Result<()> {
    use crate::dedup::FuzzyDeduper;

    let history = HistoryDataSource::new(config.clone());
    let entries = history.filter_prompts().await?;

    // Include timestamps for sorting
    let prompts: Vec<(String, i64)> = entries
        .iter()
        .map(|e| (e.display.clone(), e.timestamp))
        .collect();

    let deduper = FuzzyDeduper::new(threshold, min_length);
    let mut clusters = deduper.cluster(prompts);

    // Sort based on user preference
    match sort {
        "latest" => FuzzyDeduper::sort_by_latest(&mut clusters),
        _ => FuzzyDeduper::sort_by_count(&mut clusters),
    }

    let filtered: Vec<_> = clusters
        .into_iter()
        .filter(|c| c.count >= min_count)
        .take(limit)
        .collect();

    let mut writer = OutputWriter::new(std::io::stdout(), format);

    match format {
        OutputFormat::Json => {
            let data: Vec<_> = filtered
                .iter()
                .map(|c| {
                    serde_json::json!({
                        "prompt": c.canonical,
                        "count": c.count,
                        "latest": c.latest_timestamp,
                        "variants": c.variants
                    })
                })
                .collect();
            writer.write_json(&data)?;
        }
        OutputFormat::Raw | OutputFormat::Jsonl => {
            for cluster in &filtered {
                writer.write_json(&serde_json::json!({
                    "prompt": cluster.canonical,
                    "count": cluster.count,
                    "latest": cluster.latest_timestamp,
                    "variants": cluster.variants
                }))?;
            }
        }
        OutputFormat::Table => {
            let mut table = create_table();
            if sort == "latest" {
                if show_variants {
                    table.set_header(vec!["Last Used", "Count", "Prompt", "Variants"]);
                } else {
                    table.set_header(vec!["Last Used", "Count", "Prompt"]);
                }
            } else if show_variants {
                table.set_header(vec!["Count", "Prompt", "Variants"]);
            } else {
                table.set_header(vec!["Count", "Prompt"]);
            }

            for cluster in &filtered {
                let time_str = chrono::DateTime::from_timestamp_millis(cluster.latest_timestamp)
                    .map(|dt| dt.format("%Y-%m-%d %H:%M").to_string())
                    .unwrap_or_else(|| "-".to_string());

                let variants_str = if cluster.variants.len() > 1 {
                    cluster
                        .variants
                        .iter()
                        .filter(|v| *v != &cluster.canonical)
                        .take(3)
                        .cloned()
                        .collect::<Vec<_>>()
                        .join(", ")
                } else {
                    "-".to_string()
                };

                if sort == "latest" {
                    if show_variants {
                        table.add_row(vec![
                            Cell::new(&time_str),
                            Cell::new(cluster.count),
                            Cell::new(truncate_string(&cluster.canonical, 40)),
                            Cell::new(truncate_string(&variants_str, 30)),
                        ]);
                    } else {
                        table.add_row(vec![
                            Cell::new(&time_str),
                            Cell::new(cluster.count),
                            Cell::new(truncate_string(&cluster.canonical, 60)),
                        ]);
                    }
                } else if show_variants {
                    table.add_row(vec![
                        Cell::new(cluster.count),
                        Cell::new(truncate_string(&cluster.canonical, 50)),
                        Cell::new(truncate_string(&variants_str, 40)),
                    ]);
                } else {
                    table.add_row(vec![
                        Cell::new(cluster.count),
                        Cell::new(truncate_string(&cluster.canonical, 70)),
                    ]);
                }
            }

            writer.write_table(table)?;
            writer.writeln(&format!(
                "\nShowing {} clusters (min count: {}, min length: {} chars, threshold: {:.0}%, sort: {})",
                filtered.len(),
                min_count,
                min_length,
                threshold * 100.0,
                sort
            ))?;
        }
    }

    Ok(())
}

pub async fn sql(
    config: &Config,
    query_str: &str,
    write_enabled: bool,
    dry_run: bool,
    format: OutputFormat,
) -> Result<()> {
    let options = SqlOptions {
        write_enabled,
        dry_run,
    };

    let mut engine = SqlEngine::new(config.clone(), options)?;

    if dry_run && crate::sql::is_write_operation_public(query_str) {
        let mut writer = OutputWriter::new(std::io::stdout(), format);
        writer.writeln("[DRY RUN] Would execute:")?;
        writer.writeln(query_str)?;
        writer.writeln("\nNo changes made. Remove --dry-run to execute.")?;
        return Ok(());
    }

    let results = engine.execute(query_str).await?;

    let mut writer = OutputWriter::new(std::io::stdout(), format);

    match format {
        OutputFormat::Json => {
            writer.write_json(&results)?;
        }
        OutputFormat::Raw | OutputFormat::Jsonl => {
            for result in &results {
                writer.write_json(result)?;
            }
        }
        OutputFormat::Table => {
            if results.is_empty() {
                writer.writeln("No results.")?;
            } else {
                // Try to build a table from the first result's keys
                if let Some(first) = results.first() {
                    if let Some(obj) = first.as_object() {
                        let headers: Vec<&str> = obj.keys().map(|s| s.as_str()).collect();
                        let mut table = create_table();
                        table.set_header(headers.clone());

                        for result in &results {
                            if let Some(obj) = result.as_object() {
                                let row: Vec<Cell> = headers
                                    .iter()
                                    .map(|h| {
                                        let val = obj.get(*h).unwrap_or(&serde_json::Value::Null);
                                        let display = match val {
                                            serde_json::Value::String(s) => truncate_string(s, 50),
                                            serde_json::Value::Null => "-".to_string(),
                                            _ => truncate_string(&val.to_string(), 50),
                                        };
                                        Cell::new(display)
                                    })
                                    .collect();
                                table.add_row(row);
                            }
                        }

                        writer.write_table(table)?;
                        writer.writeln(&format!("\n{} row(s)", results.len()))?;
                    } else {
                        // Not an object, just print as JSON
                        for result in &results {
                            let json = serde_json::to_string_pretty(result)?;
                            writer.writeln(&json)?;
                        }
                    }
                }
            }
        }
    }

    Ok(())
}
