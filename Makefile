src/generated: unity2rust.toml
	@tmp=$$(mktemp); \
	cargo run --release -q --manifest-path ../rabex-env/Cargo.toml --example unity2rust unity2rust.toml > "$$tmp" \
	&& mv "$$tmp" src/generated.rs
