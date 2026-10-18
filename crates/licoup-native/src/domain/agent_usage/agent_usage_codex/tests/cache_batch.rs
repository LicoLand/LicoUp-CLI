use super::super::cache::open_cache_database;
use super::super::cache_batch::CacheBatch;
use super::super::file_collection::FileMetadata;
use super::super::models::ParserState;
use super::support::temp_dir;
use licoup_statistics::contract::UsageVariant;

#[test]
fn cache_batch_round_trips_and_deletes_file_state() {
    let directory = temp_dir("codex-cache-batch");
    let database_path = directory.join("usage.sqlite3");
    let mut connection = open_cache_database(&database_path).unwrap();
    let transaction = connection.transaction().unwrap();
    {
        let mut batch = CacheBatch::new(&transaction).unwrap();
        let metadata = FileMetadata {
            modified_ns: 41,
            size: 43,
            file_id: Some("synthetic-file".to_string()),
        };
        let state = ParserState {
            session_id: Some("synthetic-session".to_string()),
            current_model: Some("synthetic-model".to_string()),
            current_variant: UsageVariant {
                effort: Some("high".to_owned()),
                fast: Some(false),
            },
            pending_context: true,
            ..ParserState::default()
        };
        batch
            .save("root", "source", &metadata, 37, "guard", &state)
            .unwrap();

        let cached = batch.load("root", "source").unwrap().unwrap();
        assert_eq!(cached.modified_ns, 41);
        assert_eq!(cached.size, 43);
        assert_eq!(cached.parsed_bytes, 37);
        assert_eq!(cached.state.current_variant, state.current_variant);
        assert!(cached.state.pending_context);
        assert_eq!(
            cached.state.session_id.as_deref(),
            Some("synthetic-session")
        );

        batch.delete_source("root", "source").unwrap();
        assert!(batch.load("root", "source").unwrap().is_none());
    }
    transaction.commit().unwrap();
}
