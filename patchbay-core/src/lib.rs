pub mod audio;
pub mod daw_readers;
pub mod db;
pub mod indexer;
pub mod live_project;
pub mod scanner;
pub mod watcher;

pub fn new_sync_id() -> String {
    uuid::Uuid::new_v4().to_string()
}
