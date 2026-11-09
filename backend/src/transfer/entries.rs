use std::fs::File;
use std::io::{Cursor, Read, Write};

use anyhow::{Context, Result, bail};
use zip::write::SimpleFileOptions;
use zip::{ZipArchive, ZipWriter};

use crate::secret;

/// Far above any real activity file, but protects against a corrupt archive.
pub(super) const MAX_ENTRY_BYTES: u64 = 512 * 1024 * 1024;

pub(super) fn read_limited(entry: &mut impl Read) -> Result<Vec<u8>> {
    let mut content = Vec::new();
    entry
        .take(MAX_ENTRY_BYTES + 1)
        .read_to_end(&mut content)
        .context("could not extract ZIP entry")?;
    if content.len() as u64 > MAX_ENTRY_BYTES {
        bail!("a file in the archive is too large");
    }
    Ok(content)
}

pub(super) fn read_entry(archive: &mut ZipArchive<File>, name: &str) -> Result<Vec<u8>> {
    read_limited(&mut archive.by_name(name)?)
}

pub(super) fn optional_entry(
    archive: &mut ZipArchive<File>,
    name: &str,
) -> Result<Option<Vec<u8>>> {
    if archive.index_for_name(name).is_none() {
        return Ok(None);
    }
    read_entry(archive, name).map(Some)
}

pub(super) fn single_file_zip(name: &str, content: &[u8]) -> Result<Vec<u8>> {
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    zip.start_file(name, SimpleFileOptions::default())?;
    zip.write_all(content)?;
    Ok(zip.finish()?.into_inner())
}

/// An ID for a file from elsewhere, from a hash of it. Negative, so that it never
/// meets one of Garmin's, and the same for the same file, so that importing it
/// again updates it. Within 53 bits, which browsers read as numbers exactly.
pub(super) fn imported_id(identity: &[u8]) -> i64 {
    const MAX_EXACT_IN_JAVASCRIPT: i64 = (1 << 53) - 1;
    let hash = secret::sha256_bytes(identity);
    let number =
        i64::from_le_bytes(hash[..8].try_into().expect("8 bytes")) & MAX_EXACT_IN_JAVASCRIPT;
    -number.max(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn imported_ids_are_negative_and_stable() {
        let id = imported_id(b"file");
        assert!(id < 0 && id > -(1 << 53));
        assert_eq!(id, imported_id(b"file"));
        assert_ne!(id, imported_id(b"other file"));
    }
}
