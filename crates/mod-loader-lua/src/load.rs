use std::{fs::File, io::BufReader, rc::Rc};

use kfc::{container::{KFCCursor, KFCFile, KFCReader, KFCReaderOptions, KFCWriteLock, KFCWriteOptions, KFCWriter}, reflection::TypeRegistry};

use crate::{alias::{Path, PathBuf}, log::{debug, error, info, warn}};

pub fn load_kfc_file(
    kfc_path: impl AsRef<Path>
) -> Result<Rc<KFCFile>, ()> {
    let kfc_path = kfc_path.as_ref();

    match KFCFile::from_path(kfc_path, false) {
        Ok(file) => Ok(Rc::new(file)),
        Err(e) => {
            error!(
                error = %e,
                path = ?kfc_path,
                "Failed to open KFC file",
            );
            Err(())
        }
    }
}

pub fn create_reader(
    dir: &Path,
    file_name: &str,
) -> Result<KFCCursor<KFCReader>, ()> {
    match KFCReader::new_with_options(
        dir,
        file_name,
        KFCReaderOptions {
            kfc_extension: "kfc.bak",
            ..Default::default()
        }
    ).and_then(|reader| reader.into_cursor()) {
        Ok(reader) => Ok(reader),
        Err(e) => {
            error!(
                path = ?dir,
                file_name = %file_name,
                error = %e,
                "Failed to create KFC reader",
            );
            Err(())
        }
    }
}

pub fn create_writer(
    dir: &Path,
    file_name: &str,
    type_registry: &Rc<TypeRegistry>,
    ref_file: &Rc<KFCFile>,
    transaction: KFCWriteLock,
) -> Result<KFCWriter<Rc<KFCFile>, Rc<TypeRegistry>>, ()> {
    let writer = match KFCWriter::new_incremental_with_lock(
        dir,
        file_name,
        ref_file.clone(),
        type_registry.clone(),
        KFCWriteOptions {
            overwrite_containers: true,
            ..Default::default()
        },
        transaction,
    ) {
        Ok(writer) => writer,
        Err(e) => {
            error!(
                path = ?dir,
                file_name = %file_name,
                error = %e,
                "Failed to create KFC writer",
            );
            return Err(());
        }
    };

    Ok(writer)
}

#[cfg(test)]
fn create_backup(kfc_path: impl AsRef<Path>) -> Result<PathBuf, ()> {
    let transaction = KFCWriteLock::acquire(kfc_path.as_ref().as_std_path()).map_err(|_| ())?;
    create_backup_locked(kfc_path, &transaction)
}

pub fn create_backup_locked(kfc_path: impl AsRef<Path>, transaction: &KFCWriteLock) -> Result<PathBuf, ()> {
    let path = kfc_path.as_ref();
    transaction.ensure_path(path.as_std_path()).map_err(|e| {
        error!(error = %e, "Invalid backup transaction");
    })?;
    let bak_path = PathBuf::from(format!("{path}.bak"));
    if bak_path.exists() && validate_backup(path, &bak_path)? {
        return Ok(bak_path);
    }
    // Read the complete replacement before touching the previous backup.
    let bytes = std::fs::read(path).map_err(|e| {
        error!(error = %e, path = ?path, "Failed to read backup source");
    })?;
    KFCFile::get_version_tag(path).map_err(|e| {
        error!(error = %e, path = ?path, "Invalid backup source");
    })?;
    kfc::container::replace_file_atomically(bak_path.as_std_path(), &bytes).map_err(|e| {
        error!(error = %e, path = ?bak_path, "Failed to publish backup; previous backup preserved");
    })?;
    info!(path = ?bak_path, "Backup file created successfully");
    Ok(bak_path)
}

pub fn restore_backup(
    kfc_path: impl AsRef<Path>,
) -> Result<(), ()> {
    let path = kfc_path.as_ref();
    let _write_lock = kfc::container::KFCWriteLock::acquire(path.as_std_path()).map_err(|e| {
        error!(error = %e, "Could not lock game data for restoration");
    })?;
    let mut bak_path = path.to_string();
    bak_path.push_str(".bak");
    let bak_path = Path::new(&bak_path);

    if !bak_path.exists() {
        warn!(
            path = ?bak_path,
            "Backup file does not exist, cannot restore.",
        );
        return Err(());
    }

    if !bak_path.is_file() {
        error!(
            path = ?bak_path,
            "Backup path is not a file, please remove it manually.",
        );
        return Err(());
    }

    if !validate_backup(path, bak_path)? {
        warn!(
            path = ?path,
            backup_path = ?bak_path,
            "Backup file is not compatible with the current KFC file, cannot restore.",
        );
        return Err(());
    }

    let result = std::fs::read(bak_path).and_then(|bytes|
        kfc::container::replace_file_atomically(path.as_std_path(), &bytes));
    if let Err(e) = result {
        error!(
            path = ?path,
            backup_path = ?bak_path,
            error = %e,
            "Failed to restore backup file",
        );
        return Err(());
    } else {
        info!(
            path = ?path,
            backup_path = ?bak_path,
            "Backup file restored successfully",
        );
    }

    Ok(())
}

/// Checks whether the backup file has the same version tag as the KFC file.
///
/// Returns `Err` if the version tag of the KFC file cannot be read, since
/// compatibility cannot be determined in that case. An unreadable backup file
/// is reported as incompatible.
pub fn validate_backup(
    kfc_path: &Path,
    kfc_path_bak: &Path
) -> Result<bool, ()> {
    let version = match KFCFile::get_version_tag(kfc_path) {
        Ok(version) => version,
        Err(e) => {
            error!(
                path = ?kfc_path,
                error = %e,
                "Failed to read version tag of KFC file",
            );
            return Err(());
        }
    };

    let version_bak = match KFCFile::get_version_tag(kfc_path_bak) {
        Ok(version) => version,
        Err(e) => {
            warn!(
                path = ?kfc_path_bak,
                error = %e,
                "Failed to read version tag of backup file, treating it as incompatible",
            );
            return Ok(false);
        }
    };

    Ok(version == version_bak)
}

pub fn load_type_registry(
    game_dir: &Path,
    cache_dir: &Path,
    file_name: &str,
) -> Result<(TypeRegistry, bool), ()> {
    if let Err(e) = std::fs::create_dir_all(cache_dir) {
        warn!(
            error = %e,
            path = ?cache_dir,
            "Failed to create cache directory, unable to load or save type registry",
        );
        return Err(());
    }

    let types_path = cache_dir.join("types.json");
    let exe_path = game_dir.join(file_name).with_extension("exe");
    let kfc_path = game_dir.join(file_name).with_extension("kfc");

    let type_registry = match File::open(&types_path) {
        Ok(file) => {
            let reader = BufReader::new(file);

            match serde_json::from_reader::<_, TypeRegistry>(reader) {
                Ok(registry) => Some(registry),
                Err(e) => {
                    debug!(
                        error = %e,
                        path = ?types_path,
                        "Failed to read type registry from file, attempting to extract types...",
                    );

                    None
                },
            }
        }
        Err(e) => {
            debug!(
                error = %e,
                path = ?types_path,
                "Failed to read type registry from file, attempting to extract types...",
            );

            None
        },
    };

    let version_tag = match KFCFile::get_version_tag(&kfc_path) {
        Ok(tag) => Some(tag),
        Err(e) => {
            warn!(
                error = %e,
                path = ?types_path,
                kfc_path = ?kfc_path,
                "Failed to get version tag from KFC file, assuming types are outdated, attempting to extract types...",
            );

            None
        }
    };

    let type_registry = match type_registry {
        Some(type_registry) => {
            if let Some(version_tag) = &version_tag {
                if version_tag != &type_registry.version {
                    warn!(
                        path = ?types_path,
                        kfc_path = ?kfc_path,
                        "Type registry is outdated, attempting to extract types again..."
                    );

                    None
                } else {
                    Some(type_registry)
                }
            } else {
                None
            }
        },
        None => None
    };

    let (type_registry, is_dirty) = match type_registry {
        Some(type_registry) => (type_registry, false),
        None => match TypeRegistry::load_from_executable(&exe_path) {
            Ok(mut registry) => {
                if let Some(version_tag) = version_tag {
                    registry.version = version_tag;

                    match serde_json::to_string(&registry) {
                        Ok(json) => {
                            if let Err(e) = std::fs::write(&types_path, json) {
                                warn!(
                                    error = %e,
                                    path = ?types_path,
                                    "Failed to write type registry to file",
                                );
                            }
                        }
                        Err(e) => {
                            warn!(
                                error = %e,
                                path = ?types_path,
                                "Failed to serialize type registry to JSON",
                            );
                        }
                    }
                } else {
                    warn!(
                        path = ?types_path,
                        kfc_path = ?kfc_path,
                        "No version tag found, type registry cannot be saved",
                    );
                }

                (registry, true)
            }
            Err(e) => {
                error!(
                    error = %e,
                    path = ?types_path,
                    exe_path = ?exe_path,
                    "Failed to load type registry",
                );

                return Err(());
            }
        }
    };

    info!(
        path = ?types_path,
        exe_path = ?exe_path,
        kfc_path = ?kfc_path,
        version = type_registry.version,
        is_dirty = is_dirty,
        "Type registry loaded successfully",
    );

    Ok((type_registry, is_dirty))
}

pub fn export_lua_definitions(
    cache_dir: &Path,
    type_registry: &TypeRegistry,
    force: bool,
) {
    let lua_cache_dir = cache_dir.join("lua");

    if let Err(e) = std::fs::create_dir_all(&lua_cache_dir) {
        warn!(
            error = %e,
            path = ?lua_cache_dir,
            "Failed to create lua cache directory, unable to generate lua definitions",
        );
    } else {
        let lua_type_def_path = lua_cache_dir.join("types.lua");
        let lua_base_def_path = lua_cache_dir.join("base.lua");

        if force || !lua_type_def_path.exists() {
            let def = crate::definition::generator::generate(type_registry);

            match std::fs::write(&lua_type_def_path, def) {
                Ok(_) => info!(
                    path = ?lua_type_def_path,
                    "Lua type definition file has been generated",
                ),
                Err(e) => {
                    warn!(
                        error = %e,
                        path = ?lua_type_def_path,
                        "Failed to write lua type definition file",
                    );
                }
            }
        }

        if force || !lua_base_def_path.exists() {
            match std::fs::write(&lua_base_def_path, crate::definition::DEFINITION_FILE) {
                Ok(_) => info!(
                    path = ?lua_base_def_path,
                    "Lua base definition file has been generated",
                ),
                Err(e) => {
                    warn!(
                        error = %e,
                        path = ?lua_base_def_path,
                        "Failed to write lua base definition file",
                    );
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> Self {
            let dir = PathBuf::try_from(std::env::temp_dir())
                .unwrap()
                .join(format!("eml-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// Writes a minimal KFC header followed by the given version tag.
    fn write_kfc(path: &Path, version: &str) {
        const HEADER_SIZE: u32 = 16 + 16 * 8;
        const VERSION_LOCATION: u32 = 16;

        let mut data = Vec::new();
        data.extend_from_slice(b"KFC3");
        data.extend_from_slice(&0u32.to_le_bytes());
        data.extend_from_slice(&12u32.to_le_bytes());
        data.extend_from_slice(&[0; 4]);
        data.extend_from_slice(&(HEADER_SIZE - VERSION_LOCATION).to_le_bytes());
        data.extend_from_slice(&(version.len() as u32).to_le_bytes());
        data.resize(HEADER_SIZE as usize, 0);
        data.extend_from_slice(version.as_bytes());

        std::fs::write(path, data).unwrap();
    }

    fn read_version(path: &Path) -> String {
        KFCFile::get_version_tag(path).unwrap()
    }

    #[test]
    fn competing_backup_cannot_capture_an_in_progress_publication() {
        let dir = TempDir::new("backup-competing");
        let path = dir.0.join("test.kfc");
        write_kfc(&path, "1.0");
        let transaction = kfc::container::KFCWriteLock::acquire(path.as_std_path()).unwrap();
        let other_path = path.clone();
        let result = std::thread::spawn(move || create_backup(&other_path)).join().unwrap();
        assert!(result.is_err(), "backup must not read a transaction's changing directory");
        assert!(!dir.0.join("test.kfc.bak").exists());
        drop(transaction);
        assert!(create_backup(&path).is_ok());
    }

    #[test]
    fn restore_and_backup_cannot_interleave_publication_and_cache_recording() {
        let dir = TempDir::new("publication-cache");
        let path = dir.0.join("test.kfc");
        write_kfc(&path, "test-1.0");
        File::create(dir.0.join("test.kfc_resources")).unwrap();
        KFCWriter::<KFCFile, _>::new(&dir.0, "test", TypeRegistry::default(), "test-1.0")
            .unwrap().finalize().unwrap();
        let baseline = KFCFile::from_path(&path, false).unwrap();
        let vanilla = std::fs::read(&path).unwrap();
        let transaction = KFCWriteLock::acquire(path.as_std_path()).unwrap();
        let backup = create_backup_locked(&path, &transaction).unwrap();
        let mut writer = KFCWriter::new_incremental_with_lock(
            &dir.0, "test", baseline, TypeRegistry::default(),
            KFCWriteOptions::default(), transaction.clone(),
        ).unwrap();
        writer.write_content(&kfc::guid::ContentHash::from_data(b"patch"), b"patch").unwrap();
        writer.finalize().unwrap();
        let published = std::fs::read(&path).unwrap();
        assert_ne!(published, vanilla);
        let other_path = path.clone();
        assert!(std::thread::spawn(move || restore_backup(other_path)).join().unwrap().is_err());
        let other_path = path.clone();
        assert!(std::thread::spawn(move || create_backup(other_path)).join().unwrap().is_err());
        assert_eq!(std::fs::read(&path).unwrap(), published);
        assert_eq!(std::fs::read(&backup).unwrap(), vanilla);
        let mut cache = crate::cache::FileStateCache::default();
        cache.track_game_files(&dir.0);
        cache.write(&dir.0);
        drop(transaction);
        assert!(create_backup(&path).is_ok());
        assert_eq!(std::fs::read(&backup).unwrap(), vanilla);
        assert!(restore_backup(&path).is_ok());
        assert_eq!(std::fs::read(&path).unwrap(), vanilla);
    }

    #[test]
    fn create_backup_keeps_backup_with_equal_version() {
        let dir = TempDir::new("backup-equal");
        let kfc_path = dir.0.join("test.kfc");
        let bak_path = dir.0.join("test.kfc.bak");
        write_kfc(&kfc_path, "1.0");
        write_kfc(&bak_path, "1.0");
        let backup = std::fs::read(&bak_path).unwrap();
        std::fs::OpenOptions::new()
            .append(true)
            .open(&kfc_path)
            .and_then(|mut file| std::io::Write::write_all(&mut file, b"patched"))
            .unwrap();

        assert_eq!(create_backup(&kfc_path), Ok(bak_path.clone()));
        assert_eq!(std::fs::read(&bak_path).unwrap(), backup);
    }

    #[test]
    fn create_backup_replaces_backup_with_different_version() {
        let dir = TempDir::new("backup-different");
        let kfc_path = dir.0.join("test.kfc");
        let bak_path = dir.0.join("test.kfc.bak");
        write_kfc(&kfc_path, "2.0");
        write_kfc(&bak_path, "1.0");

        assert_eq!(create_backup(&kfc_path), Ok(bak_path.clone()));
        assert_eq!(read_version(&bak_path), "2.0");
    }

    #[test]
    fn create_backup_replaces_unreadable_backup() {
        let dir = TempDir::new("backup-corrupt");
        let kfc_path = dir.0.join("test.kfc");
        let bak_path = dir.0.join("test.kfc.bak");
        write_kfc(&kfc_path, "2.0");
        std::fs::write(&bak_path, b"corrupt").unwrap();

        assert_eq!(create_backup(&kfc_path), Ok(bak_path.clone()));
        assert_eq!(read_version(&bak_path), "2.0");
    }

    #[test]
    fn create_backup_keeps_backup_if_kfc_is_unreadable() {
        let dir = TempDir::new("backup-unreadable");
        let kfc_path = dir.0.join("test.kfc");
        let bak_path = dir.0.join("test.kfc.bak");
        std::fs::write(&kfc_path, b"corrupt").unwrap();
        write_kfc(&bak_path, "1.0");

        assert_eq!(create_backup(&kfc_path), Err(()));
        assert_eq!(read_version(&bak_path), "1.0");
    }

    #[test]
    fn create_backup_keeps_backup_if_kfc_is_missing() {
        let dir = TempDir::new("backup-missing");
        let kfc_path = dir.0.join("test.kfc");
        let bak_path = dir.0.join("test.kfc.bak");
        write_kfc(&bak_path, "1.0");

        assert_eq!(create_backup(&kfc_path), Err(()));
        assert_eq!(read_version(&bak_path), "1.0");
    }
}
