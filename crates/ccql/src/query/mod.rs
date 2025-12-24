use crate::error::Result;
use serde_json::Value;

pub struct QueryEngine;

impl QueryEngine {
    pub fn new() -> Self {
        Self
    }

    pub fn execute(&self, query: &str, input: Value) -> Result<Vec<Value>> {
        // Parse the jq-like query and execute it
        let query = query.trim();

        if query == "." {
            return Ok(vec![input]);
        }

        if query.starts_with(".[]") {
            // Iterate over array
            let rest = query.strip_prefix(".[]").unwrap_or("");
            return match input {
                Value::Array(arr) => {
                    if rest.is_empty() {
                        Ok(arr)
                    } else {
                        let engine = QueryEngine::new();
                        let mut results = Vec::new();
                        for item in arr {
                            results.extend(engine.execute(rest, item)?);
                        }
                        Ok(results)
                    }
                }
                _ => Ok(vec![]),
            };
        }

        if query.starts_with(".[") {
            // Array index
            if let Some(end) = query.find(']') {
                let idx_str = &query[2..end];
                if let Ok(idx) = idx_str.parse::<usize>() {
                    let rest = &query[end + 1..];
                    return match &input {
                        Value::Array(arr) => {
                            if let Some(item) = arr.get(idx) {
                                if rest.is_empty() {
                                    Ok(vec![item.clone()])
                                } else {
                                    self.execute(rest, item.clone())
                                }
                            } else {
                                Ok(vec![])
                            }
                        }
                        _ => Ok(vec![]),
                    };
                }
            }
        }

        if let Some(field_query) = query.strip_prefix('.') {
            // Field access
            let (field, rest) = if let Some(pos) = field_query.find(['.', '[', '|']) {
                (&field_query[..pos], &field_query[pos..])
            } else {
                (field_query, "")
            };

            return match &input {
                Value::Object(obj) => {
                    if let Some(value) = obj.get(field) {
                        if rest.is_empty() {
                            Ok(vec![value.clone()])
                        } else {
                            self.execute(rest, value.clone())
                        }
                    } else {
                        Ok(vec![Value::Null])
                    }
                }
                _ => Ok(vec![Value::Null]),
            };
        }

        if query.starts_with("select(") {
            // select filter
            if let Some(end) = query.rfind(')') {
                let condition = &query[7..end];
                if self.evaluate_condition(condition, &input)? {
                    let rest = &query[end + 1..].trim_start_matches([' ', '|']).trim();
                    if rest.is_empty() {
                        return Ok(vec![input]);
                    } else {
                        return self.execute(rest, input);
                    }
                } else {
                    return Ok(vec![]);
                }
            }
        }

        if query.contains('|') {
            // Pipeline
            let parts: Vec<&str> = query.splitn(2, '|').collect();
            if parts.len() == 2 {
                let first = parts[0].trim();
                let second = parts[1].trim();

                let intermediate = self.execute(first, input)?;
                let mut results = Vec::new();
                for item in intermediate {
                    results.extend(self.execute(second, item)?);
                }
                return Ok(results);
            }
        }

        // Return original for unrecognized queries
        Ok(vec![input])
    }

    fn evaluate_condition(&self, condition: &str, input: &Value) -> Result<bool> {
        let condition = condition.trim();

        // Handle .field == "value"
        if condition.contains("==") {
            let parts: Vec<&str> = condition.splitn(2, "==").collect();
            if parts.len() == 2 {
                let left = parts[0].trim();
                let right = parts[1].trim().trim_matches('"');

                let left_val = self.execute(left, input.clone())?;
                if let Some(val) = left_val.first() {
                    return Ok(val.as_str().map(|s| s == right).unwrap_or(false));
                }
            }
        }

        // Handle .field != "value"
        if condition.contains("!=") {
            let parts: Vec<&str> = condition.splitn(2, "!=").collect();
            if parts.len() == 2 {
                let left = parts[0].trim();
                let right = parts[1].trim().trim_matches('"');

                let left_val = self.execute(left, input.clone())?;
                if let Some(val) = left_val.first() {
                    return Ok(val.as_str().map(|s| s != right).unwrap_or(true));
                }
            }
        }

        // Handle .field | test("pattern")
        if condition.contains("test(") {
            if let Some(pipe_pos) = condition.find('|') {
                let field = condition[..pipe_pos].trim();
                let test_part = condition[pipe_pos + 1..].trim();

                if test_part.starts_with("test(") {
                    if let Some(end) = test_part.rfind(')') {
                        let pattern = test_part[5..end].trim().trim_matches('"');
                        let field_val = self.execute(field, input.clone())?;

                        if let Some(val) = field_val.first() {
                            if let Some(s) = val.as_str() {
                                return Ok(s.contains(pattern));
                            }
                        }
                    }
                }
            }
        }

        // Default to false for unknown conditions
        Ok(false)
    }

    pub fn execute_on_array(&self, query: &str, inputs: Vec<Value>) -> Result<Vec<Value>> {
        let array = Value::Array(inputs);
        self.execute(query, array)
    }

    pub fn execute_per_item(&self, query: &str, inputs: Vec<Value>) -> Result<Vec<Value>> {
        let mut results = Vec::new();
        for input in inputs {
            let item_results = self.execute(query, input)?;
            results.extend(item_results);
        }
        Ok(results)
    }
}

impl Default for QueryEngine {
    fn default() -> Self {
        Self::new()
    }
}

pub struct FilterBuilder;

impl FilterBuilder {
    pub fn select_type(message_type: &str) -> String {
        format!(r#"select(.type == "{}")"#, message_type)
    }

    pub fn select_field_contains(field: &str, value: &str) -> String {
        format!(r#"select(.{} | test("{}"))"#, field, value)
    }

    pub fn project_fields(fields: &[&str]) -> String {
        let field_list = fields
            .iter()
            .map(|f| format!("{}: .{}", f, f))
            .collect::<Vec<_>>()
            .join(", ");
        format!("{{ {} }}", field_list)
    }
}
