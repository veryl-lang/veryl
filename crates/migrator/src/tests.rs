use crate::Migrator;
use crate::Parser;
use veryl_metadata::Metadata;

#[track_caller]
fn migrate(code: &str, exp: &str) {
    let parser = Parser::parse(code, &"").unwrap();
    let mut migrator = Migrator::new(&Metadata::create_default("prj").unwrap());
    migrator.migrate(&parser.veryl, code);
    assert_eq!(migrator.as_str(), exp);
}

#[test]
fn migrate_readmemh_in_initial() {
    let code = r#"
    module A {
        var mem: logic<8> [4];
        var other: logic<8>;

        initial {
            $readmemh("a.hex", mem);
        }
    }"#;

    let exp = r#"
    module A {
        #[allow(initial_assign)] var mem: logic<8> [4];
        var other: logic<8>;

        initial {
            $readmemh("a.hex", mem);
        }
    }"#;

    migrate(code, exp);
}
