use std::{
    env::VarError,
    path::{Path, PathBuf},
};

use shellexpand::path::LookupError;

pub struct ExpandedPath(PathBuf);

impl ExpandedPath {
    pub fn new(path: impl AsRef<Path>) -> Result<Self, LookupError<VarError>> {
        let expanded_path = shellexpand::path::full(&path)?;
        Ok(Self(expanded_path.into()))
    }

    pub fn as_path(&self) -> &Path {
        self.0.as_path()
    }
}
