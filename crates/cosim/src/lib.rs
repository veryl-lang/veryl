use std::ffi::{CStr, c_char};
use std::ptr::NonNull;
use veryl_analyzer::ir as air;
use veryl_analyzer::value::{SvLogicVecVal, Value};
use veryl_analyzer::{Analyzer, Context};
use veryl_metadata::Metadata;
use veryl_parser::Parser;
use veryl_simulator::ir as sir;
use veryl_simulator::{Config, Simulator};

fn build_ir(code: &str, top: &str, config: &Config) -> sir::Ir {
    let metadata = Metadata::create_default("prj").unwrap();
    let parser = Parser::parse(code, &"").unwrap();
    let analyzer = Analyzer::new(&metadata);
    let mut context = Context::default();

    let mut ir = air::Ir::default();
    analyzer.analyze_pass1("prj", &parser.veryl);
    Analyzer::analyze_post_pass1();
    analyzer.analyze_pass2("prj", &parser.veryl, &mut context, Some(&mut ir));

    sir::build_ir(ir, top.into(), config).unwrap()
}

type Sim = Simulator<std::io::Empty>;

#[unsafe(no_mangle)]
#[allow(clippy::missing_safety_doc)]
pub unsafe extern "C" fn cosim_open(
    path: *const c_char,
    top: *const c_char,
    use_4state: bool,
) -> NonNull<Sim> {
    let path = unsafe { CStr::from_ptr(path) };
    let path = path.to_str().unwrap();
    let code = std::fs::read_to_string(path).unwrap();

    let top = unsafe { CStr::from_ptr(top) };
    let top = top.to_str().unwrap();

    let config = Config {
        use_4state,
        ..Default::default()
    };

    let ir = build_ir(&code, top, &config);

    let sim = Box::new(Sim::new(ir, None));
    let sim = Box::<Sim>::into_raw(sim);
    NonNull::new(sim).unwrap()
}

#[unsafe(no_mangle)]
#[allow(clippy::missing_safety_doc)]
pub unsafe extern "C" fn cosim_close(handle: NonNull<Sim>) {
    let _sim = unsafe { Box::<Sim>::from_raw(handle.as_ptr()) };
}

#[unsafe(no_mangle)]
#[allow(clippy::missing_safety_doc)]
pub unsafe extern "C" fn cosim_step_reset(handle: NonNull<Sim>, name: *const c_char) {
    let sim = unsafe { &mut *handle.as_ptr() };

    let name = unsafe { CStr::from_ptr(name) };
    let name = name.to_str().unwrap();

    let reset = sim.get_reset(name).unwrap();
    sim.step(&reset);
}

#[unsafe(no_mangle)]
#[allow(clippy::missing_safety_doc)]
pub unsafe extern "C" fn cosim_step_clock(handle: NonNull<Sim>, name: *const c_char) {
    let sim = unsafe { &mut *handle.as_ptr() };

    let name = unsafe { CStr::from_ptr(name) };
    let name = name.to_str().unwrap();

    let reset = sim.get_clock(name).unwrap();
    sim.step(&reset);
}

#[unsafe(no_mangle)]
#[allow(clippy::missing_safety_doc)]
pub unsafe extern "C" fn cosim_set(
    handle: NonNull<Sim>,
    name: *const c_char,
    value: &[SvLogicVecVal; 4],
) {
    let sim = unsafe { &mut *handle.as_ptr() };

    let name = unsafe { CStr::from_ptr(name) };
    let name = name.to_str().unwrap();

    let value: Value = value.as_slice().into();

    sim.set(name, value);
}

#[unsafe(no_mangle)]
#[allow(clippy::missing_safety_doc)]
pub unsafe extern "C" fn cosim_get(
    handle: NonNull<Sim>,
    name: *const c_char,
    value: &mut [SvLogicVecVal; 4],
) {
    let sim = unsafe { &mut *handle.as_ptr() };

    let name = unsafe { CStr::from_ptr(name) };
    let name = name.to_str().unwrap();

    let ret = sim.get(name).unwrap();
    let ret: Vec<SvLogicVecVal> = (&ret).into();

    for (i, val) in value.iter_mut().enumerate() {
        if let Some(x) = ret.get(i) {
            val.aval = x.aval;
            val.bval = x.bval;
        } else {
            val.aval = 0;
            val.bval = 0;
        }
    }
}
