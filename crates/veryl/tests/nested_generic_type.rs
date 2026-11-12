use std::fs;
use std::process::Command;

#[test]
fn dependency_type_argument_keeps_its_callers_generic_scope() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir_all(dir.path().join("src")).unwrap();
    fs::create_dir_all(dir.path().join("lib/src")).unwrap();
    fs::write(
        dir.path().join("Veryl.toml"),
        r#"
[project]
name = "nested_types"
version = "0.1.0"
[build]
sources = ["src"]
[dependencies]
widths = {path = "lib"}
"#,
    )
    .unwrap();
    fs::write(
        dir.path().join("lib/Veryl.toml"),
        r#"
[project]
name = "widths"
version = "0.1.0"
[build]
sources = ["src"]
"#,
    )
    .unwrap();
    fs::write(
        dir.path().join("lib/src/main.veryl"),
        r#"
pub package Bound::<N: u32> {type Index = logic<$clog2(N)>;}
"#,
    )
    .unwrap();
    fs::write(
        dir.path().join("src/geometry.veryl"),
        r#"
pub package Geometry::<N: u32> {const ROWS: u32 = N + 1;}
"#,
    )
    .unwrap();
    fs::write(
        dir.path().join("src/main.veryl"),
        r#"
pub module Binding::<N: u32> (
    value: output widths::Bound::<Geometry::<N>::ROWS>::Index,
) {assign value = '1;}
#[test(nested_type)]
module NestedTypeTest {
    var value_two: logic<2>;
    var value_three: logic<3>;
    inst a: Binding::<2> (value: value_two);
    inst b: Binding::<4> (value: value_three);
    initial {
        $assert(value_two == 3, "TWO_BIT_INSTANCE");
        $assert(value_three == 7, "THREE_BIT_INSTANCE");
        $finish();
    }
}
"#,
    )
    .unwrap();
    let build = Command::new(env!("CARGO_BIN_EXE_veryl"))
        .current_dir(dir.path())
        .arg("build")
        .output()
        .unwrap();
    assert!(
        build.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&build.stdout),
        String::from_utf8_lossy(&build.stderr)
    );
    for backend in ["interpret", "cranelift", "cc"] {
        let output = Command::new(env!("CARGO_BIN_EXE_veryl"))
            .current_dir(dir.path())
            .env("VERYL_DUT_REUSE", "0")
            .args([
                "test",
                "--backend",
                backend,
                "--format",
                "json",
                "--backend-validate",
                "1",
            ])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{backend}\n{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(report["passed"], 1);
        assert_eq!(report["failed"], 0);
        assert_eq!(report["ignored"], 0);
        assert_eq!(report["tests"].as_array().unwrap().len(), 1);
    }
}
