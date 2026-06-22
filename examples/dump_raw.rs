//! Throwaway: dump RAW ActionData (pre-decode), incl. dataVersion + stringParams, for a given
//! bundle/path_id/state. Used to investigate where FsmEvent names actually live.
use anyhow::Result;
use playmakerfsm::model::ptype;
use playmakerfsm::raw::*;
use rabex::objects::pptr::PathId;

mod utils;

fn main() -> Result<()> {
    let env = utils::find_game("silksong")?.unwrap();

    let mut dump = |bundle: &str, path_id: PathId, want: &str| -> Result<()> {
        let file = env.load_addressables_bundle_content(bundle)?;
        let fsm = file.object_at::<PlayMakerFSM>(path_id)?.read()?;
        let fsm = &fsm.fsm;
        println!("FSM {:?}  dataVersion={}", fsm.name, fsm.dataVersion);
        for state in &fsm.states {
            if state.name != want {
                continue;
            }
            let ad = &state.actionData;
            println!("state {:?}", state.name);
            println!("  actionNames = {:?}", ad.actionNames);
            println!("  stringParams = {:?}", ad.stringParams);
            println!("  byteData.len = {}", ad.byteData.len());
            for j in 0..ad.paramName.len() {
                let name = &ad.paramName[j];
                let dt = ad.paramDataType[j];
                let pos = ad.paramDataPos[j] as usize;
                let size = ad.paramByteDataSize.get(j).copied().unwrap_or(0) as usize;
                let tn = ptype(dt);
                println!("  [{j:2}] name={name:<16} type={tn:<14} pos={pos:<3} size={size:<3}");
            }
        }
        Ok(())
    };

    // "Control" boss golem FSM with the PlayerDataBoolTest in state "Meet?"
    dump(
        "scenes_scenes_scenes/bone_east_08_boss_golem.bundle",
        253,
        "Meet?",
    )?;
    println!("----");
    // "Check Corner" we earlier (maybe wrongly) called dead
    dump("scenes_scenes_scenes/clover_10.bundle", 12234, "Check")?;
    Ok(())
}
