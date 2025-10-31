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
fn migrate_eq() {
    let code = r#"
    module A {
        let a: logic = (1 === 1);
    }"#;

    let exp = r#"
    module A {
        let a: logic = (1 ==  1);
    }"#;

    migrate(code, exp);
}

#[test]
fn migrate_ne() {
    let code = r#"
    module A {
        let a: logic = (1 !== 1);
    }"#;

    let exp = r#"
    module A {
        let a: logic = (1 !=  1);
    }"#;

    migrate(code, exp);
}

#[test]
fn migrate_xnor() {
    let code = r#"
    module A {
        let a: logic = (1 ^~ 1);
    }"#;

    let exp = r#"
    module A {
        let a: logic = (1 ~^ 1);
    }"#;

    migrate(code, exp);
}
