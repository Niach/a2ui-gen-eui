//! Model files: sha256 pinning, and an explicit download (feature `download`).

use std::io::Read;
use std::path::Path;

use sha2::{Digest, Sha256};

use crate::{Error, Result};

/// A model file pinned by hash.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelSpec {
    pub file_name: String,
    pub url: String,
    /// Lowercase hex.
    pub sha256: String,
}

impl ModelSpec {
    /// A pinned file; the file name is the URL's last segment.
    pub fn new(url: impl Into<String>, sha256: impl Into<String>) -> Self {
        let url = url.into();
        let path = url.split(['?', '#']).next().unwrap_or(&url);
        let file_name = path.rsplit('/').next().unwrap_or("model.gguf").to_owned();
        ModelSpec {
            file_name,
            url,
            sha256: sha256.into().to_lowercase(),
        }
    }

    /// The base (not fine-tuned) Qwen3.5 0.8B, Q4_K_M, 508 MB. For trying the runtime out.
    pub fn base_0_8b() -> Self {
        Self::new(
            "https://huggingface.co/unsloth/Qwen3.5-0.8B-GGUF/resolve/6ab461498e2023f6e3c1baea90a8f0fe38ab64d0/Qwen3.5-0.8B-Q4_K_M.gguf",
            "bd258782e35f7f458f8aced1adc053e6e92e89bc735ba3be89d38a06121dc517",
        )
    }
}

/// The file's sha256, lowercase hex.
pub fn sha256(path: impl AsRef<Path>) -> Result<String> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        match file.read(&mut buf)? {
            0 => break,
            n => hasher.update(&buf[..n]),
        }
    }
    Ok(hex(&hasher.finalize()))
}

/// Fails unless the file has the given sha256.
pub fn verify(path: impl AsRef<Path>, expected: &str) -> Result<()> {
    let actual = sha256(&path)?;
    check(path.as_ref(), expected, actual)
}

fn check(path: &Path, expected: &str, actual: String) -> Result<()> {
    if actual.eq_ignore_ascii_case(expected) {
        return Ok(());
    }
    Err(Error::Sha256 {
        path: path.display().to_string(),
        expected: expected.to_owned(),
        actual,
    })
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Downloads `spec` into `dir` unless it is there, and returns its path. The file appears under its name
/// only after its hash matched; `progress` gets bytes done and, when known, the total.
#[cfg(feature = "download")]
pub fn download(
    spec: &ModelSpec,
    dir: impl AsRef<Path>,
    mut progress: impl FnMut(u64, Option<u64>),
) -> Result<std::path::PathBuf> {
    use std::io::Write;

    let dir = dir.as_ref();
    if spec.file_name.contains(['/', '\\']) || spec.file_name.starts_with('.') {
        return Err(Error::Io(format!("not a file name: {}", spec.file_name)));
    }
    let path = dir.join(&spec.file_name);
    if path.exists() {
        verify(&path, &spec.sha256)?;
        return Ok(path);
    }
    std::fs::create_dir_all(dir)?;
    let part = dir.join(format!("{}.{}.part", spec.file_name, std::process::id()));
    let run = |progress: &mut dyn FnMut(u64, Option<u64>)| -> Result<()> {
        let mut response = ureq::get(&spec.url)
            .call()
            .map_err(|e| Error::Io(e.to_string()))?;
        let total = response.body().content_length();
        let mut body = response.body_mut().as_reader();
        let mut file = std::fs::File::create(&part)?;
        let mut hasher = Sha256::new();
        let mut buf = vec![0u8; 1 << 20];
        let mut done = 0u64;
        loop {
            let n = body.read(&mut buf)?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
            file.write_all(&buf[..n])?;
            done += n as u64;
            progress(done, total);
        }
        file.sync_all()?;
        check(&path, &spec.sha256, hex(&hasher.finalize()))?;
        std::fs::rename(&part, &path)?;
        Ok(())
    };
    let result = run(&mut progress);
    if result.is_err() {
        let _ = std::fs::remove_file(&part);
    }
    result.map(|()| path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pins_by_hash() {
        let path = std::env::temp_dir().join(format!("a2ui-gen-eui-{}", std::process::id()));
        std::fs::write(&path, "abc").unwrap();
        let abc = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
        verify(&path, abc).unwrap();
        assert!(matches!(verify(&path, "00"), Err(Error::Sha256 { .. })));
        std::fs::remove_file(&path).unwrap();
        assert_eq!(ModelSpec::base_0_8b().file_name, "Qwen3.5-0.8B-Q4_K_M.gguf");
    }
}
