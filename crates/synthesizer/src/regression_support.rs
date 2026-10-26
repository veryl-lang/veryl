use crate::ir::{CellKind, GateModule, NetDriver};

pub fn eval(
    module: &GateModule,
    net: u32,
    input: &std::collections::HashMap<u32, bool>,
    cache: &mut [Option<bool>],
    depth: usize,
) -> bool {
    if let Some(value) = cache[net as usize] {
        return value;
    }
    assert!(depth <= module.nets.len(), "unexpected combinational cycle");
    let result = match module.nets[net as usize].driver {
        NetDriver::Const(v) => v,
        NetDriver::PortInput | NetDriver::RamRead(_, _, _) | NetDriver::FfQ(_) => {
            *input.get(&net).expect("unbound circuit input")
        }
        NetDriver::Cell(i) => {
            let cell = &module.cells[i];
            let v: Vec<_> = cell
                .inputs
                .iter()
                .map(|&n| eval(module, n, input, cache, depth + 1))
                .collect();
            use CellKind::*;
            match cell.kind {
                Buf => v[0],
                Not => !v[0],
                And2 => v[0] && v[1],
                Or2 => v[0] || v[1],
                Nand2 => !(v[0] && v[1]),
                Nor2 => !(v[0] || v[1]),
                Xor2 => v[0] ^ v[1],
                Xnor2 => !(v[0] ^ v[1]),
                And3 => v[0] && v[1] && v[2],
                Or3 => v[0] || v[1] || v[2],
                Nand3 => !(v[0] && v[1] && v[2]),
                Nor3 => !(v[0] || v[1] || v[2]),
                Ao21 => (v[0] && v[1]) || v[2],
                Aoi21 => !((v[0] && v[1]) || v[2]),
                Oa21 => (v[0] || v[1]) && v[2],
                Oai21 => !((v[0] || v[1]) && v[2]),
                Ao31 => (v[0] && v[1] && v[2]) || v[3],
                Aoi31 => !((v[0] && v[1] && v[2]) || v[3]),
                Ao22 => (v[0] && v[1]) || (v[2] && v[3]),
                Aoi22 => !((v[0] && v[1]) || (v[2] && v[3])),
                Oai22 => !((v[0] || v[1]) && (v[2] || v[3])),
                Mux2 => {
                    if v[0] {
                        v[2]
                    } else {
                        v[1]
                    }
                }
            }
        }
        ref driver => panic!("unexpected driver {driver:?}"),
    };
    cache[net as usize] = Some(result);
    result
}
