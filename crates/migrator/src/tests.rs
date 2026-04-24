use crate::Migrator;
use crate::Parser;
use veryl_metadata::Metadata;

#[track_caller]
fn migrate(code: &str, exp: &str) {
    let parser = Parser::parse(&code, &"").unwrap();
    let mut migrator = Migrator::new(&Metadata::create_default("prj").unwrap());
    migrator.migrate(&parser.veryl, code);
    assert_eq!(migrator.as_str(), exp);
}

#[test]
fn migrate_for_statement_type_specifier() {
    let code = r#"
    module A {
        always_comb {
            for i: u32 in 0..10 {
            }
        }
    }"#;

    let exp = r#"
    module A {
        always_comb {
            for i      in 0..10 {
            }
        }
    }"#;

    migrate(code, exp);
}

#[test]
fn migrate_for_statement_signed_type() {
    let code = r#"
    module A {
        always_comb {
            for i: i32 in rev 0..10 {
            }
        }
    }"#;

    let exp = r#"
    module A {
        always_comb {
            for i      in rev 0..10 {
            }
        }
    }"#;

    migrate(code, exp);
}
