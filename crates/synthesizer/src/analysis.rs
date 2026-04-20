use crate::ir::{CellKind, GateModule, NetDriver, NetId};
use crate::library::BuiltinLibrary;
use std::collections::{HashMap, HashSet};
use std::fmt;

#[derive(Clone, Debug, Default)]
pub struct AreaReport {
    pub total: f64,
    pub combinational: f64,
    pub sequential: f64,
    pub by_kind: Vec<(CellKind, usize, f64)>,
    pub ff_count: usize,
}

impl fmt::Display for AreaReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "area:")?;
        writeln!(f, "  total:          {:.2}", self.total)?;
        writeln!(f, "  combinational:  {:.2}", self.combinational)?;
        writeln!(
            f,
            "  sequential:     {:.2}  (ff count: {})",
            self.sequential, self.ff_count
        )?;
        writeln!(f, "  by cell kind:")?;
        for (kind, count, area) in &self.by_kind {
            writeln!(
                f,
                "    {:>5}  x{:<6} area {:.2}",
                kind.symbol(),
                count,
                area
            )?;
        }
        Ok(())
    }
}

pub fn compute_area(module: &GateModule, library: &BuiltinLibrary) -> AreaReport {
    let mut buckets: HashMap<CellKind, (usize, f64)> = HashMap::new();
    let mut comb_total = 0.0;
    for cell in &module.cells {
        let info = library.info(cell.kind);
        let entry = buckets.entry(cell.kind).or_insert((0, 0.0));
        entry.0 += 1;
        entry.1 += info.area;
        comb_total += info.area;
    }
    let mut by_kind: Vec<_> = buckets.into_iter().map(|(k, (c, a))| (k, c, a)).collect();
    by_kind.sort_by_key(|x| x.0.symbol());

    let ff_count = module.ffs.len();
    let seq_total = ff_count as f64 * library.ff_area();

    AreaReport {
        total: comb_total + seq_total,
        combinational: comb_total,
        sequential: seq_total,
        by_kind,
        ff_count,
    }
}

#[derive(Clone, Debug)]
pub struct TimingReport {
    pub critical_path_delay: f64,
    pub critical_path_depth: usize,
    pub critical_path: Vec<PathStep>,
    pub endpoint: Option<Endpoint>,
}

impl Default for TimingReport {
    fn default() -> Self {
        Self {
            critical_path_delay: 0.0,
            critical_path_depth: 0,
            critical_path: Vec::new(),
            endpoint: None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct PathStep {
    pub net: NetId,
    pub kind: StepKind,
    pub arrival: f64,
}

#[derive(Clone, Debug)]
pub enum StepKind {
    StartPoint,
    FfOutput(usize),
    CellOutput(usize, CellKind),
    FfInput(usize),
    PortOutput,
}

#[derive(Clone, Debug)]
pub enum Endpoint {
    Ff(usize),
    Port,
}

impl fmt::Display for TimingReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(
            f,
            "timing: critical path delay = {:.3}  depth = {} cells",
            self.critical_path_delay, self.critical_path_depth
        )?;
        if self.critical_path.is_empty() {
            writeln!(f, "  (no combinational path)")?;
        } else {
            writeln!(f, "  path:")?;
            for step in &self.critical_path {
                writeln!(
                    f,
                    "    arrival {:.3}  net n{}  via {}",
                    step.arrival,
                    step.net,
                    format_step_kind(&step.kind),
                )?;
            }
        }
        Ok(())
    }
}

fn format_step_kind(k: &StepKind) -> String {
    match k {
        StepKind::StartPoint => "start (input or constant)".into(),
        StepKind::FfOutput(i) => format!("ff#{} Q", i),
        StepKind::CellOutput(i, kind) => format!("cell#{} {}", i, kind.symbol()),
        StepKind::FfInput(i) => format!("ff#{} D (setup)", i),
        StepKind::PortOutput => "port output".into(),
    }
}

pub fn compute_timing(module: &GateModule, library: &BuiltinLibrary) -> TimingReport {
    // arrival[net] is the latest input-signal arrival time for `net`; FF Q
    // outputs and primary inputs start at 0. predecessor[n] records the
    // fan-in that produced that max, so we can walk the critical path.
    let n_nets = module.nets.len();
    let mut arrival = vec![0.0_f64; n_nets];
    let mut depth = vec![0_usize; n_nets];
    let mut predecessor = vec![None::<NetId>; n_nets];

    // Cell indices already honor topological order for straight expression
    // trees, but branch-merging MUXes can land out of order. Iterate to a
    // fixed point instead of pre-sorting.
    let mut changed = true;
    while changed {
        changed = false;
        for cell in &module.cells {
            let info = library.info(cell.kind);
            let mut max_in_arrival = 0.0_f64;
            let mut max_in_depth = 0_usize;
            let mut arg_net = None;
            for &inp in &cell.inputs {
                let i = inp as usize;
                if arrival[i] > max_in_arrival || arg_net.is_none() {
                    max_in_arrival = arrival[i];
                    arg_net = Some(inp);
                }
                if depth[i] > max_in_depth {
                    max_in_depth = depth[i];
                }
            }
            let out = cell.output as usize;
            let new_arr = max_in_arrival + info.delay;
            let new_depth = max_in_depth + usize::from(!matches!(cell.kind, CellKind::Buf));
            if new_arr > arrival[out] + 1e-12 || new_depth > depth[out] {
                arrival[out] = new_arr;
                depth[out] = new_depth;
                predecessor[out] = arg_net;
                changed = true;
            }
        }
    }

    // Endpoints close the path: FF D pins pay an extra setup cost on top
    // of arrival, primary outputs don't.
    let mut best_delay: f64 = 0.0;
    let mut best_depth: usize = 0;
    let mut best_endpoint: Option<Endpoint> = None;
    let mut best_end_net: Option<NetId> = None;

    for (i, ff) in module.ffs.iter().enumerate() {
        let d = ff.d as usize;
        let t = arrival[d] + library.ff_setup();
        if t > best_delay {
            best_delay = t;
            best_depth = depth[d];
            best_endpoint = Some(Endpoint::Ff(i));
            best_end_net = Some(ff.d);
        }
    }
    for port in &module.ports {
        if matches!(
            port.dir,
            crate::ir::PortDir::Output | crate::ir::PortDir::Inout
        ) {
            for &n in &port.nets {
                let t = arrival[n as usize];
                if t > best_delay {
                    best_delay = t;
                    best_depth = depth[n as usize];
                    best_endpoint = Some(Endpoint::Port);
                    best_end_net = Some(n);
                }
            }
        }
    }

    let mut path = Vec::new();
    if let Some(end_net) = best_end_net {
        let mut visited: HashSet<NetId> = HashSet::new();
        let mut cur = Some(end_net);
        let mut tail_kind = match &best_endpoint {
            Some(Endpoint::Ff(i)) => Some(StepKind::FfInput(*i)),
            Some(Endpoint::Port) => Some(StepKind::PortOutput),
            None => None,
        };

        let mut trace = Vec::new();
        while let Some(n) = cur {
            if !visited.insert(n) {
                break;
            }
            let idx = n as usize;
            let kind = match &module.nets[idx].driver {
                NetDriver::Const(_) => StepKind::StartPoint,
                NetDriver::PortInput => StepKind::StartPoint,
                NetDriver::FfQ(i) => StepKind::FfOutput(*i),
                NetDriver::Cell(i) => {
                    let k = module.cells[*i].kind;
                    StepKind::CellOutput(*i, k)
                }
                NetDriver::Undriven => StepKind::StartPoint,
            };
            trace.push(PathStep {
                net: n,
                kind,
                arrival: arrival[idx],
            });
            match &module.nets[idx].driver {
                NetDriver::Cell(_) => {
                    cur = predecessor[idx];
                }
                _ => break,
            }
        }
        trace.reverse();
        path = trace;
        if let Some(k) = tail_kind.take() {
            path.push(PathStep {
                net: end_net,
                kind: k,
                arrival: best_delay,
            });
        }
    }

    TimingReport {
        critical_path_delay: best_delay,
        critical_path_depth: best_depth,
        critical_path: path,
        endpoint: best_endpoint,
    }
}
