export RUSTFLAGS = -C codegen-units=1 --cfg tokio_unstable

launch: test
	strip target/release/simeis-server.exe
	cargo run --release --verbose

typst-install:
	cargo install typst-cli

typst:
	typst compile doc/manual.typ


build:
	cargo build --release

test: build_devmode
	python tests/propertybased.py

build_devmode:
	cargo build --verbose
