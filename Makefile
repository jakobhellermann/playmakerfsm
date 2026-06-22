src/generated: unity2rust.toml
	@tmp=$$(mktemp); \
	cargo run --release -q --manifest-path ../rabex-env/Cargo.toml --example unity2rust unity2rust.toml > "$$tmp" \
	&& mv "$$tmp" src/generated.rs

out/fsms_hk.json:
	@mkdir -p out
	rabex --steam-game 'Hollow Knight' file globalgamemanagers.assets object PlayMakerFSM references --format json > out/fsms_hk.json

out/fsms_hkss.json:
	@mkdir -p out
	rabex --steam-game silk bundle 94696d22b6ed0a74097d1bd58feb4dce_monoscripts.bundle file object PlayMakerFSM references --format json > out/fsms_hkss.json

.PHONY: content content-hk content-ss
content: content-hk content-ss

content-hk: out/fsms_hk.json
	cargo run --release -q --example content_index -- hk

content-ss: out/fsms_hkss.json
	cargo run --release -q --example content_index -- silksong
