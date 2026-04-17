export RUSTFLAGS = -C code-model=kernel -C codegen-units=1 --cfg tokio_unstable

launch: test
	strip target/release/simeis-server.exe
	cargo run --release --verbose

build:
	cargo build --release
	cargo install typst-cli
	typst compile doc/manual.typ

test: build_devmode
	python tests/propertybased.py

build_devmode:
	cargo build --verbose
