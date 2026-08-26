//! Enumerate the serialized files of a game the way the FSM index labels them.
//!
//! FSMs live both in plain serialized files (`levelN`, `*.assets`) and inside
//! Addressables bundles, which each hold several serialized files. A bundle's
//! files are labelled with the bundle path rather than their internal archive
//! path: that is what the scene lookup keys on and what the UI shows.

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
    /// The name the index and the UI use for the files this source yields.
    pub fn label(&self) -> String {
        match self {
            Source::Plain(path) | Source::Bundle(path) => path.to_string_lossy().into_owned(),
        }
    }
}

/// Every source of the game, plain files first. Callers are free to run them in
/// parallel; each [`for_each_file`] call is independent.
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

/// Call `f` for every serialized file of `source`, with the label to record it under.
pub fn for_each_file<R: EnvResolver, P: TypeTreeProvider>(
    env: &Environment<R, P>,
    source: &Source,
    mut f: impl FnMut(&str, &SerializedFileHandle<'_, R, P>) -> Result<()>,
) -> Result<()> {
    let label = source.label();
    match source {
        Source::Plain(path) => {
            let handle = env.load_serialized(path)?;
            f(&label, &handle)
        }
        Source::Bundle(path) => {
            let bundle = env.load_addressables_bundle(path)?;
            let bundle_id = bundle
                .main_serializedfile()
                .map(|file| file.path.clone())
                .with_context(|| format!("{label} has no main serialized file"))?;
            for entry in bundle.serialized_files() {
                let archive_path = ArchivePath::new(&bundle_id, &entry.path);
                let data = bundle.read_at_entry(entry)?;
                let mut serialized = SerializedFile::from_reader(&mut Cursor::new(&data))?;
                serialized
                    .m_UnityVersion
                    .get_or_insert(env.unity_version()?.clone());
                let handle =
                    env.insert_cache(archive_path.to_string().into(), serialized, data.into());
                f(&label, &handle)?;
            }
            Ok(())
        }
    }
}
