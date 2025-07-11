PKG_VERSION = $(patsubst "%",%, $(word 3, $(shell grep version ./crates/veryl/Cargo.toml)))
BUILD_DATE = $(shell date +"%Y-%m-%d")
GIT_REVISION = $(shell git log -1 --format="%h")
CHANNEL ?=
VERSION = $(PKG_VERSION)$(CHANNEL) ($(GIT_REVISION) $(BUILD_DATE))
ZIP_NAME = veryl
BIN_NAMES = veryl veryl-ls

export VERSION

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
	cargo build --locked --release --target=x86_64-unknown-linux-musl --manifest-path ./support/sourcemap-resolver/Cargo.toml
	zip -j ${ZIP_NAME}-x86_64-linux.zip $(addprefix target/x86_64-unknown-linux-musl/release/, ${BIN_NAMES}) \
		                                ./support/sourcemap-resolver/target/x86_64-unknown-linux-musl/release/sourcemap-resolver

release_lnx_aarch64:
	cargo build --locked --release --target=aarch64-unknown-linux-musl $(addprefix --bin , ${BIN_NAMES})
	cargo build --locked --release --target=aarch64-unknown-linux-musl --manifest-path ./support/sourcemap-resolver/Cargo.toml
	zip -j ${ZIP_NAME}-aarch64-linux.zip $(addprefix target/aarch64-unknown-linux-musl/release/, ${BIN_NAMES}) \
		                                ./support/sourcemap-resolver/target/aarch64-unknown-linux-musl/release/sourcemap-resolver

release_win:
	cargo build --locked --release --target=x86_64-pc-windows-msvc $(addprefix --bin , ${BIN_NAMES})
	cargo build --locked --release --target=x86_64-pc-windows-msvc --manifest-path ./support/sourcemap-resolver/Cargo.toml
	mv -v $(addsuffix .exe, $(addprefix target/x86_64-pc-windows-msvc/release/, ${BIN_NAMES})) ./
	mv -v ./support/sourcemap-resolver/target/x86_64-pc-windows-msvc/release/sourcemap-resolver.exe ./
	7z a ${ZIP_NAME}-x86_64-windows.zip $(addsuffix .exe, ${BIN_NAMES}) sourcemap-resolver.exe

release_mac:
	cargo build --locked --release --target=x86_64-apple-darwin  $(addprefix --bin , ${BIN_NAMES})
	cargo build --locked --release --target=aarch64-apple-darwin $(addprefix --bin , ${BIN_NAMES})
	cargo build --locked --release --target=x86_64-apple-darwin  --manifest-path ./support/sourcemap-resolver/Cargo.toml
	cargo build --locked --release --target=aarch64-apple-darwin --manifest-path ./support/sourcemap-resolver/Cargo.toml
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
	verylup install local
	cargo install --profile release-verylup --path crates/mdbook

gen_sv:
	cargo run --bin veryl -- build

fmt_veryl:
	cargo run --bin veryl -- fmt

flamegraph:
	cargo bench --bench benchmark -- --profile-time=5
