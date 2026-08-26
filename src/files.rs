//! Enumerate a game's serialized files: the plain ones (`levelN`, `*.assets`)
//! and those inside Addressables bundles, which each hold several.

use std::io::Cursor;
use std::path::PathBuf;

use anyhow::{Context as _, Result};
use rabex_env::Environment;
use rabex_env::addressables::ArchivePath;
use rabex_env::handle::SerializedFileHandle;
use rabex_env::rabex::files::SerializedFile;
use rabex_env::rabex::typetree::TypeTreeProvider;
use rabex_env::resolver::EnvResolver;

/// One unit of work: either a plain serialized file or a bundle holding several.
pub enum Source {
    Plain(PathBuf),
    Bundle(PathBuf),
}

impl Source {
    /// The bundle path for a bundle, the file path otherwise — never a bundle's
    /// internal archive path.
    pub fn label(&self) -> String {
        match self {
            Source::Plain(path) | Source::Bundle(path) => path.to_string_lossy().into_owned(),
        }
    }

    /// Call `f` for each serialized file this source holds.
    pub fn for_each_file<R: EnvResolver, P: TypeTreeProvider>(
        &self,
        env: &Environment<R, P>,
        mut f: impl FnMut(&SerializedFileHandle<'_, R, P>) -> Result<()>,
    ) -> Result<()> {
        match self {
            Source::Plain(path) => {
                let handle = env.load_serialized(path)?;
                f(&handle)
            }
            Source::Bundle(path) => {
                let bundle = env.load_addressables_bundle(path)?;
                let bundle_id = bundle
                    .main_serializedfile()
                    .map(|file| file.path.clone())
                    .with_context(|| format!("{} has no main serialized file", self.label()))?;
                for entry in bundle.serialized_files() {
                    let archive_path = ArchivePath::new(&bundle_id, &entry.path);
                    let data = bundle.read_at_entry(entry)?;
                    let mut serialized = SerializedFile::from_reader(&mut Cursor::new(&data))?;
                    serialized
                        .m_UnityVersion
                        .get_or_insert(env.unity_version()?.clone());
                    let handle =
                        env.insert_cache(archive_path.to_string().into(), serialized, data.into());
                    f(&handle)?;
                }
                Ok(())
            }
        }
    }
}

/// Every source of the game, plain files first.
pub fn sources<R: EnvResolver, P: TypeTreeProvider>(
    env: &Environment<R, P>,
) -> Result<Vec<Source>> {
    let plain = env.game_files.serialized_files()?;
    let bundles = env.addressables_bundles()?;
    Ok(plain
        .into_iter()
        .map(Source::Plain)
        .chain(bundles.into_iter().map(Source::Bundle))
        .collect())
}
