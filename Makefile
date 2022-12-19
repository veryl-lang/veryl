watch:
	cargo watch -i crates/parser/src/generated -x test -x bench

install:
	cargo install --path crates/languageserver
	cargo install --path crates/veryl
