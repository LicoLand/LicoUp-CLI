use anyhow::{Context, Result};
use rusqlite::Connection;

pub(super) fn reclaim_cache_space(connection: &Connection) -> Result<()> {
    connection.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
    let page_count =
        connection.pragma_query_value(None, "page_count", |row| row.get::<_, u64>(0))?;
    let free_pages =
        connection.pragma_query_value(None, "freelist_count", |row| row.get::<_, u64>(0))?;
    if page_count > 0 && free_pages.saturating_mul(4) >= page_count {
        // A bulk first-time rollup can free most pages. Routine daily cleanup
        // uses incremental vacuum and never rewrites the whole database.
        connection.execute_batch("VACUUM;")?;
    } else if free_pages > 0 {
        connection.execute_batch("PRAGMA incremental_vacuum(256);")?;
    }
    connection
        .execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
        .context("agent usage cache compaction failed")?;
    Ok(())
}
