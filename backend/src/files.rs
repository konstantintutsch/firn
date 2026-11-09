//! Keeps the large files of activities, the original files and photos, on disk
//! instead of in the database, which stays small and quick to back up.
//!
//! Each activity has a folder named by its ID: `activities/<id>/original.zip`
//! and `activities/<id>/images/<image>`. Versions before activities had IDs of
//! their own named the folder by the Garmin ID, which `rename_activity` fixes.

use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use uuid::Uuid;

use crate::secret;

#[derive(Clone)]
pub struct FileStore {
    root: PathBuf,
}

impl FileStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    fn activity_dir(&self, activity_id: Uuid) -> PathBuf {
        self.root.join("activities").join(activity_id.to_string())
    }

    fn original_path(&self, activity_id: Uuid) -> PathBuf {
        self.activity_dir(activity_id).join("original.zip")
    }

    fn image_path(&self, activity_id: Uuid, image_id: &str) -> PathBuf {
        self.activity_dir(activity_id)
            .join("images")
            .join(file_name(image_id))
    }

    /// The original file as Garmin sent it, in its ZIP.
    pub async fn original(&self, activity_id: Uuid) -> Result<Option<Vec<u8>>> {
        read(&self.original_path(activity_id)).await
    }

    /// Saves the original file, or removes it with `None`.
    pub async fn save_original(&self, activity_id: Uuid, zip: Option<&[u8]>) -> Result<()> {
        let path = self.original_path(activity_id);
        match zip {
            Some(zip) => write(&path, zip).await,
            None => remove(&path).await,
        }
    }

    pub async fn image(&self, activity_id: Uuid, image_id: &str) -> Result<Option<Vec<u8>>> {
        read(&self.image_path(activity_id, image_id)).await
    }

    /// Replaces all images of an activity.
    pub async fn save_images(&self, activity_id: Uuid, images: &[(&str, &[u8])]) -> Result<()> {
        let dir = self.activity_dir(activity_id).join("images");
        remove_dir(&dir).await?;
        for (image_id, data) in images {
            self.save_image(activity_id, image_id, data).await?;
        }
        Ok(())
    }

    pub async fn save_image(&self, activity_id: Uuid, image_id: &str, data: &[u8]) -> Result<()> {
        write(&self.image_path(activity_id, image_id), data).await
    }

    /// A new path for a temporary file, on the same disk as the files, which an
    /// export or import can need much space for.
    pub async fn temporary_path(&self, prefix: &str) -> Result<PathBuf> {
        let dir = self.root.join("tmp");
        tokio::fs::create_dir_all(&dir)
            .await
            .with_context(|| format!("could not create {}", dir.display()))?;
        Ok(dir.join(format!("{prefix}-{}", secret::random_token())))
    }

    /// Gives the folder that was named by the Garmin ID the activity's own ID.
    /// Does nothing if there is no such folder, or the new one exists already.
    pub async fn rename_activity(&self, garmin_id: i64, activity_id: Uuid) -> Result<()> {
        let from = self.root.join("activities").join(garmin_id.to_string());
        let to = self.activity_dir(activity_id);
        if !tokio::fs::try_exists(&from).await? || tokio::fs::try_exists(&to).await? {
            return Ok(());
        }
        tokio::fs::rename(&from, &to)
            .await
            .with_context(|| format!("could not rename {}", from.display()))
    }

    /// Removes everything of an activity.
    pub async fn remove_activity(&self, activity_id: Uuid) -> Result<()> {
        remove_dir(&self.activity_dir(activity_id)).await
    }
}

/// Garmin's image IDs are plain, but a name from elsewhere must not lead out of the folder.
fn file_name(id: &str) -> String {
    let plain = !id.is_empty()
        && id.len() <= 100
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if plain {
        id.to_owned()
    } else {
        URL_SAFE_NO_PAD.encode(secret::sha256(id))
    }
}

async fn read(path: &Path) -> Result<Option<Vec<u8>>> {
    match tokio::fs::read(path).await {
        Ok(data) => Ok(Some(data)),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).with_context(|| format!("could not read {}", path.display())),
    }
}

/// Writes to a temporary file first, so that a crash never leaves half a file.
async fn write(path: &Path, data: &[u8]) -> Result<()> {
    let dir = path.parent().expect("files are in a folder");
    tokio::fs::create_dir_all(dir)
        .await
        .with_context(|| format!("could not create {}", dir.display()))?;
    let temporary = path.with_extension("partial");
    tokio::fs::write(&temporary, data)
        .await
        .with_context(|| format!("could not write {}", temporary.display()))?;
    tokio::fs::rename(&temporary, path)
        .await
        .with_context(|| format!("could not write {}", path.display()))?;
    Ok(())
}

async fn remove(path: &Path) -> Result<()> {
    match tokio::fs::remove_file(path).await {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).with_context(|| format!("could not remove {}", path.display())),
    }
}

async fn remove_dir(path: &Path) -> Result<()> {
    match tokio::fs::remove_dir_all(path).await {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).with_context(|| format!("could not remove {}", path.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_image_ids_stay_and_others_are_hashed() {
        assert_eq!(file_name("abc-123_x"), "abc-123_x");
        assert_eq!(file_name("../../etc/passwd").len(), 43);
        assert!(!file_name("a/b").contains('/'));
        assert_eq!(file_name("").len(), 43);
    }

    #[tokio::test]
    async fn saves_reads_and_removes_files() {
        let root = std::env::temp_dir().join(format!("firn-files-{}", secret::random_token()));
        let store = FileStore::new(&root);
        let id = Uuid::new_v4();
        assert_eq!(store.original(id).await.unwrap(), None);

        store.save_original(id, Some(b"zip")).await.unwrap();
        store
            .save_images(id, &[("a", b"one"), ("b", b"two")])
            .await
            .unwrap();
        assert_eq!(
            store.original(id).await.unwrap().as_deref(),
            Some(&b"zip"[..])
        );
        assert_eq!(
            store.image(id, "b").await.unwrap().as_deref(),
            Some(&b"two"[..])
        );

        store.save_images(id, &[("c", b"three")]).await.unwrap();
        assert_eq!(store.image(id, "a").await.unwrap(), None);

        store.save_original(id, None).await.unwrap();
        assert_eq!(store.original(id).await.unwrap(), None);
        store.remove_activity(id).await.unwrap();
        assert_eq!(store.image(id, "c").await.unwrap(), None);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn renames_a_folder_named_by_the_garmin_id() {
        let root = std::env::temp_dir().join(format!("firn-files-{}", secret::random_token()));
        let store = FileStore::new(&root);
        let id = Uuid::new_v4();
        let old = root.join("activities").join("42");
        std::fs::create_dir_all(&old).unwrap();
        std::fs::write(old.join("original.zip"), b"zip").unwrap();

        store.rename_activity(42, id).await.unwrap();
        assert_eq!(
            store.original(id).await.unwrap().as_deref(),
            Some(&b"zip"[..])
        );
        // Again, with nothing left to rename.
        store.rename_activity(42, id).await.unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }
}
