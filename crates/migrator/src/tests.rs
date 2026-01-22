use crate::Migrator;
use crate::Parser;
use veryl_metadata::Metadata;

#[track_caller]
fn migrate(code: &str, exp: &str) {
    let parser = Parser::parse(&code, &"").unwrap();
    let mut migrator = Migrator::new(&Metadata::create_default("prj").unwrap());
    migrator.migrate(&parser.veryl);
    if cfg!(windows) {
        assert_eq!(migrator.as_str().replace("\r\n", "\n"), exp);
    } else {
        assert_eq!(migrator.as_str(), exp);
    }
}

#[test]
fn migrate_statement_block() {
    let code = r#"
    module A {
        always_comb {
            {
            }
        }
    }"#;

    let exp = r#"
    module A {
        always_comb {
            block {
            }
        }
    }"#;

    migrate(code, exp);
}
