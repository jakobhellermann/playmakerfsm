use anyhow::Result;
use playmakerfsm::model::{Context, decode_fsm};
use playmakerfsm::raw::PlayMakerFSM;
use rabex::objects::pptr::PathId;
use rabex_env::scene_lookup::SceneLookup;

mod utils;

fn main() -> Result<()> {
    let env = utils::find_game("silksong")?.unwrap();

    let bundle = "scenes_scenes_scenes/tut_04.bundle";
    let path_id: PathId = 4720;

    let file = env.load_addressables_bundle_content(bundle)?;
    let scene = SceneLookup::new(file.file, &mut file.reader(), &file.env.tpk)?;
    let _obj = scene
        .lookup_path(
            &mut file.reader(),
            "States/Intro Scene/Snail Shamans Set/RestBench",
        )?
        .unwrap();

    let item = file.object_at::<PlayMakerFSM>(path_id)?.read()?;
    let mut ctx = Context::new(&file);
    let model = decode_fsm(&item.fsm, &mut ctx);

    println!("{}", serde_json::to_string_pretty(&model)?);

    Ok(())
}
