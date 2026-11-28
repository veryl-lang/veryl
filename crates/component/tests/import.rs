//! `import!` and `#[modport]`, over the description in `veryl.types.json`
//! next to this crate's `Cargo.toml`.

use veryl_component::testing::{MockSim, RefDir};
use veryl_component::{
    FixedWidth, InputOnly, Modport, ModportRef, Packed, Result, SimCtx, Value, component_impl,
    import, modport,
};

import!(test_pkg);
import!(test_if, use(test_pkg = crate::test_pkg));

use test_pkg::{BusConfig, Mode};

fn config() -> BusConfig {
    BusConfig {
        mode: Mode::Busy,
        regs: 0xa5,
        nibbles: [0x1, 0x2, 0x3],
        delta: -2,
        wide_value: (1 << 69) | 0x55,
    }
}

#[test]
fn constants_and_aliases() {
    assert_eq!(test_pkg::REGS, 8u32);
    assert_eq!(test_pkg::NEG_VALUE, -2i8);
    let enable: bool = test_pkg::ENABLE;
    assert!(enable);
    assert_eq!(test_pkg::BUS_NAME, "bus \"vip\"");
    assert_eq!(test_pkg::DEFAULT_MODE, Mode::Busy);
    assert_eq!(
        test_pkg::DEFAULT_CONFIG,
        BusConfig {
            mode: Mode::Idle,
            regs: 16,
            nibbles: [1, 2, 3],
            delta: -1,
            wide_value: 5,
        }
    );
    let word: test_pkg::Word = 0xffff;
    assert_eq!(word, u16::MAX);
}

#[test]
fn enum_conversion() {
    assert_eq!(<Mode as FixedWidth>::WIDTH, 2);
    // The variants carry Veryl's values.
    assert_eq!(Mode::Idle as u8, 1);
    assert_eq!(Mode::Busy as u8, 2);
    // No variant is 0: the first is the default.
    assert_eq!(Mode::default(), Mode::Idle);
    assert_eq!(
        Mode::from_value(&Value::from_u64(2, 2)).unwrap(),
        Mode::Busy
    );
    let err = Mode::from_value(&Value::from_u64(3, 2)).unwrap_err();
    assert!(err.to_string().contains("3 is no value of enum `Mode`"));
    assert_eq!(Value::from(Mode::Idle), Value::from_u64(1, 2));
}

#[test]
fn struct_layout() {
    assert_eq!(<BusConfig as FixedWidth>::WIDTH, 96);
    let value = Value::from(config());
    assert_eq!(value.width(), 96);
    // The first field is the most significant.
    let words = value.as_slice();
    assert_eq!(words[1] >> 30, 0b10); // mode
    assert_eq!((words[1] >> 22) & 0xff, 0xa5); // regs
    assert_eq!((words[1] >> 10) & 0xfff, 0x123); // nibbles
    assert_eq!((words[1] >> 6) & 0xf, 0b1110); // delta
    assert_eq!(BusConfig::from_value(&value).unwrap(), config());

    // An invalid enum value names the field.
    let mut words = value.as_slice().to_vec();
    words[1] |= 0b11 << 30;
    let value = Value::from_bits(words.into(), Default::default(), 96);
    let err = format!("{:#}", BusConfig::from_value(&value).unwrap_err());
    assert!(err.contains("field `mode` of struct `BusConfig`"), "{err}");

    assert!(BusConfig::from_value(&Value::from_u64(0, 95)).is_err());
}

#[test]
fn default_struct() {
    let x = BusConfig::default();
    assert_eq!(x.mode, Mode::Idle);
    assert_eq!(x.nibbles, [0; 3]);
}

/// A hand-written modport, binding a member under another name.
#[modport]
struct Watch {
    #[input(name = "ready")]
    accepted: bool,
    #[input(name = "dataOut")]
    data: u8,
}

fn input_only<T: InputOnly>() {}

#[derive(veryl_component::Component, Default)]
struct Driver {
    bus: Option<test_if::Master>,
    watch: Option<Watch>,
    config: Option<BusConfig>,
    seen: Vec<(bool, u8, bool, u8)>,
}

#[component_impl]
impl Driver {
    fn set_bus(&mut self, _ctx: &mut SimCtx, bus: ModportRef) -> Result<()> {
        self.bus = Some(test_if::Master::bind(&bus)?);
        self.watch = Some(Watch::bind(&bus)?);
        Ok(())
    }

    fn set_config(&mut self, _ctx: &mut SimCtx, cfg: BusConfig) -> Result<()> {
        self.config = Some(cfg);
        Ok(())
    }

    fn step(&mut self, ctx: &mut SimCtx) -> Result<()> {
        let bus = self.bus.as_mut().unwrap();
        bus.load(ctx)?;
        let watch = self.watch.as_mut().unwrap();
        watch.load(ctx)?;
        self.seen
            .push((bus.ready, bus.data_out, watch.accepted, watch.data));
        bus.valid = true;
        bus.cfg = self.config.clone().unwrap_or_default();
        bus.store(ctx);
        Ok(())
    }

    /// Drives `valid` low and reads `ready`, one member at a time.
    fn idle(&mut self, ctx: &mut SimCtx) -> Result<bool> {
        let bus = self.bus.as_mut().unwrap();
        bus.valid = false;
        bus.force_valid(ctx);
        bus.load_ready(ctx)?;
        Ok(bus.ready)
    }
}

#[test]
fn modport_reads_and_drives() {
    input_only::<Watch>();
    input_only::<test_if::Monitor>();

    let mut sim = MockSim::new();
    let mut driver = sim.build::<Driver>().unwrap();
    let bus = sim.modport_ref(
        "u_if",
        "test_if",
        "master",
        &[
            ("clk", RefDir::Clock, &[]),
            ("valid", RefDir::Output, &[1]),
            ("cfg", RefDir::Output, &[96]),
            ("ready", RefDir::Input, &[1]),
            ("dataOut", RefDir::Input, &[8]),
        ],
    );
    sim.call_with_refs(&mut driver, "set_bus", &[bus]).unwrap();
    sim.call(&mut driver, "set_config", &[Value::from(config())])
        .unwrap();

    sim.set("u_if.ready", 1u64);
    sim.set("u_if.dataOut", 0x5au64);
    sim.call(&mut driver, "step", &[]).unwrap();
    assert_eq!(driver.seen, vec![(true, 0x5a, true, 0x5a)]);
    assert_eq!(sim.get("u_if.valid").as_u64().unwrap(), 1);
    assert_eq!(
        BusConfig::from_value(&sim.get("u_if.cfg")).unwrap(),
        config()
    );

    sim.set("u_if.ready", 0u64);
    let ready = sim.call(&mut driver, "idle", &[]).unwrap();
    assert!(!ready.as_bool());
    assert_eq!(sim.get("u_if.valid").as_u64().unwrap(), 0);
}

#[test]
fn modport_rejects_a_member_too_wide() {
    let mut sim = MockSim::new();
    let mut driver = sim.build::<Driver>().unwrap();
    let bus = sim.modport_ref(
        "u_if",
        "test_if",
        "master",
        &[
            ("valid", RefDir::Output, &[1]),
            ("cfg", RefDir::Output, &[96]),
            ("ready", RefDir::Input, &[1]),
            ("dataOut", RefDir::Input, &[9]),
        ],
    );
    let err = sim
        .call_with_refs(&mut driver, "set_bus", &[bus])
        .unwrap_err();
    assert!(err.to_string().contains("u_if.dataOut"), "{err}");
}
