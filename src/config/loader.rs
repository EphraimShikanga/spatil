use std::collections::HashSet;
use std::path::{Path, PathBuf};

use toml::{Table, Value};

pub fn load_with_includes(path: &Path) -> Result<Table, String> {
    let mut visited = HashSet::new();
    load_file(path, &mut visited)
}

fn load_file(path: &Path, visited: &mut HashSet<PathBuf>) -> Result<Table, String> {
    let key = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    if !visited.insert(key) {
        tracing::warn!("config include cycle at {}; skipping", path.display());
        return Ok(Table::new());
    }

    let contents =
        std::fs::read_to_string(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let mut table: Table =
        toml::from_str(&contents).map_err(|e| format!("parse error in {}: {e}", path.display()))?;

    // The including file's own keys are the base; each include layers on top.
    for include in take_includes(&mut table, path) {
        if include.exists() {
            let included = load_file(&include, visited)?;
            merge_tables(&mut table, included);
        } else {
            tracing::warn!(
                "config include {} (from {}) not found; skipping",
                include.display(),
                path.display()
            );
        }
    }
    Ok(table)
}

/// Removes the `include` key and resolves its entries to paths, relative to the
/// including file's directory. Accepts a single string or an array of strings.
fn take_includes(table: &mut Table, base: &Path) -> Vec<PathBuf> {
    let Some(value) = table.remove("include") else {
        return Vec::new();
    };
    let base_dir = base.parent().unwrap_or_else(|| Path::new("."));
    let entries = match value {
        Value::String(s) => vec![s],
        Value::Array(arr) => arr.into_iter().filter_map(|v| v.as_str().map(str::to_owned)).collect(),
        other => {
            tracing::warn!("`include` must be a string or array of strings, got {other:?}");
            Vec::new()
        }
    };
    entries
        .into_iter()
        .map(|e| {
            let p = PathBuf::from(e);
            if p.is_absolute() { p } else { base_dir.join(p) }
        })
        .collect()
}

fn merge_tables(base: &mut Table, over: Table) {
    for (key, over_value) in over {
        match base.get_mut(&key) {
            Some(base_value) => merge_value(base_value, over_value),
            None => {
                base.insert(key, over_value);
            }
        }
    }
}

fn merge_value(base: &mut Value, over: Value) {
    match (base, over) {
        // Two tables merge recursively; anything else, the override replaces.
        (Value::Table(b), Value::Table(o)) => merge_tables(b, o),
        (base, over) => *base = over,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn later_scalar_wins_and_tables_merge() {
        let mut base: Table = toml::from_str("a = 1\n[t]\nx = 1\ny = 1\n").unwrap();
        let over: Table = toml::from_str("a = 2\n[t]\ny = 9\nz = 9\n").unwrap();
        merge_tables(&mut base, over);
        assert_eq!(base["a"].as_integer(), Some(2));
        let t = base["t"].as_table().unwrap();
        assert_eq!(t["x"].as_integer(), Some(1));
        assert_eq!(t["y"].as_integer(), Some(9));
        assert_eq!(t["z"].as_integer(), Some(9));
    }

    #[test]
    fn arrays_are_replaced_not_concatenated() {
        let mut base: Table = toml::from_str("v = [1, 2]\n").unwrap();
        let over: Table = toml::from_str("v = [3]\n").unwrap();
        merge_tables(&mut base, over);
        assert_eq!(base["v"].as_array().unwrap().len(), 1);
    }
}
