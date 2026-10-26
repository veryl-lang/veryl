use crate::aig::convert::{aig_to_cells, aigify};
use crate::aig::techmap::aig_to_cells_techmap;
use crate::ir::{
    Cell, CellKind, ClockEdge, FfCell, GateModule, GatePort, NetDriver, NetInfo, PortDir, RamBlock,
    RamReadPort, RamWritePort,
};

use crate::regression_support as common;
fn eval(module: &GateModule, net: u32, input: [bool; 4], depth: usize) -> bool {
    let values = [2, 3, 4, 11].into_iter().zip(input).collect();
    common::eval(
        module,
        net,
        &values,
        &mut vec![None; module.nets.len()],
        depth,
    )
}

#[test]
fn ram_boundaries_survive_aig_round_trips() {
    let mut module = GateModule {
        nets: (0..12)
            .map(|_| NetInfo {
                driver: NetDriver::Undriven,
                origin: None,
            })
            .collect(),
        ..Default::default()
    };
    module.nets[0].driver = NetDriver::Const(false);
    module.nets[1].driver = NetDriver::Const(true);
    module.nets[2].driver = NetDriver::PortInput;
    module.nets[3].driver = NetDriver::PortInput;
    module.nets[4].driver = NetDriver::RamRead(0, 0, 0);
    for (output, kind, inputs) in [
        (5, CellKind::Not, vec![2]),
        (6, CellKind::And2, vec![2, 3]),
        (7, CellKind::Xor2, vec![3, 4]),
        (8, CellKind::Or2, vec![2, 4]),
        (9, CellKind::Not, vec![3]),
        (10, CellKind::Not, vec![4]),
    ] {
        module.nets[output as usize].driver = NetDriver::Cell(module.cells.len());
        module.cells.push(Cell {
            kind,
            inputs,
            output,
        });
    }
    module.nets[11].driver = NetDriver::FfQ(0);
    module.ffs.push(FfCell {
        clock: 2,
        clock_edge: ClockEdge::Posedge,
        reset: None,
        d: 7,
        q: 11,
        reset_value: false,
        clock_domain: Default::default(),
        origin: None,
    });
    module.ports.push(GatePort {
        name: Default::default(),
        path: vec![],
        dir: PortDir::Output,
        nets: vec![10],
    });
    module.ram_blocks.push(RamBlock {
        name: Default::default(),
        depth: 2,
        width: 1,
        clock: 5,
        clock_edge: ClockEdge::Negedge,
        read_ports: vec![RamReadPort {
            addr: vec![6],
            data: vec![4],
            sync: false,
        }],
        write_ports: vec![RamWritePort {
            addr: vec![6],
            data: vec![7],
            enable: 8,
            mask: Some(vec![9]),
        }],
    });
    let aig = aigify(&module);
    for mapped in [
        aig_to_cells(&aig, &module),
        aig_to_cells_techmap(&aig, &module),
    ] {
        assert_eq!(
            mapped.ram_blocks.len(),
            1,
            "synthesizer RAM-boundary output mismatch: missing block"
        );
        assert!(
            matches!(mapped.nets[4].driver, NetDriver::RamRead(0, 0, 0)),
            "synthesizer RAM-boundary output mismatch: read driver"
        );
        let ram = &mapped.ram_blocks[0];
        assert_eq!((ram.depth, ram.width, ram.clock), (2, 1, 5));
        assert_eq!(ram.clock_edge, ClockEdge::Negedge);
        assert_eq!(ram.read_ports[0].addr, vec![6]);
        assert_eq!(ram.read_ports[0].data, vec![4]);
        assert!(!ram.read_ports[0].sync);
        assert_eq!(ram.write_ports[0].addr, vec![6]);
        assert_eq!(ram.write_ports[0].data, vec![7]);
        assert_eq!(ram.write_ports[0].enable, 8);
        assert_eq!(ram.write_ports[0].mask, Some(vec![9]));
        for assignment in 0..16 {
            let inputs = std::array::from_fn(|bit| assignment & (1 << bit) != 0);
            for net in [5, 6, 7, 8, 9, 10] {
                assert_eq!(
                    eval(&mapped, net, inputs, 0),
                    eval(&module, net, inputs, 0),
                    "synthesizer RAM-boundary output mismatch"
                );
            }
            assert_eq!(
                eval(&mapped, mapped.ffs[0].d, inputs, 0),
                eval(&module, 7, inputs, 0),
                "synthesizer RAM-boundary output mismatch: FF sink"
            );
        }
    }
}
