export RUSTFLAGS = -C code-model=kernel -C codegen-units=1 --cfg tokio_unstable

launch: test
	strip target/release/simeis-server.exe
	cargo run --release --verbose

typst:
	cargo install typst-cli

build:
	cargo build --release
	typst compile doc/manual.typ

test: build_devmode
	python tests/propertybased.py

build_devmode:
	cargo build --verbose
