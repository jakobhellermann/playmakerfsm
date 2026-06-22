//! Decoder-coverage census over *every* PlayMakerFSM in the game (indexed by `out/fsms_hkss.json`), not just one.
//!
//! For each FSM it decodes via [`playmakerfsm::model`] and tallies, per ActionData parameter type:
//! how often it's used and how often the model fails to decode it (`ParamValue::Raw`). That answers
//! "which param types actually occur in the corpus" and surfaces decoder gaps (unhandled types, bad
//! indices) and any `ParamDataType` ordinal not in `PARAM_TYPES`.

use anyhow::Result;
use playmakerfsm::model::{Context, ParamValue, decode_fsm, ptype};
use playmakerfsm::raw::*;
use rabex::objects::pptr::PathId;
use serde::Deserialize;
use std::collections::BTreeMap;

mod utils;

#[derive(Deserialize)]
struct Index {
    referrers: Vec<Referrer>,
}
#[derive(Deserialize)]
struct Referrer {
    file: String,
    path_id: PathId,
}

#[derive(Default)]
struct Stat {
    total: usize,
    raw: usize,
}

fn main() -> Result<()> {
    let env = utils::find_game("silksong")?.unwrap();

    let index: Index = serde_json::from_str(&std::fs::read_to_string("out/fsms_hkss.json")?)?;

    // group by bundle so each is loaded & parsed only once
    let mut by_file: BTreeMap<&str, Vec<PathId>> = BTreeMap::new();
    for r in &index.referrers {
        by_file.entry(&r.file).or_default().push(r.path_id);
    }

    let mut per_type: BTreeMap<&'static str, Stat> = BTreeMap::new();
    let mut unknown_ords: BTreeMap<i32, usize> = BTreeMap::new();
    let (mut bundles_ok, mut bundles_err) = (0usize, 0usize);
    let (mut fsms_ok, mut fsms_err) = (0usize, 0usize);

    let n_files = by_file.len();
    for (i, (file, ids)) in by_file.iter().enumerate() {
        if i % 50 == 0 {
            eprintln!("[{i}/{n_files}] {file}");
        }
        let handle = match env.load_addressables_bundle_content(file) {
            Ok(h) => h,
            Err(_) => {
                bundles_err += 1;
                continue;
            }
        };
        bundles_ok += 1;
        let mut ctx = Context::new(&handle);
        for &id in ids {
            let fsm = match handle.object_at::<PlayMakerFSM>(id).and_then(|o| o.read()) {
                Ok(f) => f,
                Err(_) => {
                    fsms_err += 1;
                    continue;
                }
            };
            fsms_ok += 1;

            let model = decode_fsm(&fsm.fsm, &mut ctx);
            for state in &model.states {
                for action in &state.actions {
                    for p in &action.params {
                        let stat = per_type.entry(p.type_name).or_default();
                        stat.total += 1;
                        if matches!(p.value, ParamValue::Raw(_)) {
                            stat.raw += 1;
                        }
                    }
                }
            }
            // numeric ordinals the table doesn't cover (model renders these as type_name "?")
            for state in &fsm.fsm.states {
                for &dt in &state.actionData.paramDataType {
                    if ptype(dt) == "?" {
                        *unknown_ords.entry(dt).or_default() += 1;
                    }
                }
            }
        }
    }

    // ── report ──
    let total_params: usize = per_type.values().map(|s| s.total).sum();
    let total_raw: usize = per_type.values().map(|s| s.raw).sum();
    println!("\n=== PlayMakerFSM decoder coverage over out/fsms_hkss.json ===");
    println!("bundles : {bundles_ok} ok, {bundles_err} failed to load");
    println!("FSMs    : {fsms_ok} ok, {fsms_err} failed to read");
    println!(
        "params  : {total_params} total, {total_raw} undecoded ({:.2}%)\n",
        100.0 * total_raw as f64 / total_params.max(1) as f64
    );

    let mut rows: Vec<_> = per_type.iter().collect();
    rows.sort_by_key(|r| std::cmp::Reverse(r.1.total));
    println!("{:<22}{:>10}{:>12}", "param type", "count", "undecoded");
    for (ty, stat) in rows {
        let flag = if stat.raw == 0 {
            String::new()
        } else if stat.raw == stat.total {
            "   ← never decoded".into()
        } else {
            format!("   ← {} undecoded", stat.raw)
        };
        let undec = if stat.raw == 0 {
            "-".to_string()
        } else {
            stat.raw.to_string()
        };
        println!("{ty:<22}{:>10}{undec:>12}{flag}", stat.total);
    }

    println!();
    if unknown_ords.is_empty() {
        println!("unknown ParamDataType ordinals: none ✓");
    } else {
        println!("unknown ParamDataType ordinals (not in PARAM_TYPES):");
        for (ord, count) in &unknown_ords {
            println!("  ordinal {ord}: {count} occurrences");
        }
    }
    Ok(())
}
