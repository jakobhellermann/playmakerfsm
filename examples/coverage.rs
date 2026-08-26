//! Decoder-coverage census over every FSM of a game, not just one.
//!
//! For each FSM it decodes via [`playmakerfsm::model`] and tallies, per ActionData parameter type:
//! how often it's used and how often the model fails to decode it (`ParamValue::Raw`). That answers
//! "which param types actually occur in the corpus" and surfaces decoder gaps (unhandled types, bad
//! indices) and any `ParamDataType` ordinal not in `PARAM_TYPES`.
//!
//! Usage: `cargo run --release --example coverage -- [game name filter]`

use std::collections::BTreeMap;
use std::sync::Mutex;

use anyhow::Result;
use playmakerfsm::files;
use playmakerfsm::model::{Context, ParamValue, decode_fsm, ptype};
use playmakerfsm::raw::*;
use rabex_env::rabex::typetree::TypeTreeProvider;
use rabex_env::resolver::EnvResolver;
use rabex_env::unity::types::MonoBehaviour;
use rayon::prelude::*;

mod utils;

#[derive(Default)]
struct Stat {
    total: usize,
    raw: usize,
}

#[derive(Default)]
struct Census {
    per_type: BTreeMap<String, Stat>,
    unknown_ords: BTreeMap<i32, usize>,
    fsms: usize,
}

impl Census {
    fn add<R: EnvResolver, P: TypeTreeProvider>(&mut self, fsm: &Fsm, ctx: &mut Context<'_, R, P>) {
        self.fsms += 1;
        let model = decode_fsm(fsm, ctx);
        for state in &model.states {
            for action in &state.actions {
                for param in &action.params {
                    let stat = self
                        .per_type
                        .entry(param.type_name.to_string())
                        .or_default();
                    stat.total += 1;
                    if matches!(param.value, ParamValue::Raw(_)) {
                        stat.raw += 1;
                    }
                }
            }
        }
        // numeric ordinals the table doesn't cover (the model names these "?")
        for state in &fsm.states {
            for &dt in &state.actionData.paramDataType {
                if ptype(dt) == "?" {
                    *self.unknown_ords.entry(dt).or_default() += 1;
                }
            }
        }
    }

    fn merge(&mut self, other: Census) {
        self.fsms += other.fsms;
        for (ty, stat) in other.per_type {
            let mine = self.per_type.entry(ty).or_default();
            mine.total += stat.total;
            mine.raw += stat.raw;
        }
        for (ord, count) in other.unknown_ords {
            *self.unknown_ords.entry(ord).or_default() += count;
        }
    }
}

fn main() -> Result<()> {
    let filter = std::env::args().nth(1).unwrap_or_else(|| "silksong".into());
    let env = utils::find_game(&filter)?.unwrap();

    let sources = files::sources(&env)?;
    eprintln!("{filter}: {} files and bundles", sources.len());

    let census = Mutex::new(Census::default());
    sources.par_iter().try_for_each(|source| -> Result<()> {
        let mut local = Census::default();
        source.for_each_file(&env, |handle| {
            let mut ctx = Context::new(handle);
            // Components hold the FSM they run; templates hold FSMs that only a
            // RunFSM action instantiates, and no component data covers those.
            for mb in handle
                .scripts::<MonoBehaviour>("PlayMakerFSM")
                .into_iter()
                .flatten()
            {
                let component = handle.object_at::<PlayMakerFSM>(mb.path_id())?.read()?;
                local.add(&component.fsm, &mut ctx);
            }
            for mb in handle
                .scripts::<MonoBehaviour>("FsmTemplate")
                .into_iter()
                .flatten()
            {
                let template = handle.object_at::<FsmTemplate>(mb.path_id())?.read()?;
                local.add(&template.fsm, &mut ctx);
            }
            Ok(())
        })?;
        census.lock().unwrap().merge(local);
        Ok(())
    })?;
    let census = census.into_inner().unwrap();

    let total_params: usize = census.per_type.values().map(|s| s.total).sum();
    let total_raw: usize = census.per_type.values().map(|s| s.raw).sum();
    println!("\n=== PlayMakerFSM decoder coverage over {filter} ===");
    println!("FSMs    : {}", census.fsms);
    println!(
        "params  : {total_params} total, {total_raw} undecoded ({:.2}%)\n",
        100.0 * total_raw as f64 / total_params.max(1) as f64
    );

    let mut rows: Vec<_> = census.per_type.iter().collect();
    rows.sort_by_key(|row| std::cmp::Reverse(row.1.total));
    println!("{:<22}{:>10}{:>12}", "param type", "count", "undecoded");
    for (ty, stat) in rows {
        let undecoded = match stat.raw {
            0 => "-".to_string(),
            raw => raw.to_string(),
        };
        let flag = match stat.raw {
            0 => String::new(),
            raw if raw == stat.total => "   ← never decoded".into(),
            raw => format!("   ← {raw} undecoded"),
        };
        println!("{ty:<22}{:>10}{undecoded:>12}{flag}", stat.total);
    }

    println!();
    if census.unknown_ords.is_empty() {
        println!("unknown ParamDataType ordinals: none");
    } else {
        println!("unknown ParamDataType ordinals (not in PARAM_TYPES):");
        for (ord, count) in &census.unknown_ords {
            println!("  ordinal {ord}: {count} occurrences");
        }
    }
    Ok(())
}
