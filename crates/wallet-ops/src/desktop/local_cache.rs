use super::*;

pub fn reset_local_merkle_forest_cache(db: &DbStore) -> Result<usize> {
    let dir = db.blob_dir().join("merkle_forest");
    let removed = count_files(&dir)?;
    match std::fs::remove_dir_all(&dir) {
        Ok(()) => Ok(removed),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(0),
        Err(error) => Err(error).wrap_err("remove local Merkle forest cache"),
    }
}

fn count_files(path: &Path) -> Result<usize> {
    let entries = match std::fs::read_dir(path) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(error).wrap_err("read local Merkle forest cache"),
    };

    let mut count = 0;
    for entry in entries {
        let entry = entry.wrap_err("read local Merkle forest cache entry")?;
        let file_type = entry
            .file_type()
            .wrap_err("read local Merkle forest cache entry type")?;
        if file_type.is_dir() {
            count += count_files(&entry.path())?;
        } else {
            count += 1;
        }
    }
    Ok(count)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::SystemTime;

    use super::*;

    static TEMP_DB_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn temp_db_root() -> PathBuf {
        let dir = std::env::temp_dir().join("railoxide-local-cache-tests");
        fs::create_dir_all(&dir).expect("create temp db dir");
        let pid = std::process::id();
        let nanos = SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos());
        let counter = TEMP_DB_COUNTER.fetch_add(1, Ordering::Relaxed);
        dir.join(format!("db-{pid}-{nanos}-{counter}"))
    }

    #[test]
    fn reset_local_merkle_forest_cache_removes_only_forest_blobs() {
        let root_dir = temp_db_root();
        let db = DbStore::open(DbConfig {
            root_dir: root_dir.clone(),
        })
        .expect("open test db");
        let forest_dir = db.blob_dir().join("merkle_forest");
        let anchors_dir = forest_dir.join("anchors");
        let other_dir = db.blob_dir().join("artifacts");
        fs::create_dir_all(&anchors_dir).expect("create anchors dir");
        fs::create_dir_all(&other_dir).expect("create other dir");
        fs::write(forest_dir.join("forest-1.msgpack"), b"forest").expect("write forest file");
        fs::write(anchors_dir.join("forest-1-anchor.msgpack"), b"anchor")
            .expect("write anchor file");
        fs::write(other_dir.join("artifact.bin"), b"artifact").expect("write artifact file");
        db.put_app_settings_record("wallet-settings", b"settings-v1")
            .expect("store settings");

        let removed = reset_local_merkle_forest_cache(&db).expect("reset forest cache");

        assert_eq!(removed, 2);
        assert!(!forest_dir.exists());
        assert!(other_dir.join("artifact.bin").exists());
        assert_eq!(
            db.get_app_settings_record("wallet-settings")
                .expect("load settings")
                .expect("settings present"),
            b"settings-v1"
        );
        drop(db);
        fs::remove_dir_all(root_dir).expect("remove temp db dir");
    }

    #[test]
    fn reset_local_merkle_forest_cache_allows_missing_directory() {
        let root_dir = temp_db_root();
        let db = DbStore::open(DbConfig {
            root_dir: root_dir.clone(),
        })
        .expect("open test db");

        let removed = reset_local_merkle_forest_cache(&db).expect("reset forest cache");

        assert_eq!(removed, 0);
        drop(db);
        fs::remove_dir_all(root_dir).expect("remove temp db dir");
    }
}
