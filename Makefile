src/generated: unity2rust.toml
	@tmp=$$(mktemp); \
	cargo run --release -q --manifest-path ../rabex-env/Cargo.toml --example unity2rust unity2rust.toml > "$$tmp" \
	&& mv "$$tmp" src/generated.rs

out/fsms_hk.json:
	@mkdir -p out
	rabex --steam-game 'Hollow Knight' file globalgamemanagers.assets object PlayMakerFSM references --format json > out/fsms_hk.json

out/fsms_hkss.json:
	@mkdir -p out
	rabex --steam-game 'Silksong' file globalgamemanagers.assets object PlayMakerFSM references --format json > out/fsms_hkss.json
