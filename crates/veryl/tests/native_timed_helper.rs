use std::fs;
use std::process::Command;

const SOURCE: &str = r#"
interface Channel {
    var valid: logic;
    var ready: logic;
    modport sink {valid: input, ready: output,}
}
module Coupled (channel: modport Channel::sink) {assign channel.ready = channel.valid;}
#[test(ready_repro)]
module ReadyReproTest {
    inst clk: $tb::clock_gen #(period: 10);
    inst channel: Channel;
    inst dut: Coupled(channel);
    var waits: u32;
    function tick() {
        $display("EDGE ready=%0d", channel.ready);
        waits += 1;
        clk.next();
    }
    function send() {
        channel.valid = 1;
        for _wait in 0..8 {if !channel.ready {tick();}}
        $assert(waits == 0, "NO_WAIT_FOR_ALREADY_READY");
    }
    function outer() {send();}
    initial {
        channel.valid = 0;
        waits = 0;
        clk.next();
        CALL();
        $finish();
    }
}
"#;

#[test]
fn timed_helper_and_direct_drive_poll_have_the_same_settle_boundary() {
    // Use the CLI pipeline and its real source paths, not a hand-constructed
    // simulator IR. The full helper lowering exposed a bug absent in the
    // original simulator-unit fixture.
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir(dir.path().join("src")).unwrap();
    fs::write(
        dir.path().join("Veryl.toml"),
        r#"
[project]
name = "ready_repro"
version = "0.1.0"
[build]
sources = ["src"]
[test]
four_state = true
"#,
    )
    .unwrap();
    let direct = r#"
        channel.valid = 1;
        for _wait in 0..8 {if !channel.ready {waits += 1; clk.next();}}
        $assert(waits == 0, "DIRECT_NO_WAIT_FOR_ALREADY_READY");
    "#;
    for call in ["send();", "outer();", direct] {
        fs::write(
            dir.path().join("src/main.veryl"),
            SOURCE.replace("CALL();", call),
        )
        .unwrap();
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
                "call={call}, backend={backend}\n{}\n{}",
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
}
