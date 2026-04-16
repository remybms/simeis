export RUSTFLAGS = -C code-model=kernel -C codegen-units=1 --cfg tokio_unstable

launch: test
	strip target/release/simeis-server.exe
	cargo run --release --verbose

build:
	cargo build --verbose
	typst compile doc/manual.typ

test: build
	python tests/propertybased.py