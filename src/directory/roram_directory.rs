use std::{
    collections::HashMap,
    fs::{File, OpenOptions},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::{Arc, RwLock},
};

use common::file_slice::FileSlice;
use fs4::FileExt;

use crate::core::META_FILEPATH;

use super::{
    error::{LockError, OpenReadError, OpenWriteError},
    file_watcher::FileWatcher,
    mmap_directory::ReleaseLockFile,
    Directory, DirectoryLock, META_LOCK,
};

/// RoRamDirectory is a read only directory that stores data in RAM.
/// Note: please make sure the index files exist before creating a RoRamDirectory.
#[derive(Clone)]
pub struct RoRamDirectory {
    inner: Arc<RwLock<RoRamDirectoryInner>>,
}

impl RoRamDirectory {
    pub fn new(dir: &Path) -> Result<RoRamDirectory, std::io::Error> {
        Ok(RoRamDirectory {
            inner: Arc::new(RwLock::new(RoRamDirectoryInner::new(dir)?)),
        })
    }
}

impl std::fmt::Debug for RoRamDirectory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RoRamDirectory").finish()
    }
}

impl Directory for RoRamDirectory {
    fn get_file_handle(
        &self,
        path: &std::path::Path,
    ) -> Result<std::sync::Arc<dyn common::file_slice::FileHandle>, super::error::OpenReadError>
    {
        let file_slice = self.open_read(path)?;
        Ok(Arc::new(file_slice))
    }

    fn open_read(&self, path: &Path) -> Result<FileSlice, super::error::OpenReadError> {
        self.inner.write().unwrap().open_read(path)
    }

    fn delete(&self, path: &std::path::Path) -> Result<(), super::error::DeleteError> {
        self.inner.write().unwrap().delete(path);
        Ok(())
    }

    fn exists(&self, path: &std::path::Path) -> Result<bool, super::error::OpenReadError> {
        Ok(self.inner.read().unwrap().exists(path))
    }

    fn open_write(
        &self,
        path: &std::path::Path,
    ) -> Result<super::WritePtr, super::error::OpenWriteError> {
        unimplemented!()
    }

    fn acquire_lock(
        &self,
        lock: &super::Lock,
    ) -> Result<super::DirectoryLock, super::error::LockError> {
        let full_path = self.inner.read().unwrap().root_path.join(&lock.filepath);
        // We make sure that the file exists.
        let file: File = OpenOptions::new()
            .write(true)
            .create(true) //< if the file does not exist yet, create it.
            .truncate(false)
            .open(full_path)
            .map_err(LockError::wrap_io_error)?;
        if lock.is_blocking {
            file.lock_exclusive().map_err(LockError::wrap_io_error)?;
        } else {
            file.try_lock_exclusive().map_err(|_| LockError::LockBusy)?
        }
        // dropping the file handle will release the lock.
        Ok(DirectoryLock::from(Box::new(ReleaseLockFile {
            path: lock.filepath.clone(),
            _file: file,
        })))
    }

    fn atomic_read(&self, path: &std::path::Path) -> Result<Vec<u8>, super::error::OpenReadError> {
        let full_path = self.inner.read().unwrap().root_path.join(path);
        let mut buffer = Vec::new();
        match File::open(full_path) {
            Ok(mut file) => {
                file.read_to_end(&mut buffer).map_err(|io_error| {
                    OpenReadError::wrap_io_error(io_error, path.to_path_buf())
                })?;
                Ok(buffer)
            }
            Err(io_error) => {
                if io_error.kind() == io::ErrorKind::NotFound {
                    Err(OpenReadError::FileDoesNotExist(path.to_owned()))
                } else {
                    Err(OpenReadError::wrap_io_error(io_error, path.to_path_buf()))
                }
            }
        }
    }

    fn atomic_write(&self, _path: &std::path::Path, _data: &[u8]) -> std::io::Result<()> {
        unimplemented!("RoRamDirectory is read-only")
    }

    fn sync_directory(&self) -> std::io::Result<()> {
        Ok(())
    }

    fn watch(&self, watch_callback: super::WatchCallback) -> crate::Result<super::WatchHandle> {
        self.inner.read().unwrap().watch(watch_callback)
    }
}

struct RoRamDirectoryInner {
    root_path: PathBuf,
    files: HashMap<PathBuf, FileSlice>,
    watcher: FileWatcher,
}

fn open_file(path: &Path) -> Result<FileSlice, std::io::Error> {
    let mut file = File::open(path)?;
    let mut data = Vec::new();
    file.read_to_end(&mut data)?;
    let file_slice = FileSlice::from(data);
    Ok(file_slice)
}

impl RoRamDirectoryInner {
    fn new(dir: &Path) -> Result<RoRamDirectoryInner, std::io::Error> {
        // read all files in the directory
        let mut files: HashMap<PathBuf, FileSlice> = HashMap::new();
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            if !entry.file_type()?.is_file() {
                warn!("Skipping non-file {:?}", path);
                continue;
            }
            let file_slice = open_file(&path)?;
            files.insert(entry.file_name().into(), file_slice);
        }
        Ok(RoRamDirectoryInner {
            root_path: dir.to_path_buf(),
            files,
            watcher: FileWatcher::new(&dir.join(*META_FILEPATH)),
        })
    }

    fn open_read(&mut self, path: &Path) -> Result<FileSlice, super::error::OpenReadError> {
        let slice = self.files.get(path).cloned();
        match slice {
            Some(slice) => Ok(slice),
            None => {
                let full_path = self.root_path.join(path);
                let file_slice = open_file(&full_path)
                    .map_err(|io_error| OpenReadError::wrap_io_error(io_error, full_path))?;
                self.files.insert(path.to_path_buf(), file_slice.clone());
                Ok(file_slice)
            }
        }
    }

    fn exists(&self, path: &Path) -> bool {
        self.files.contains_key(path)
    }

    fn watch(&self, watch_callback: super::WatchCallback) -> crate::Result<super::WatchHandle> {
        Ok(self.watcher.watch(watch_callback))
    }

    fn delete(&mut self, path: &Path) {
        self.files.remove(path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn test_ro_ram_directory() {
        let dir = tempfile::tempdir().unwrap();
        let dir_path = dir.path();
        let file_name = Path::new("test.txt");
        let file_path = dir_path.join(file_name);
        let mut file = File::create(&file_path).unwrap();
        file.write_all(b"hello world").unwrap();

        let ram_dir = RoRamDirectory::new(dir_path).unwrap();

        let file_slice = ram_dir.open_read(&file_name).unwrap();
        assert_eq!(file_slice.read_bytes().unwrap().as_slice(), b"hello world");

        assert!(ram_dir.exists(&file_name).unwrap());
    }
}
