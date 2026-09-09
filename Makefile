PKG_VERSION = $(patsubst "%",%, $(word 3, $(shell grep version ./crates/veryl/Cargo.toml)))
BUILD_DATE = $(shell date +"%Y-%m-%d")
GIT_REVISION = $(shell git log -1 --format="%h")
CHANNEL ?=
VERSION = $(PKG_VERSION)$(CHANNEL) ($(GIT_REVISION) $(BUILD_DATE))
ZIP_NAME = veryl
BIN_NAMES = veryl veryl-ls

export VERSION

.PHONY: all test clean lint build_lnx build_lnx_aarch64 release_lnx release_lnx_aarch64 release_win release_mac

all:
	cargo build

test:
	cargo test

clean:
	cargo clean

lint:
	cargo fmt --check
	cargo clippy -- -D warnings

# The published binaries must stay dynamically linked: a static binary carries
# no dynamic loader, so it can neither dlopen a native verification component
# nor load the object the AOT-C backend compiles. Building is split from
# packaging because the release workflow runs it in an old-glibc container;
# running it on the host would raise the baseline to whatever the host has.
build_lnx:
	cargo build --locked --release --target=x86_64-unknown-linux-gnu $(addprefix --bin , ${BIN_NAMES})
	cargo build --locked --release --target=x86_64-unknown-linux-gnu --manifest-path ./support/sourcemap-resolver/Cargo.toml

release_lnx:
	zip -j ${ZIP_NAME}-x86_64-linux.zip $(addprefix target/x86_64-unknown-linux-gnu/release/, ${BIN_NAMES}) \
		                                ./support/sourcemap-resolver/target/x86_64-unknown-linux-gnu/release/sourcemap-resolver

build_lnx_aarch64:
	cargo build --locked --release --target=aarch64-unknown-linux-gnu $(addprefix --bin , ${BIN_NAMES})
	cargo build --locked --release --target=aarch64-unknown-linux-gnu --manifest-path ./support/sourcemap-resolver/Cargo.toml

release_lnx_aarch64:
	zip -j ${ZIP_NAME}-aarch64-linux.zip $(addprefix target/aarch64-unknown-linux-gnu/release/, ${BIN_NAMES}) \
		                                 ./support/sourcemap-resolver/target/aarch64-unknown-linux-gnu/release/sourcemap-resolver

release_win:
	cargo build --locked --release --target=x86_64-pc-windows-msvc $(addprefix --bin , ${BIN_NAMES})
	cargo build --locked --release --target=x86_64-pc-windows-msvc --manifest-path ./support/sourcemap-resolver/Cargo.toml
	mv -v $(addsuffix .exe, $(addprefix target/x86_64-pc-windows-msvc/release/, ${BIN_NAMES})) ./
	mv -v ./support/sourcemap-resolver/target/x86_64-pc-windows-msvc/release/sourcemap-resolver.exe ./
	7z a ${ZIP_NAME}-x86_64-windows.zip $(addsuffix .exe, ${BIN_NAMES}) sourcemap-resolver.exe

release_win_aarch64:
	cargo build --locked --release --target=aarch64-pc-windows-msvc $(addprefix --bin , ${BIN_NAMES})
	cargo build --locked --release --target=aarch64-pc-windows-msvc --manifest-path ./support/sourcemap-resolver/Cargo.toml
	mv -v $(addsuffix .exe, $(addprefix target/aarch64-pc-windows-msvc/release/, ${BIN_NAMES})) ./
	mv -v ./support/sourcemap-resolver/target/aarch64-pc-windows-msvc/release/sourcemap-resolver.exe ./
	7z a ${ZIP_NAME}-aarch64-windows.zip $(addsuffix .exe, ${BIN_NAMES}) sourcemap-resolver.exe

release_mac:
	cargo build --locked --release --target=x86_64-apple-darwin  $(addprefix --bin , ${BIN_NAMES})
	cargo build --locked --release --target=aarch64-apple-darwin $(addprefix --bin , ${BIN_NAMES})
	cargo build --locked --release --target=x86_64-apple-darwin  --manifest-path ./support/sourcemap-resolver/Cargo.toml
	cargo build --locked --release --target=aarch64-apple-darwin --manifest-path ./support/sourcemap-resolver/Cargo.toml
	zip -j ${ZIP_NAME}-x86_64-mac.zip $(addprefix target/x86_64-apple-darwin/release/, ${BIN_NAMES}) \
		                              ./support/sourcemap-resolver/target/x86_64-apple-darwin/release/sourcemap-resolver
	zip -j ${ZIP_NAME}-aarch64-mac.zip $(addprefix target/aarch64-apple-darwin/release/, ${BIN_NAMES}) \
		                               ./support/sourcemap-resolver/target/aarch64-apple-darwin/release/sourcemap-resolver

release_version:
	echo "$(VERSION)" > version

watch:
	cargo watch -i crates/parser/src/generated -x test -x bench

install:
	verylup install local
	cargo install --profile release-verylup --path crates/mdbook

gen_sv:
	cargo run --bin veryl -- build

gen_ir:
	cargo run --bin veryl -- dump --ir

fmt_veryl:
	cargo run --bin veryl -- fmt

flamegraph:
	cargo bench --bench benchmark -- --profile-time=5

# crates.io release. The veryl-component family is versioned independently, so
# release it separately; when both changed, release it first (veryl-simulator
# depends on it).
#
# These are dry-runs by default; pass EXECUTE=1 to actually release
# (e.g. `make release-veryl-patch EXECUTE=1`).

RELEASE_EXECUTE := $(if $(EXECUTE),--execute)

.PHONY: release-veryl-patch release-veryl-minor release-component-patch release-component-minor

release-veryl-patch:
	cargo release patch --exclude veryl-component --exclude veryl-component-sys --exclude veryl-component-macros $(RELEASE_EXECUTE)

release-veryl-minor:
	cargo release minor --exclude veryl-component --exclude veryl-component-sys --exclude veryl-component-macros $(RELEASE_EXECUTE)

release-component-patch:
	cargo release patch -p veryl-component-sys -p veryl-component-macros -p veryl-component $(RELEASE_EXECUTE)

release-component-minor:
	cargo release minor -p veryl-component-sys -p veryl-component-macros -p veryl-component $(RELEASE_EXECUTE)
