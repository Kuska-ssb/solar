use sha2::Digest;
use sha2::Sha256;
use std::fs::File;
use std::io::Result;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

pub struct BlobStorage {
    path: Option<PathBuf>,
}

impl Default for BlobStorage {
    fn default() -> Self {
        Self { path: None }
    }
}

trait ToBlobHashId {
    fn blob_hash_id(&self) -> String;
}

impl ToBlobHashId for &[u8] {
    fn blob_hash_id(&self) -> String {
        let mut hasher = Sha256::new();
        hasher.input(self);
        format!("&{}.sha256", base64::encode(&hasher.result()))
    }
}

impl BlobStorage {
    pub fn open(&mut self, path: PathBuf) {
        self.path = Some(path);
    }
    fn path_of(&self, id: &str) -> PathBuf {
        let id = id.replace("&", "").replace("/", "_");
        [self.path.as_ref().unwrap(), Path::new(&id)]
            .iter()
            .collect()
    }
    pub fn size_of(&self, id: &str) -> Result<Option<u64>> {
        if let Ok(metadata) = std::fs::metadata(self.path_of(id)) {
            Ok(Some(metadata.len()))
        } else {
            Ok(None)
        }
    }
    pub fn insert<D: AsRef<[u8]>>(&self, content: D) -> Result<String> {
        let id = content.as_ref().blob_hash_id();
        File::create(self.path_of(&id))?.write_all(content.as_ref())?;
        Ok(id)
    }
    pub fn get(&self, id: &str) -> Result<Vec<u8>> {
        let mut file = File::open(self.path_of(id))?;
        let mut content = Vec::with_capacity(file.metadata()?.len() as usize);
        file.read_to_end(&mut content)?;
        Ok(content)
    }
}
