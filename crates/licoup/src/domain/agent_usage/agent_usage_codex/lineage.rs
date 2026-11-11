use anyhow::Result;
use rusqlite::{Transaction, params};
use std::collections::{BTreeSet, HashMap};

fn session_lineage_parents(
    transaction: &Transaction<'_>,
    root_key: &str,
) -> Result<HashMap<String, String>> {
    let mut statement = transaction.prepare(
        "SELECT session_id, forked_from_id
         FROM usage_files
         WHERE root_key=?1 AND session_id IS NOT NULL AND forked_from_id IS NOT NULL
         ORDER BY source_key",
    )?;
    let rows = statement.query_map([root_key], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    let mut candidates = HashMap::<String, BTreeSet<String>>::new();
    for row in rows {
        let (session_id, parent_id) = row?;
        if session_id.is_empty() || parent_id.is_empty() || session_id == parent_id {
            continue;
        }
        candidates.entry(session_id).or_default().insert(parent_id);
    }
    Ok(candidates
        .into_iter()
        .filter_map(|(session_id, parents)| {
            if parents.len() != 1 {
                return None;
            }
            parents
                .into_iter()
                .next()
                .map(|parent_id| (session_id, parent_id))
        })
        .collect())
}

pub(super) fn reconcile_lineage_scopes(
    transaction: &Transaction<'_>,
    root_key: &str,
) -> Result<()> {
    let parents = session_lineage_parents(transaction, root_key)?;
    let files = {
        let mut statement = transaction.prepare(
            "SELECT source_key, session_id FROM usage_files WHERE root_key=?1 ORDER BY source_key",
        )?;
        let rows = statement.query_map([root_key], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?))
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };
    let mut update_scope = transaction.prepare(
        "UPDATE usage_files
         SET lineage_scope=?3
         WHERE root_key=?1 AND source_key=?2 AND lineage_scope<>?3",
    )?;
    for (source_key, session_id) in files {
        let scope = lineage_scope(session_id.as_deref(), &source_key, &parents);
        update_scope.execute(params![root_key, source_key, scope])?;
    }
    Ok(())
}

pub(super) fn lineage_scope(
    session_id: Option<&str>,
    source_key: &str,
    parents: &HashMap<String, String>,
) -> String {
    let Some(session_id) = session_id.filter(|value| !value.is_empty()) else {
        return format!("source:{source_key}");
    };
    let mut current = session_id.to_string();
    let mut visited = BTreeSet::<String>::new();
    loop {
        if !visited.insert(current.clone()) {
            let root = visited
                .into_iter()
                .min()
                .unwrap_or_else(|| session_id.to_string());
            return format!("session:{root}");
        }
        let Some(parent) = parents.get(&current) else {
            return format!("session:{current}");
        };
        current.clone_from(parent);
    }
}
