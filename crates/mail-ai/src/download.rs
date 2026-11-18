//! Resumable, verified model downloads.
//!
//! Bytes go to `<file>.part` and are hashed as they arrive; an interrupted
//! download resumes from the partial file. Only a file whose size and SHA-256
//! match the catalog is renamed into place.

use crate::catalog::{ModelFile, ModelSpec};
use crate::{AiError, Result};
use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

pub fn file_path(dir: &Path, file: &ModelFile) -> PathBuf {
    dir.join(file.file_name)
}

/// Both files are present with the expected sizes. Content was verified when
/// the download finished; this check stays cheap for status polling.
pub fn is_installed(dir: &Path, spec: &ModelSpec) -> bool {
    [spec.weights, spec.tokenizer].iter().all(|f| is_complete(dir, f))
}

fn is_complete(dir: &Path, file: &ModelFile) -> bool {
    fs::metadata(file_path(dir, file)).map(|m| m.len() == file.bytes).unwrap_or(false)
}

/// Bytes on disk so far, complete or partial, for progress.
pub fn bytes_present(dir: &Path, spec: &ModelSpec) -> u64 {
    [spec.weights, spec.tokenizer]
        .iter()
        .map(|f| {
            fs::metadata(file_path(dir, f))
                .or_else(|_| fs::metadata(part_path(dir, f)))
                .map(|m| m.len().min(f.bytes))
                .unwrap_or(0)
        })
        .sum()
}

fn part_path(dir: &Path, file: &ModelFile) -> PathBuf {
    dir.join(format!("{}.part", file.file_name))
}

/// Remove a model's files. The tokenizer stays while another installed model
/// still uses it.
pub fn remove(dir: &Path, spec: &ModelSpec, others: &[&ModelSpec]) -> Result<()> {
    let mut files = vec![spec.weights];
    if !others.iter().any(|o| o.tokenizer.file_name == spec.tokenizer.file_name && is_installed(dir, o)) {
        files.push(spec.tokenizer);
    }
    for file in files {
        for path in [file_path(dir, &file), part_path(dir, &file)] {
            match fs::remove_file(&path) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e.into()),
            }
        }
    }
    Ok(())
}

/// Download both files of `spec` into `dir`. `progress(done, total)` is
/// called as bytes arrive; setting `cancel` stops at the next chunk and keeps
/// the partial file for a later resume.
pub fn download(
    dir: &Path,
    spec: &ModelSpec,
    cancel: &AtomicBool,
    progress: &dyn Fn(u64, u64),
) -> Result<()> {
    let unpublished = |f: &ModelFile| f.url.is_none() && !is_complete(dir, f);
    if [spec.tokenizer, spec.weights].iter().any(unpublished) {
        return Err(AiError::Download("The sorting model isn't available to download yet.".into()));
    }
    fs::create_dir_all(dir)?;
    let total = spec.download_bytes();
    let client = reqwest::blocking::Client::builder()
        .connect_timeout(Duration::from_secs(20))
        .timeout(None)
        .user_agent("Email (local mail sorting)")
        .build()
        .map_err(|e| AiError::Download(e.to_string()))?;
    let mut finished = 0;
    for file in [spec.tokenizer, spec.weights] {
        if is_complete(dir, &file) {
            finished += file.bytes;
            progress(finished, total);
            continue;
        }
        fetch(&client, dir, &file, cancel, &|done| progress(finished + done, total))?;
        finished += file.bytes;
    }
    progress(total, total);
    Ok(())
}

fn fetch(
    client: &reqwest::blocking::Client,
    dir: &Path,
    file: &ModelFile,
    cancel: &AtomicBool,
    progress: &dyn Fn(u64),
) -> Result<()> {
    let part = part_path(dir, file);
    let mut hasher = Sha256::new();
    let mut have = 0u64;
    if let Ok(mut existing) = File::open(&part) {
        let mut buffer = vec![0u8; 1 << 20];
        loop {
            let n = existing.read(&mut buffer)?;
            if n == 0 {
                break;
            }
            hasher.update(&buffer[..n]);
            have += n as u64;
        }
    }
    if have > file.bytes {
        fs::remove_file(&part)?;
        hasher = Sha256::new();
        have = 0;
    }
    if have < file.bytes {
        let url = file.url.ok_or_else(|| AiError::Download("The sorting model isn't available to download yet.".into()))?;
        let mut request = client.get(url);
        if have > 0 {
            request = request.header(reqwest::header::RANGE, format!("bytes={have}-"));
        }
        let mut response = request
            .send()
            .map_err(|e| AiError::Download(format!("Couldn't reach Hugging Face: {e}")))?;
        let status = response.status();
        if have > 0 && status == reqwest::StatusCode::OK {
            // The server ignored the range; start over.
            hasher = Sha256::new();
            have = 0;
        } else if !status.is_success() {
            return Err(AiError::Download(format!("Hugging Face answered {status}")));
        }
        let mut out = OpenOptions::new()
            .create(true)
            .write(true)
            .append(have > 0)
            .truncate(have == 0)
            .open(&part)?;
        let mut buffer = vec![0u8; 256 * 1024];
        loop {
            if cancel.load(Ordering::Relaxed) {
                out.flush()?;
                return Err(AiError::Cancelled);
            }
            let n = response
                .read(&mut buffer)
                .map_err(|e| AiError::Download(format!("Download interrupted: {e}")))?;
            if n == 0 {
                break;
            }
            if have + n as u64 > file.bytes {
                drop(out);
                let _ = fs::remove_file(&part);
                return Err(AiError::Download("The download was larger than expected".into()));
            }
            out.write_all(&buffer[..n])?;
            hasher.update(&buffer[..n]);
            have += n as u64;
            progress(have);
        }
        out.flush()?;
    }
    let digest = hasher.finalize().iter().map(|b| format!("{b:02x}")).collect::<String>();
    if have != file.bytes || digest != file.sha256 {
        let _ = fs::remove_file(&part);
        return Err(AiError::Download(
            "The downloaded file didn't match. Try again.".into(),
        ));
    }
    fs::rename(&part, file_path(dir, file))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unpublished_model_fails_before_any_network_access() {
        let spec = ModelSpec {
            id: "test",
            model_name: "Test",
            license: "MIT",
            memory_bytes: 1,
            weights: ModelFile { file_name: "w.bin", url: None, sha256: "", bytes: 4 },
            // Unroutable: reaching the network would hang or fail differently.
            tokenizer: ModelFile { file_name: "t.json", url: Some("https://10.255.255.1/t.json"), sha256: "", bytes: 4 },
        };
        let dir = std::env::temp_dir().join(format!("mail-ai-download-{}", std::process::id()));
        let err = download(&dir, &spec, &AtomicBool::new(false), &|_, _| {}).unwrap_err();
        assert!(err.to_string().contains("isn't available"), "{err}");
        assert!(!dir.exists(), "nothing is written for an unpublished model");
    }
}
