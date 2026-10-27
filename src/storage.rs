use crate::model::History;
use fs2::FileExt;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

pub struct Store {
    directory: PathBuf,
    _lock: File,
}

impl Store {
    pub fn default_directory() -> Result<PathBuf, String> {
        let home =
            std::env::var_os("HOME").ok_or("Cannot find your home directory (HOME is unset).")?;
        Ok(PathBuf::from(home).join("Library/Application Support/MacAi"))
    }

    pub fn open(directory: PathBuf) -> Result<(Self, History), String> {
        fs::create_dir_all(&directory)
            .map_err(|error| format!("Cannot create {}: {error}", directory.display()))?;
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))
            .map_err(|error| format!("Cannot protect the history folder: {error}"))?;
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .mode(0o600)
            .open(directory.join("history.lock"))
            .map_err(|error| format!("Cannot open the history lock: {error}"))?;
        lock.try_lock_exclusive().map_err(|error| {
            format!("Cannot lock chat history. Another MacAi window may already be open.\n{error}")
        })?;
        let store = Self {
            directory,
            _lock: lock,
        };
        let mut history = match fs::read(store.path()) {
            Ok(bytes) => {
                let history: History = serde_json::from_slice(&bytes).map_err(|error| {
                    format!(
                        "Cannot read {}. Your existing file has not been changed.\n{error}",
                        store.path().display()
                    )
                })?;
                if history.version != 1 {
                    return Err(format!(
                        "History format {} is not supported. Your file has not been changed.",
                        history.version
                    ));
                }
                let mut ids = std::collections::HashSet::new();
                if history.chats.iter().any(|chat| !ids.insert(&chat.id)) {
                    return Err(
                        "Chat history contains duplicate IDs. Your file has not been changed."
                            .into(),
                    );
                }
                history
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => History::default(),
            Err(error) => return Err(format!("Cannot read chat history: {error}")),
        };
        history.recover();
        store.save(&history)?;
        Ok((store, history))
    }

    pub fn directory(&self) -> &Path {
        &self.directory
    }

    fn path(&self) -> PathBuf {
        self.directory.join("history.json")
    }

    pub fn save(&self, history: &History) -> Result<(), String> {
        let result = (|| -> Result<(), Box<dyn std::error::Error>> {
            let mut file = tempfile::NamedTempFile::new_in(&self.directory)?;
            serde_json::to_writer(&mut file, history)?;
            file.write_all(b"\n")?;
            file.as_file().sync_all()?;
            file.persist(self.path())?;
            File::open(&self.directory)?.sync_all()?;
            Ok(())
        })();
        result.map_err(|error| {
            format!(
                "Your latest changes could not be saved to {}.\n{error}",
                self.path().display()
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::MessageStatus;

    #[test]
    fn history_round_trips_with_private_permissions_and_interrupted_recovery() {
        let directory = tempfile::tempdir().unwrap();
        let (store, mut history) = Store::open(directory.path().to_owned()).unwrap();
        history.selected_mut().begin("Remember otter".into());
        history.selected_mut().messages[1].text = "Partial answer".into();
        history.selected_mut().draft = "A draft".into();
        store.save(&history).unwrap();
        assert_eq!(
            fs::metadata(store.path()).unwrap().permissions().mode() & 0o777,
            0o600
        );
        drop(store);
        let (_, loaded) = Store::open(directory.path().to_owned()).unwrap();
        assert_eq!(loaded.selected().draft, "A draft");
        assert_eq!(loaded.selected().messages[1].text, "Partial answer");
        assert_eq!(
            loaded.selected().messages[1].status,
            MessageStatus::Interrupted
        );
    }

    #[test]
    fn a_second_instance_cannot_overwrite_history() {
        let directory = tempfile::tempdir().unwrap();
        let (_store, _) = Store::open(directory.path().to_owned()).unwrap();
        assert!(Store::open(directory.path().to_owned()).is_err());
    }

    #[test]
    fn corrupted_and_future_history_are_not_overwritten() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("history.json");
        for contents in [
            "not json".to_owned(),
            r#"{"version":99,"selected":"","chats":[]}"#.to_owned(),
        ] {
            fs::write(&path, &contents).unwrap();
            assert!(Store::open(directory.path().to_owned()).is_err());
            assert_eq!(fs::read_to_string(&path).unwrap(), contents);
        }
    }
}
