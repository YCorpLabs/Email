//! The models Email offers, pinned to exact Hugging Face revisions.
//!
//! Each entry is two files: the weights and the tokenizer. Sizes
//! and SHA-256 digests are checked after download; a file that does not match
//! is deleted and never loaded.

use serde::Serialize;

#[derive(Debug, Clone, Copy, Serialize)]
pub struct ModelFile {
    pub file_name: &'static str,
    /// Pinned download location. `None` until the file is published; the
    /// file can still be placed in the models directory by hand.
    pub url: Option<&'static str>,
    pub sha256: &'static str,
    pub bytes: u64,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct ModelSpec {
    pub id: &'static str,
    /// The model behind it, for people who want to know.
    pub model_name: &'static str,
    pub license: &'static str,
    /// Memory this model needs while sorting, roughly.
    pub memory_bytes: u64,
    pub weights: ModelFile,
    pub tokenizer: ModelFile,
}

impl ModelSpec {
    pub fn download_bytes(&self) -> u64 {
        self.weights.bytes + self.tokenizer.bytes
    }
}

/// multilingual-e5-small's tokenizer, unchanged by fine-tuning.
const E5_SMALL_TOKENIZER: ModelFile = ModelFile {
    file_name: "e5-small-tokenizer.json",
    url: Some("https://huggingface.co/intfloat/multilingual-e5-small/resolve/614241f622f53c4eeff9890bdc4f31cfecc418b3/tokenizer.json"),
    sha256: "0b44a9d7b51c3c62626640cda0e2c2f70fdacdc25bbbd68038369d14ebdf4c39",
    bytes: 17_082_730,
};

static CATALOG: [ModelSpec; 1] = [ModelSpec {
    id: "email-sorter-1",
    model_name: "Email sorter",
    license: "MIT",
    memory_bytes: 600_000_000,
    weights: ModelFile {
        file_name: "email-sorter-1.safetensors",
        url: Some("https://huggingface.co/YCorpLabs/email-sorter/resolve/cbfafb430c4c7abac6e442bd82b8d8658f99d0dd/email-sorter-1.safetensors"),
        sha256: "14ae1737f4e60e5d630dc3dad0b6ea2ebd963d477565e35acb498756a75b0c9a",
        bytes: 139_520_580,
    },
    tokenizer: E5_SMALL_TOKENIZER,
}];

pub fn catalog() -> &'static [ModelSpec] {
    &CATALOG
}

pub fn find_model(id: &str) -> Option<&'static ModelSpec> {
    CATALOG.iter().find(|m| m.id == id)
}

/// The model Email uses. There is one: it is small enough for every
/// supported computer (see docs/filters.md).
pub fn recommended_model() -> &'static ModelSpec {
    &CATALOG[0]
}

/// Physical memory, when the platform reports it.
pub fn system_memory_bytes() -> Option<u64> {
    #[cfg(target_os = "macos")]
    {
        let mut value: u64 = 0;
        let mut size = std::mem::size_of::<u64>();
        let name = c"hw.memsize";
        // SAFETY: hw.memsize is a u64 sysctl; buffer and size match.
        let rc = unsafe {
            libc::sysctlbyname(
                name.as_ptr(),
                (&mut value as *mut u64).cast(),
                &mut size,
                std::ptr::null_mut(),
                0,
            )
        };
        (rc == 0 && value > 0).then_some(value)
    }
    #[cfg(target_os = "linux")]
    {
        let text = std::fs::read_to_string("/proc/meminfo").ok()?;
        let line = text.lines().find(|l| l.starts_with("MemTotal:"))?;
        let kib: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
        Some(kib * 1024)
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_entries_are_pinned_and_hashed() {
        for spec in catalog() {
            for file in [spec.weights, spec.tokenizer] {
                if let Some(url) = file.url {
                    assert!(url.starts_with("https://huggingface.co/"));
                    // A 40-hex revision, never a moving branch.
                    let revision = url.split("/resolve/").nth(1).unwrap().split('/').next().unwrap();
                    assert_eq!(revision.len(), 40, "{url}");
                    assert!(revision.chars().all(|c| c.is_ascii_hexdigit()), "{url}");
                }
                assert_eq!(file.sha256.len(), 64);
                assert!(file.bytes > 0);
            }
        }
    }

}
