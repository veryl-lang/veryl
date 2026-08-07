// Incomplete-effect boundary coverage for comb-loop migration.
use super::*;

#[test]
#[ignore = "comb-loop migration: false positive; malformed-effect cycle must be suppressed"]
fn comb_loop_malformed_effect_is_a_causal_barrier() {
    // Why this case exists: a rejected statement may have unknown side
    // effects, so a cycle which crosses it is not proven. The malformed
    // procedure must not suppress a separate exact cycle in another procedure.
    let errors = analyze(
        r#"
        module Top (
            o: output logic,
        ) {
            var a: logic;
            var b: logic;
            var c: logic;
            var d: logic;
            always_comb {
                a = b;
                missing_function();
                b = a;
            }
            always_comb {
                c = d;
                d = c;
                o = d;
            }
        }
        "#,
    );
    let loops = errors
        .iter()
        .filter_map(|error| match error {
            AnalyzerError::CombinationalLoop { identifier, .. } => Some(identifier.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert!(
        loops.len() == 1 && matches!(loops[0], "c" | "d"),
        "only the cycle independent of the malformed barrier is proven: {errors:#?}"
    );
}
