use ruffle_core::backend::storage::StorageBackend;
use sha2::{Digest, Sha256};
use std::cell::Cell;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

const MAGIC: &[u8; 8] = b"LWSTORE1";

struct Record {
    generation: u64,
    deleted: bool,
    data: Vec<u8>,
    slot: usize,
}

pub struct FileStorage {
    root: PathBuf,
    read_failed: Cell<bool>,
}

impl FileStorage {
    pub fn new(root: impl Into<PathBuf>) -> io::Result<Self> {
        let root = root.into();
        fs::create_dir_all(&root)?;
        if fs::symlink_metadata(&root)?.file_type().is_symlink() {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "Save directory is a symbolic link",
            ));
        }
        Ok(Self {
            root,
            read_failed: Cell::new(false),
        })
    }

    fn path(&self, name: &str, slot: usize) -> PathBuf {
        self.root
            .join(format!("{:x}.{slot}", Sha256::digest(name.as_bytes())))
    }

    fn read_slot(path: &Path, slot: usize) -> io::Result<Option<Record>> {
        let metadata = match fs::symlink_metadata(path) {
            Ok(value) => value,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        if !metadata.file_type().is_file() {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "Save slot is not a regular file",
            ));
        }
        let bytes = fs::read(path)?;
        let invalid = || io::Error::new(io::ErrorKind::InvalidData, "Invalid save record");
        if bytes.len() < 53 || &bytes[..8] != MAGIC || bytes[16] > 1 {
            return Err(invalid());
        }
        let length = u32::from_le_bytes(bytes[17..21].try_into().unwrap()) as usize;
        if length.checked_add(53) != Some(bytes.len())
            || Sha256::digest(&bytes[..bytes.len() - 32])[..] != bytes[bytes.len() - 32..]
            || (bytes[16] == 1 && length != 0)
        {
            return Err(invalid());
        }
        Ok(Some(Record {
            generation: u64::from_le_bytes(bytes[8..16].try_into().unwrap()),
            deleted: bytes[16] == 1,
            data: bytes[21..21 + length].to_vec(),
            slot,
        }))
    }

    fn latest(&self, name: &str) -> io::Result<Option<Record>> {
        let mut records = Vec::new();
        let mut corrupt = false;
        for slot in 0..2 {
            match Self::read_slot(&self.path(name, slot), slot) {
                Ok(Some(record)) => records.push(record),
                Ok(None) => (),
                Err(error) if error.kind() == io::ErrorKind::InvalidData => corrupt = true,
                Err(error) => return Err(error),
            }
        }
        if corrupt && records.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "No valid save generation remains",
            ));
        }
        if corrupt {
            tracing::warn!("Recovered the last valid save generation");
        }
        records.sort_by_key(|record| record.generation);
        if records.len() == 2
            && records[0].generation == records[1].generation
            && (records[0].data != records[1].data || records[0].deleted != records[1].deleted)
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Conflicting save generations",
            ));
        }
        Ok(records.pop())
    }

    fn commit(&mut self, name: &str, data: Option<&[u8]>) -> io::Result<()> {
        if self.read_failed.get() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Save writes disabled after a read failure",
            ));
        }
        let current = self.latest(name)?;
        let generation = current
            .as_ref()
            .map_or(0, |record| record.generation)
            .checked_add(1)
            .ok_or_else(|| io::Error::other("Save generation overflow"))?;
        let slot = current.as_ref().map_or(0, |record| 1 - record.slot);
        let path = self.path(name, slot);
        if let Ok(metadata) = fs::symlink_metadata(&path) {
            if !metadata.file_type().is_file() {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "Save slot is not a regular file",
                ));
            }
        }
        let mut bytes = Vec::new();
        let payload = data.unwrap_or_default();
        let length =
            u32::try_from(payload.len()).map_err(|_| io::Error::other("Save is too large"))?;
        bytes.extend_from_slice(MAGIC);
        bytes.extend_from_slice(&generation.to_le_bytes());
        bytes.push(data.is_none() as u8);
        bytes.extend_from_slice(&length.to_le_bytes());
        bytes.extend_from_slice(payload);
        bytes.extend_from_slice(&Sha256::digest(&bytes));
        // Vita's libc rename removes the destination before renaming; retain the current slot.
        let mut file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(path)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        Ok(())
    }
}

impl StorageBackend for FileStorage {
    fn get(&self, name: &str) -> Option<Vec<u8>> {
        match self.latest(name) {
            Ok(record) => record
                .filter(|value| !value.deleted)
                .map(|value| value.data),
            Err(error) => {
                self.read_failed.set(true);
                tracing::error!("Save read failed: {error}");
                None
            }
        }
    }

    fn put(&mut self, name: &str, value: &[u8]) -> bool {
        if let Err(error) = self.commit(name, Some(value)) {
            tracing::error!("Save write failed: {error}");
            false
        } else {
            true
        }
    }

    fn remove_key(&mut self, name: &str) {
        if let Err(error) = self.commit(name, None) {
            tracing::error!("Save removal failed: {error}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saves_survive_reopening_and_deletion_uses_a_tombstone() {
        let directory = tempfile::tempdir().unwrap();
        let mut store = FileStorage::new(directory.path()).unwrap();
        assert!(store.put("localhost/Lonewolf.swf/progress", b"first"));
        assert!(store.put("localhost/Lonewolf.swf/progress", b"second"));
        drop(store);
        let mut store = FileStorage::new(directory.path()).unwrap();
        assert_eq!(
            store.get("localhost/Lonewolf.swf/progress").unwrap(),
            b"second"
        );
        assert_eq!(store.get_size("localhost/Lonewolf.swf/progress"), Some(6));
        store.remove_key("localhost/Lonewolf.swf/progress");
        drop(store);
        assert!(
            FileStorage::new(directory.path())
                .unwrap()
                .get("localhost/Lonewolf.swf/progress")
                .is_none()
        );
    }

    #[test]
    fn interrupted_new_generation_preserves_the_previous_save() {
        let directory = tempfile::tempdir().unwrap();
        let mut store = FileStorage::new(directory.path()).unwrap();
        assert!(store.put("progress", b"old"));
        assert!(store.put("progress", b"new"));
        let latest = store.latest("progress").unwrap().unwrap();
        fs::write(store.path("progress", latest.slot), b"LWSTORE1partial").unwrap();
        assert_eq!(store.get("progress").unwrap(), b"old");
        assert!(store.put("progress", b"recovered"));
        assert_eq!(store.get("progress").unwrap(), b"recovered");
    }

    #[test]
    fn checksum_detects_complete_but_corrupt_payloads() {
        let directory = tempfile::tempdir().unwrap();
        let mut store = FileStorage::new(directory.path()).unwrap();
        assert!(store.put("progress", b"old"));
        assert!(store.put("progress", b"new"));
        let path = store.path("progress", 1);
        let mut bytes = fs::read(&path).unwrap();
        bytes[21] ^= 1;
        fs::write(&path, bytes).unwrap();
        assert_eq!(store.get("progress").unwrap(), b"old");
    }

    #[test]
    fn unreadable_history_blocks_writes_instead_of_resetting_progress() {
        let directory = tempfile::tempdir().unwrap();
        let mut store = FileStorage::new(directory.path()).unwrap();
        assert!(store.put("progress", b"old"));
        let path = store.path("progress", 0);
        let previous = fs::read(&path).unwrap();
        fs::write(&path, b"damaged").unwrap();
        assert!(store.get("progress").is_none());
        fs::write(&path, &previous).unwrap();
        assert!(!store.put("progress", b"reset"));
        assert_eq!(fs::read(&path).unwrap(), previous);
    }

    #[test]
    fn keys_cannot_escape_the_save_directory() {
        let directory = tempfile::tempdir().unwrap();
        let mut store = FileStorage::new(directory.path()).unwrap();
        for key in ["../outside", "/absolute", "ux0:/other/game"] {
            assert!(store.put(key, key.as_bytes()));
            assert!(
                store
                    .path(key, 0)
                    .parent()
                    .is_some_and(|p| p == directory.path())
            );
            assert_eq!(store.get(key).unwrap(), key.as_bytes());
        }
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 3);
    }
}
