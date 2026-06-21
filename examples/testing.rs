use anyhow::Result;
use playmakerfsm::raw::PlayMakerFSM;
use rabex::{
    objects::pptr::PathId, tpk::TpkTypeTreeBlob, typetree::typetree_cache::sync::TypeTreeCache,
};
use rabex_env::{Environment, scene_lookup::SceneLookup};

fn main() -> Result<()> {
    let path = "/home/jakob/.steamapps/Hollow Knight Silksong";
    let tpk = TypeTreeCache::new(TpkTypeTreeBlob::embedded());
    let env = Environment::new_in(path, tpk)?;

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

    let object = file.object_at::<PlayMakerFSM>(path_id)?;
    let item = object.read()?;

    println!("{:#?}", item);

    Ok(())
}
