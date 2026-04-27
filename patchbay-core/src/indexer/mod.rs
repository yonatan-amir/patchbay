use uuid::Uuid;
use crate::db::{Database, DbError, PluginRecord};
use crate::scanner::ScannedPlugin;

/// Upsert every scanned plugin into the database.
/// Returns the number of plugins written.
pub fn index_plugins(
    db: &Database,
    plugins: Vec<ScannedPlugin>,
    device_id: &str,
) -> Result<usize, DbError> {
    let mut count = 0;
    for p in plugins {
        let record = PluginRecord {
            sync_id: Uuid::new_v4().to_string(),
            name: p.name,
            vendor: p.vendor,
            format: p.format.as_str().to_string(),
            path: p.path.to_string_lossy().into_owned(),
            version: p.version,
            class_id: p.class_id,
            category: p.category,
            device_id: device_id.to_string(),
            file_mtime: p.file_mtime,
        };
        db.upsert_plugin(&record)?;
        count += 1;
    }
    Ok(count)
}
