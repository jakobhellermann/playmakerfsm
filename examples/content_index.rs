//! Serialize every FSM of a game into a content-addressed store: each distinct model is written once
//! to `out/<game>/content/<hash>.json`, and `out/<game>/index.json` maps every reference
//! (scene file, path_id, FSM name, owning GameObject) to its content hash. Identical FSMs (templates
//! reused across scenes) collapse to one file.
//!
//! Usage: `cargo run --example content_index -- [hk|silksong]` (default `hk`). Reads the referrer
//! list produced by the matching `make out/fsms_*.json` target.

use anyhow::Result;
use playmakerfsm::model::{Context, decode_fsm};
use playmakerfsm::raw::*;
use rabex::objects::ClassId;
use rabex::objects::pptr::PathId;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::hash_map::DefaultHasher;
use std::collections::{BTreeMap, BTreeSet};
use std::hash::{Hash, Hasher};
use std::sync::Mutex;

mod utils;

#[derive(Deserialize)]
struct Input {
    referrers: Vec<Referrer>,
}
#[derive(Deserialize)]
struct Referrer {
    file: String,
    path_id: PathId,
    /// hierarchy path of the component, e.g. `Some/Game/Object@PlayMakerFSM`
    #[serde(default)]
    label: String,
}

#[derive(Serialize)]
struct Entry {
    file: String,
    path_id: PathId,
    name: String,
    /// owning GameObject hierarchy path (the label with the `@Component` suffix stripped)
    game_object: String,
    hash: String,
}

/// `Some/Game/Object@PlayMakerFSM:1` -> `Some/Game/Object`
fn game_object_path(label: &str) -> &str {
    label.rsplit_once('@').map_or(label, |(go, _)| go)
}

fn main() -> Result<()> {
    let start = std::time::Instant::now();
    let arg = std::env::args().nth(1).unwrap_or_else(|| "hk".to_string());
    // `from_bundle`: Silksong FSMs live in addressables bundles, Hollow Knight's in scene files.
    let (game, input_path, out_dir, from_bundle) = match arg.as_str() {
        "hk" | "hollow knight" => ("Hollow Knight", "out/fsms_hk.json", "out/hk", false),
        "ss" | "silksong" => ("Silksong", "out/fsms_hkss.json", "out/ss", true),
        other => anyhow::bail!("unknown game {other:?}, expected `hk` or `silksong`"),
    };

    let env = utils::find_game(game)?.unwrap();
    let input: Input = serde_json::from_str(&std::fs::read_to_string(input_path)?)?;

    let mut by_file: BTreeMap<&str, Vec<(PathId, &str)>> = BTreeMap::new();
    for r in &input.referrers {
        by_file
            .entry(&r.file)
            .or_default()
            .push((r.path_id, &r.label));
    }
    // a Vec splits across rayon workers far better than a BTreeMap
    let by_file: Vec<(&str, Vec<(PathId, &str)>)> = by_file.into_iter().collect();

    let content_dir = std::path::Path::new(out_dir).join("content");
    std::fs::create_dir_all(&content_dir)?;

    let written: Mutex<BTreeSet<String>> = Mutex::new(BTreeSet::new());
    let entries: Mutex<Vec<Entry>> = Mutex::new(Vec::new());

    by_file.par_iter().for_each(|(file, ids)| {
        let loaded = if from_bundle {
            env.load_addressables_bundle_content(file)
        } else {
            env.load_serialized(file)
        };
        let Ok(handle) = loaded else {
            return;
        };
        let mut ctx = Context::new(&handle);
        let mut local: Vec<Entry> = Vec::with_capacity(ids.len());
        for &(id, label) in ids {
            let Ok(obj) = handle.object_at::<PlayMakerFSM>(id) else {
                continue;
            };
            let ClassId::MonoBehaviour = obj.class_id() else {
                continue;
            };
            let Ok(fsm) = obj.read() else {
                continue;
            };
            let model = decode_fsm(&fsm.fsm, &mut ctx);
            let Ok(json) = serde_json::to_vec(&model) else {
                continue;
            };

            let mut hasher = DefaultHasher::new();
            json.hash(&mut hasher);
            let hash = format!("{:016x}", hasher.finish());

            // first writer of a hash wins; the lock guards only the set, not the file IO
            let is_new = written.lock().unwrap().insert(hash.clone());
            if is_new {
                let _ = std::fs::write(content_dir.join(format!("{hash}.json")), &json);
            }
            local.push(Entry {
                file: file.to_string(),
                path_id: id,
                name: model.name.to_string(),
                game_object: game_object_path(label).to_string(),
                hash,
            });
        }
        entries.lock().unwrap().extend(local);
    });

    let mut entries = entries.into_inner().unwrap();
    let written = written.into_inner().unwrap();
    entries.sort_by(|a, b| (&a.name, &a.file, a.path_id).cmp(&(&b.name, &b.file, b.path_id)));
    let index_path = std::path::Path::new(out_dir).join("index.json");
    std::fs::write(&index_path, serde_json::to_vec_pretty(&entries)?)?;

    // Prune content files left over from earlier runs: the store is content-addressed, so any
    // `<hash>.json` not (re)written this run is orphaned (no index entry points at it).
    let mut pruned = 0usize;
    for entry in std::fs::read_dir(&content_dir)?.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let referenced = path
            .file_stem()
            .and_then(|s| s.to_str())
            .is_some_and(|stem| written.contains(stem));
        if !referenced {
            std::fs::remove_file(&path)?;
            pruned += 1;
        }
    }

    println!(
        "{} fsms -> {} distinct models in {}/ ({pruned} stale pruned), index in {} ({:.1}s)",
        entries.len(),
        written.len(),
        content_dir.display(),
        index_path.display(),
        start.elapsed().as_secs_f64()
    );
    Ok(())
}
