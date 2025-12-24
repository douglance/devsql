use std::collections::HashMap;
use strsim::normalized_levenshtein;

#[derive(Debug, Clone)]
pub struct PromptCluster {
    pub canonical: String,
    pub variants: Vec<String>,
    pub count: usize,
    pub latest_timestamp: i64,
}

pub struct FuzzyDeduper {
    threshold: f64,
    min_length: usize,
}

impl FuzzyDeduper {
    pub fn new(threshold: f64, min_length: usize) -> Self {
        Self { threshold, min_length }
    }

    /// Cluster similar prompts together using fuzzy matching
    /// Takes (prompt, timestamp) pairs
    pub fn cluster(&self, prompts: Vec<(String, i64)>) -> Vec<PromptCluster> {
        // Count occurrences and track latest timestamp
        let mut counts: HashMap<String, (usize, i64)> = HashMap::new();
        for (prompt, timestamp) in &prompts {
            let normalized = self.normalize(prompt);
            if !normalized.is_empty() && normalized.len() >= self.min_length {
                let entry = counts.entry(normalized).or_insert((0, 0));
                entry.0 += 1;
                if *timestamp > entry.1 {
                    entry.1 = *timestamp;
                }
            }
        }

        // Sort by count descending for clustering
        let mut items: Vec<(String, usize, i64)> = counts
            .into_iter()
            .map(|(k, (count, ts))| (k, count, ts))
            .collect();
        items.sort_by(|a, b| b.1.cmp(&a.1));

        // Cluster similar items
        let mut clusters: Vec<PromptCluster> = Vec::new();

        for (prompt, count, timestamp) in items {
            let mut found_cluster = false;

            for cluster in &mut clusters {
                if self.is_similar(&prompt, &cluster.canonical) {
                    cluster.variants.push(prompt.clone());
                    cluster.count += count;
                    if timestamp > cluster.latest_timestamp {
                        cluster.latest_timestamp = timestamp;
                    }
                    found_cluster = true;
                    break;
                }
            }

            if !found_cluster {
                clusters.push(PromptCluster {
                    canonical: prompt.clone(),
                    variants: vec![prompt],
                    count,
                    latest_timestamp: timestamp,
                });
            }
        }

        clusters
    }

    /// Sort clusters by count (default)
    pub fn sort_by_count(clusters: &mut [PromptCluster]) {
        clusters.sort_by(|a, b| b.count.cmp(&a.count));
    }

    /// Sort clusters by latest timestamp
    pub fn sort_by_latest(clusters: &mut [PromptCluster]) {
        clusters.sort_by(|a, b| b.latest_timestamp.cmp(&a.latest_timestamp));
    }

    fn normalize(&self, s: &str) -> String {
        let s = s.trim().to_lowercase();

        // Filter out code-like content
        if s.contains("import ")
            || s.contains("export ")
            || s.contains("const ")
            || s.contains("function ")
            || s.contains("interface ")
            || s.starts_with("//")
            || s.starts_with("/*")
            || s.starts_with("```")
            || s.contains(".js:")
            || s.contains(".ts:")
            || s.contains(".tsx:")
            || s.contains("chunk-")
            || s.contains("requestanimationframe")
            || s.contains("installhook")
            || s.starts_with('[')
            || s.starts_with('{')
            || s.starts_with('<')
        {
            return String::new();
        }

        s
    }

    fn is_similar(&self, a: &str, b: &str) -> bool {
        if a == b {
            return true;
        }

        let len_ratio = a.len().min(b.len()) as f64 / a.len().max(b.len()) as f64;
        if len_ratio < 0.5 {
            return false;
        }

        let similarity = normalized_levenshtein(a, b);
        similarity >= self.threshold
    }
}

impl Default for FuzzyDeduper {
    fn default() -> Self {
        Self::new(0.8, 4)
    }
}
