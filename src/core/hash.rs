use std::fs::File;
use std::io::{BufReader, Read};
use std::path::Path;

use sha1::Sha1;
use sha2::{Digest, Sha256};

use super::error::{Error, Result};

const BUFFER_SIZE: usize = 64 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Algorithm {
    Sha1,
    Sha256,
}

/// A digest a downloaded file must match before it is used.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Checksum {
    Sha1(String),
    Sha256(String),
}

impl Checksum {
    pub fn algorithm(&self) -> Algorithm {
        match self {
            Self::Sha1(_) => Algorithm::Sha1,
            Self::Sha256(_) => Algorithm::Sha256,
        }
    }

    pub fn expected(&self) -> &str {
        match self {
            Self::Sha1(value) | Self::Sha256(value) => value,
        }
    }

    pub fn verify(&self, path: &Path) -> Result<()> {
        let actual = match self.algorithm() {
            Algorithm::Sha1 => sha1_file(path)?,
            Algorithm::Sha256 => sha256_file(path)?,
        };

        if actual.eq_ignore_ascii_case(self.expected()) {
            Ok(())
        } else {
            Err(Error::ChecksumMismatch {
                path: path.to_path_buf(),
                expected: self.expected().to_ascii_lowercase(),
                actual,
            })
        }
    }
}

pub fn sha256_file(path: &Path) -> Result<String> {
    digest_file(path, |file| {
        let mut hasher = Sha256::new();
        copy_into(file, &mut hasher)?;
        Ok(hex::encode(hasher.finalize()))
    })
}

pub fn sha1_file(path: &Path) -> Result<String> {
    digest_file(path, |file| {
        let mut hasher = Sha1::new();
        copy_into(file, &mut hasher)?;
        Ok(hex::encode(hasher.finalize()))
    })
}

pub fn sha256_bytes(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

pub fn verify_sha256(path: &Path, expected: &str) -> Result<()> {
    Checksum::Sha256(expected.to_owned()).verify(path)
}

pub fn verify_sha1(path: &Path, expected: &str) -> Result<()> {
    Checksum::Sha1(expected.to_owned()).verify(path)
}

fn digest_file<F>(path: &Path, read: F) -> Result<String>
where
    F: FnOnce(&mut BufReader<File>) -> Result<String>,
{
    let file = File::open(path)
        .map_err(|e| Error::io(format!("could not open {}", path.display()), e))?;
    let mut reader = BufReader::new(file);
    read(&mut reader)
}

fn copy_into(reader: &mut BufReader<File>, hasher: &mut impl Digest) -> Result<()> {
    let mut buffer = [0u8; BUFFER_SIZE];
    loop {
        let read = reader.read(&mut buffer).map_err(|e| {
            Error::io("could not read a file while hashing it".to_owned(), e)
        })?;
        if read == 0 {
            return Ok(());
        }
        hasher.update(&buffer[..read]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_file(bytes: &[u8]) -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("file.bin");
        std::fs::write(&path, bytes).unwrap();
        (dir, path)
    }

    #[test]
    fn sha256_matches_the_abc_vector() {
        let (_dir, path) = temp_file(b"abc");
        assert_eq!(
            sha256_file(&path).unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            sha256_bytes(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn sha1_matches_the_abc_vector() {
        let (_dir, path) = temp_file(b"abc");
        assert_eq!(
            sha1_file(&path).unwrap(),
            "a9993e364706816aba3e25717850c26c9cd0d89d"
        );
    }

    #[test]
    fn checksum_accepts_uppercase_and_rejects_a_different_file() {
        let (_dir, path) = temp_file(b"abc");
        let expected = "BA7816BF8F01CFEA414140DE5DAE2223B00361A396177A9CB410FF61F20015AD";
        verify_sha256(&path, expected).unwrap();

        let (_other_dir, other) = temp_file(b"abcd");
        assert!(matches!(
            verify_sha256(&other, expected),
            Err(Error::ChecksumMismatch { .. })
        ));
    }
}
