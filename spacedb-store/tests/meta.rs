//! The `_meta` refuse-or-migrate gate, over both engines.

use spacedb_store::{
    open_meta, open_meta_with, read_store_version, write_store_version, Durability, KvEngine,
    MemEngine, MetaStatus, Migration, RedbEngine, StoreError, Table, WriteTx,
};

// ─── scenarios ───────────────────────────────────────────────────────────────

fn fresh_store_initializes_to_current(e: &impl KvEngine) {
    assert_eq!(read_store_version(e).unwrap(), None);
    assert_eq!(open_meta_with(e, 3, &[]).unwrap(), MetaStatus::Initialized);
    assert_eq!(read_store_version(e).unwrap(), Some(3));
}

fn reopen_at_same_version_is_current(e: &impl KvEngine) {
    open_meta_with(e, 3, &[]).unwrap();
    assert_eq!(open_meta_with(e, 3, &[]).unwrap(), MetaStatus::Current);
}

fn refuses_a_newer_format(e: &impl KvEngine) {
    write_store_version(e, 5).unwrap();
    let err = open_meta_with(e, 2, &[]).unwrap_err();
    assert!(matches!(
        err,
        StoreError::SchemaTooNew { found: 5, supported: 2 }
    ));
}

fn missing_migration_is_an_error(e: &impl KvEngine) {
    write_store_version(e, 1).unwrap();
    let err = open_meta_with(e, 3, &[]).unwrap_err();
    assert!(matches!(err, StoreError::Schema(_)));
    // version must NOT advance when a migration is missing
    assert_eq!(read_store_version(e).unwrap(), Some(1));
}

/// A migration step from v1 -> v2 that writes an observable marker, used to prove
/// the ladder actually runs.
struct AddMarker;
impl<E: KvEngine> Migration<E> for AddMarker {
    fn from_version(&self) -> u32 {
        1
    }
    fn apply(&self, engine: &E) -> Result<(), StoreError> {
        let mut w = engine.begin_write(Durability::Immediate)?;
        let marker: Table<String, String> = Table::new("migration_marker");
        marker.put(&mut w, &"ran".to_string(), &"v2".to_string())?;
        w.commit()
    }
}

fn migrates_an_older_store(e: &impl KvEngine) {
    write_store_version(e, 1).unwrap();
    let step = AddMarker;
    let migrations: [&dyn Migration<_>; 1] = [&step];

    let status = open_meta_with(e, 2, &migrations).unwrap();
    assert_eq!(status, MetaStatus::Migrated { from: 1 });
    assert_eq!(read_store_version(e).unwrap(), Some(2));

    // the migration's side effect is visible
    let marker: Table<String, String> = Table::new("migration_marker");
    let r = e.begin_read().unwrap();
    assert_eq!(marker.get(&r, &"ran".to_string()).unwrap(), Some("v2".to_string()));
}

fn open_meta_default_uses_store_format_version(e: &impl KvEngine) {
    // open_meta() targets STORE_FORMAT_VERSION; a fresh store initializes cleanly.
    assert_eq!(open_meta(e).unwrap(), MetaStatus::Initialized);
    assert_eq!(open_meta(e).unwrap(), MetaStatus::Current);
}

// ─── run every scenario against both engines ─────────────────────────────────

macro_rules! engine_suite {
    ($modname:ident, $make:expr, [$($scenario:ident),* $(,)?]) => {
        mod $modname {
            use super::*;
            $(
                #[test]
                fn $scenario() {
                    let (_holder, engine) = $make;
                    super::$scenario(&engine);
                }
            )*
        }
    };
}

engine_suite!(
    mem,
    ((), MemEngine::new()),
    [
        fresh_store_initializes_to_current,
        reopen_at_same_version_is_current,
        refuses_a_newer_format,
        missing_migration_is_an_error,
        migrates_an_older_store,
        open_meta_default_uses_store_format_version,
    ]
);

engine_suite!(
    redb,
    {
        let dir = tempfile::tempdir().unwrap();
        let engine = RedbEngine::open(dir.path().join("store.redb")).unwrap();
        (dir, engine)
    },
    [
        fresh_store_initializes_to_current,
        reopen_at_same_version_is_current,
        refuses_a_newer_format,
        missing_migration_is_an_error,
        migrates_an_older_store,
        open_meta_default_uses_store_format_version,
    ]
);
