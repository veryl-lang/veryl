use crate::ir::{Event, Value, VarId};
use crate::simulator::Simulator;
use crate::testbench::TestResult;
use veryl_analyzer::wavedrom::preprocess_json5;
pub use veryl_analyzer::wavedrom::strip_port_prefix;

/// Parsed WaveDrom scenario containing signals and their waveforms.
#[derive(Debug)]
pub struct WaveScenario {
    pub signals: Vec<WaveSignal>,
}

/// A single signal in the WaveDrom scenario.
#[derive(Debug)]
pub struct WaveSignal {
    pub name: String,
    pub kind: SignalKind,
    pub wave: Vec<WaveChar>,
    pub data: Vec<String>,
}

/// Classification of a signal after port mapping.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SignalKind {
    Clock,
    Input,
    Output,
    Unknown,
}

/// A single character in the expanded wave string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WaveChar {
    PosedgeClock,
    NegedgeClock,
    Low,
    High,
    DontCare,
    HighZ,
    Data(usize),
}

/// Parse a WaveDrom JSON5 string into a WaveScenario.
pub fn parse_wavedrom(json_str: &str) -> Result<WaveScenario, String> {
    let json_str = preprocess_json5(json_str);
    let value: serde_json::Value =
        serde_json::from_str(&json_str).map_err(|e| format!("JSON parse error: {e}"))?;

    let signals = extract_signals(&value)?;
    Ok(WaveScenario { signals })
}

fn extract_signals(value: &serde_json::Value) -> Result<Vec<WaveSignal>, String> {
    let signal_array = value
        .get("signal")
        .and_then(|v| v.as_array())
        .ok_or_else(|| "missing 'signal' array".to_string())?;

    let mut signals = Vec::new();
    for item in signal_array {
        if item.is_string() || item.is_array() {
            continue;
        }
        if let Some(obj) = item.as_object()
            && let Some(name) = obj.get("name").and_then(|v| v.as_str())
        {
            let wave_str = obj.get("wave").and_then(|v| v.as_str()).unwrap_or("");
            let data: Vec<String> = if let Some(d) = obj.get("data") {
                if let Some(arr) = d.as_array() {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(|s| s.to_string()))
                        .collect()
                } else if let Some(s) = d.as_str() {
                    s.split_whitespace().map(|s| s.to_string()).collect()
                } else {
                    Vec::new()
                }
            } else {
                Vec::new()
            };

            let wave = expand_wave(wave_str)?;
            signals.push(WaveSignal {
                name: name.to_string(),
                kind: SignalKind::Unknown,
                wave,
                data,
            });
        }
    }

    Ok(signals)
}

/// Expand a wave string into WaveChars, resolving '.' repetitions and removing '|'.
fn expand_wave(wave_str: &str) -> Result<Vec<WaveChar>, String> {
    let mut result = Vec::new();
    let mut last_char: Option<WaveChar> = None;
    let mut data_idx = 0;

    for c in wave_str.chars() {
        match c {
            '|' => continue,
            '.' => {
                if let Some(ref prev) = last_char {
                    result.push(prev.clone());
                }
            }
            'p' | 'P' => {
                let wc = WaveChar::PosedgeClock;
                result.push(wc.clone());
                last_char = Some(wc);
            }
            'n' | 'N' => {
                let wc = WaveChar::NegedgeClock;
                result.push(wc.clone());
                last_char = Some(wc);
            }
            '0' => {
                let wc = WaveChar::Low;
                result.push(wc.clone());
                last_char = Some(wc);
            }
            '1' => {
                let wc = WaveChar::High;
                result.push(wc.clone());
                last_char = Some(wc);
            }
            'x' => {
                let wc = WaveChar::DontCare;
                result.push(wc.clone());
                last_char = Some(wc);
            }
            'z' => {
                let wc = WaveChar::HighZ;
                result.push(wc.clone());
                last_char = Some(wc);
            }
            '2' | '3' | '4' | '5' | '6' | '7' | '8' | '9' | '=' => {
                let wc = WaveChar::Data(data_idx);
                result.push(wc.clone());
                last_char = Some(wc);
                data_idx += 1;
            }
            ' ' => continue,
            _ => {
                return Err(format!("unknown wave character: '{c}'"));
            }
        }
    }

    Ok(result)
}

/// Parse a data value string (hex/bin/dec) to u64.
fn parse_data_value(s: &str) -> Result<u64, String> {
    let s = s.trim();
    if let Some(hex) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        u64::from_str_radix(hex, 16).map_err(|e| format!("invalid hex value '{s}': {e}"))
    } else if let Some(bin) = s.strip_prefix("0b").or_else(|| s.strip_prefix("0B")) {
        u64::from_str_radix(bin, 2).map_err(|e| format!("invalid binary value '{s}': {e}"))
    } else {
        s.parse::<u64>()
            .map_err(|e| format!("invalid decimal value '{s}': {e}"))
    }
}

fn wave_value(wc: &WaveChar, data: &[String], width: usize) -> Result<Option<Value>, String> {
    match wc {
        WaveChar::Low => Ok(Some(Value::new(0, width, false))),
        WaveChar::High => Ok(Some(Value::new(1, width, false))),
        WaveChar::DontCare => Ok(None),
        WaveChar::HighZ => Ok(Some(Value::new_z(width, false))),
        WaveChar::Data(idx) => {
            if *idx < data.len() {
                let val = parse_data_value(&data[*idx])?;
                Ok(Some(Value::new(val, width, false)))
            } else {
                Err(format!(
                    "data index {idx} out of range (only {} data values)",
                    data.len()
                ))
            }
        }
        WaveChar::PosedgeClock | WaveChar::NegedgeClock => Ok(None),
    }
}

/// Classify signals based on module port information.
pub fn classify_signals(scenario: &mut WaveScenario, port_info: &[(String, String)]) {
    for signal in &mut scenario.signals {
        if !signal.wave.is_empty()
            && matches!(
                signal.wave[0],
                WaveChar::PosedgeClock | WaveChar::NegedgeClock
            )
        {
            signal.kind = SignalKind::Clock;
            continue;
        }

        let matched = port_info.iter().find(|(port_name, _)| {
            port_name == &signal.name || strip_port_prefix(port_name) == signal.name
        });

        if let Some((_, direction)) = matched {
            signal.kind = match direction.as_str() {
                "input" => SignalKind::Input,
                "output" => SignalKind::Output,
                "inout" => SignalKind::Input, // treat inout as input for driving
                _ => SignalKind::Unknown,
            };
        }
    }
}

/// Run a WaveDrom scenario against a simulator instance.
///
/// Each cycle: drive inputs -> evaluate comb -> check outputs -> step clock/reset.
/// This order ensures outputs reflect the pre-edge state (previous FF values +
/// current combinational inputs), matching WaveDrom timing conventions.
pub fn run_wavedrom_test<T: std::io::Write>(
    sim: &mut Simulator<T>,
    scenario: &WaveScenario,
    clock_event: &Event,
    reset_event: &Option<Event>,
    default_reset_cycles: u64,
    port_widths: &std::collections::HashMap<String, usize>,
) -> TestResult {
    let reset_signal_idx = scenario
        .signals
        .iter()
        .position(|s| s.kind == SignalKind::Input && is_reset_name(&s.name));

    let reset_active_low =
        reset_signal_idx.is_some_and(|idx| is_active_low_reset(&scenario.signals[idx].name));

    let has_dump = sim.dump.is_some();
    let clock_var_id: Option<VarId> = clock_event.var_id();
    let reset_var_id: Option<VarId> = reset_event.as_ref().and_then(|e| e.var_id());

    if reset_signal_idx.is_none()
        && let Some(rst_event) = reset_event
    {
        if has_dump && let Some(ref id) = reset_var_id {
            sim.set_var_by_id(id, Value::new(1, 1, false));
        }
        for _ in 0..default_reset_cycles {
            if has_dump && let Some(ref id) = clock_var_id {
                sim.set_var_by_id(id, Value::new(1, 1, false));
            }
            sim.step(rst_event);
            if has_dump {
                if let Some(ref id) = clock_var_id {
                    sim.set_var_by_id(id, Value::new(0, 1, false));
                }
                sim.dump_and_advance_time();
            }
        }
        if has_dump && let Some(ref id) = reset_var_id {
            sim.set_var_by_id(id, Value::new(0, 1, false));
        }
    }

    let max_len = scenario
        .signals
        .iter()
        .map(|s| s.wave.len())
        .max()
        .unwrap_or(0);

    for t in 0..max_len {
        // Drive input signals
        for signal in &scenario.signals {
            if signal.kind != SignalKind::Input || t >= signal.wave.len() {
                continue;
            }
            let width = port_widths.get(&signal.name).copied().unwrap_or(1);
            if let Ok(Some(val)) = wave_value(&signal.wave[t], &signal.data, width) {
                sim.set(&signal.name, val);
            }
        }

        // Evaluate combinational logic before checking outputs
        sim.ensure_comb_updated();

        // Check output signals (pre-edge values)
        for signal in &scenario.signals {
            if signal.kind != SignalKind::Output || t >= signal.wave.len() {
                continue;
            }
            let width = port_widths.get(&signal.name).copied().unwrap_or(1);
            match wave_value(&signal.wave[t], &signal.data, width) {
                Ok(Some(expected)) => {
                    let actual = sim.get(&signal.name);
                    if let Some(actual) = actual {
                        if actual.payload_u64() != expected.payload_u64() {
                            return TestResult::Fail(format!(
                                "cycle {t}: signal '{}' expected {} but got {}",
                                signal.name,
                                expected.payload_u64(),
                                actual.payload_u64(),
                            ));
                        }
                    } else {
                        return TestResult::Fail(format!(
                            "cycle {t}: signal '{}' not found in simulator",
                            signal.name,
                        ));
                    }
                }
                Ok(None) => {}
                Err(e) => {
                    return TestResult::Fail(format!("cycle {t}: signal '{}': {e}", signal.name));
                }
            }
        }

        // Step clock or reset edge
        let step_event = if let Some(rst_idx) = reset_signal_idx
            && let Some(rst_event) = reset_event
            && t < scenario.signals[rst_idx].wave.len()
        {
            let rst_asserted =
                is_reset_asserted(&scenario.signals[rst_idx].wave[t], reset_active_low);
            if rst_asserted {
                if has_dump && let Some(ref id) = reset_var_id {
                    sim.set_var_by_id(id, Value::new(1, 1, false));
                }
                rst_event
            } else {
                if has_dump && let Some(ref id) = reset_var_id {
                    sim.set_var_by_id(id, Value::new(0, 1, false));
                }
                clock_event
            }
        } else {
            clock_event
        };

        if has_dump && let Some(ref id) = clock_var_id {
            sim.set_var_by_id(id, Value::new(1, 1, false));
        }
        sim.step(step_event);
        if has_dump {
            if let Some(ref id) = clock_var_id {
                sim.set_var_by_id(id, Value::new(0, 1, false));
            }
            sim.dump_and_advance_time();
        }
    }

    TestResult::Pass
}

/// Check if a reset name indicates active-low polarity (ends with `_n`).
fn is_active_low_reset(name: &str) -> bool {
    let lower = name.to_lowercase();
    let stripped = strip_port_prefix(&lower);
    stripped.ends_with("_n")
}

fn is_reset_asserted(wc: &WaveChar, active_low: bool) -> bool {
    match wc {
        WaveChar::Low => active_low,   // 0: asserted for active-low
        WaveChar::High => !active_low, // 1: asserted for active-high
        _ => false,
    }
}

fn is_reset_name(name: &str) -> bool {
    let lower = name.to_lowercase();
    let stripped = strip_port_prefix(&lower);
    stripped == "rst"
        || stripped == "reset"
        || stripped == "rst_n"
        || stripped == "reset_n"
        || stripped.ends_with("_rst")
        || stripped.ends_with("_reset")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_simple_wavedrom() {
        let json = r#"{
            signal: [
                {name: 'clk', wave: 'p....'},
                {name: 'dat', wave: '01010'}
            ]
        }"#;
        let scenario = parse_wavedrom(json).unwrap();
        assert_eq!(scenario.signals.len(), 2);
        assert_eq!(scenario.signals[0].name, "clk");
        assert_eq!(scenario.signals[0].wave.len(), 5);
        assert_eq!(scenario.signals[1].name, "dat");
        assert_eq!(scenario.signals[1].wave[0], WaveChar::Low);
        assert_eq!(scenario.signals[1].wave[1], WaveChar::High);
    }

    #[test]
    fn test_expand_dot() {
        let json = r#"{
            signal: [
                {name: 'sig', wave: '01..0'}
            ]
        }"#;
        let scenario = parse_wavedrom(json).unwrap();
        assert_eq!(scenario.signals[0].wave.len(), 5);
        assert_eq!(scenario.signals[0].wave[0], WaveChar::Low);
        assert_eq!(scenario.signals[0].wave[1], WaveChar::High);
        assert_eq!(scenario.signals[0].wave[2], WaveChar::High);
        assert_eq!(scenario.signals[0].wave[3], WaveChar::High);
        assert_eq!(scenario.signals[0].wave[4], WaveChar::Low);
    }

    #[test]
    fn test_data_values() {
        let json = r#"{
            signal: [
                {name: 'bus', wave: '=.=.=', data: ['0x0', '0x1', '0xFF']}
            ]
        }"#;
        let scenario = parse_wavedrom(json).unwrap();
        assert_eq!(scenario.signals[0].wave[0], WaveChar::Data(0));
        assert_eq!(scenario.signals[0].wave[2], WaveChar::Data(1));
        assert_eq!(scenario.signals[0].wave[4], WaveChar::Data(2));
        assert_eq!(parse_data_value("0xFF").unwrap(), 255);
        assert_eq!(parse_data_value("0b1010").unwrap(), 10);
        assert_eq!(parse_data_value("42").unwrap(), 42);
    }

    #[test]
    fn test_classify_signals() {
        let json = r#"{
            signal: [
                {name: 'clk', wave: 'p...'},
                {name: 'din', wave: '0101'},
                {name: 'dout', wave: 'x..1'}
            ]
        }"#;
        let mut scenario = parse_wavedrom(json).unwrap();
        let ports = vec![
            ("clk".to_string(), "input".to_string()),
            ("din".to_string(), "input".to_string()),
            ("dout".to_string(), "output".to_string()),
        ];
        classify_signals(&mut scenario, &ports);
        assert_eq!(scenario.signals[0].kind, SignalKind::Clock);
        assert_eq!(scenario.signals[1].kind, SignalKind::Input);
        assert_eq!(scenario.signals[2].kind, SignalKind::Output);
    }

    #[test]
    fn test_preprocess_json5() {
        let input = "{ name: 'hello', wave: '010' }";
        let output = preprocess_json5(input);
        assert!(output.contains("\"name\""));
        assert!(output.contains("\"hello\""));
    }

    #[test]
    fn test_pipe_separator() {
        let json = r#"{
            signal: [
                {name: 'sig', wave: '01|01'}
            ]
        }"#;
        let scenario = parse_wavedrom(json).unwrap();
        assert_eq!(scenario.signals[0].wave.len(), 4);
    }
}
