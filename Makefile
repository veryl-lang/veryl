VERSION = $(patsubst "%",%, $(word 3, $(shell grep version ./crates/veryl/Cargo.toml)))
BUILD_TIME = $(shell date +"%Y/%m/%d %H:%M:%S")
GIT_REVISION = $(shell git log -1 --format="%h")
RUST_VERSION = $(word 2, $(shell rustc -V))
LONG_VERSION = "$(VERSION) ( rev: $(GIT_REVISION), rustc: $(RUST_VERSION), build at: $(BUILD_TIME) )"
ZIP_NAME = veryl
BIN_NAMES = veryl veryl-ls

export LONG_VERSION

.PHONY: all test clean lint release_lnx release_win release_mac

all:
	cargo build

test:
	cargo test

clean:
	cargo clean

lint:
	cargo fmt --check
	cargo clippy -- -D warnings

release_lnx:
	cargo build --locked --release --target=x86_64-unknown-linux-musl $(addprefix --bin , ${BIN_NAMES})
	cd ./support/sourcemap-resolver; cargo build --locked --release --target=x86_64-unknown-linux-musl
	zip -j ${ZIP_NAME}-x86_64-linux.zip $(addprefix target/x86_64-unknown-linux-musl/release/, ${BIN_NAMES}) \
		                                ./support/sourcemap-resolver/target/x86_64-unknown-linux-musl/release/sourcemap-resolver

release_win:
	cargo build --locked --release --target=x86_64-pc-windows-msvc $(addprefix --bin , ${BIN_NAMES})
	cd ./support/sourcemap-resolver && cargo build --locked --release --target=x86_64-pc-windows-msvc
	mv -v $(addsuffix .exe, $(addprefix target/x86_64-pc-windows-msvc/release/, ${BIN_NAMES})) ./
	mv -v ./support/sourcemap-resolver/target/x86_64-pc-windows-msvc/release/sourcemap-resolver.exe ./
	7z a ${ZIP_NAME}-x86_64-windows.zip $(addsuffix .exe, ${BIN_NAMES}) sourcemap-resolver.exe

release_mac:
	cargo build --locked --release --target=x86_64-apple-darwin $(addprefix --bin , ${BIN_NAMES})
	cargo build --locked --release --target=aarch64-apple-darwin $(addprefix --bin , ${BIN_NAMES})
	cd ./support/sourcemap-resolver; cargo build --locked --release --target=x86_64-apple-darwin
	cd ./support/sourcemap-resolver; cargo build --locked --release --target=aarch64-apple-darwin
	zip -j ${ZIP_NAME}-x86_64-mac.zip $(addprefix target/x86_64-apple-darwin/release/, ${BIN_NAMES}) \
		                              ./support/sourcemap-resolver/target/x86_64-apple-darwin/release/sourcemap-resolver
	zip -j ${ZIP_NAME}-aarch64-mac.zip $(addprefix target/aarch64-apple-darwin/release/, ${BIN_NAMES}) \
		                               ./support/sourcemap-resolver/target/aarch64-apple-darwin/release/sourcemap-resolver

release_rpm:
	mkdir -p target
	cargo rpm build
	cp target/x86_64-unknown-linux-musl/release/rpmbuild/RPMS/x86_64/* ./

watch:
	cargo watch -i crates/parser/src/generated -x test -x bench

install:
	cargo install --path crates/languageserver
	cargo install --path crates/veryl
	cargo install --path crates/mdbook

gen_sv:
	cargo run --bin veryl -- build

flamegraph:
	cargo bench --bench benchmark -- --profile-time=5
