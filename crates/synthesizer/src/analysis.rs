use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::fmt;

use crate::ir::{CellKind, GateModule, NetDriver, NetId, PortDir};
use crate::library::CellLibrary;

fn max_float_width(vs: impl IntoIterator<Item = f64>, prec: usize) -> usize {
    vs.into_iter()
        .map(|v| format!("{:.*}", prec, v).len())
        .max()
        .unwrap_or(1)
}

fn max_int_width(vs: impl IntoIterator<Item = usize>) -> usize {
    vs.into_iter()
        .map(|v| v.to_string().len())
        .max()
        .unwrap_or(1)
}

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
        writeln!(
            f,
            "area: {:.2} um²  (comb {:.2}, seq {:.2})",
            self.total, self.combinational, self.sequential
        )?;
        let kind_counts = self.by_kind.iter().map(|(_, c, _)| *c);
        let kind_areas = self.by_kind.iter().map(|(_, _, a)| *a);
        let ff_count = (self.ff_count > 0).then_some(self.ff_count);
        let ff_area = (self.ff_count > 0).then_some(self.sequential);
        let count_w = max_int_width(kind_counts.chain(ff_count));
        let area_w = max_float_width(kind_areas.chain(ff_area), 2);
        for (kind, count, area) in &self.by_kind {
            writeln!(
                f,
                "  {:<6} ×{:>cw$} {:>aw$.2}",
                kind.symbol(),
                count,
                area,
                cw = count_w,
                aw = area_w
            )?;
        }
        if self.ff_count > 0 {
            writeln!(
                f,
                "  {:<6} ×{:>cw$} {:>aw$.2}",
                "FF",
                self.ff_count,
                self.sequential,
                cw = count_w,
                aw = area_w
            )?;
        }
        Ok(())
    }
}

pub fn compute_area(module: &GateModule, library: &dyn CellLibrary) -> AreaReport {
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

/// Static power estimate. Unlike area/timing, this depends on two user-supplied
/// assumptions: `clock_freq_mhz` (how fast the design is clocked) and
/// `activity` (average per-cycle toggle rate of a combinational output).
/// The model is:
///   P_cell = leakage + internal_energy × activity × f_clk
///   P_ff   = leakage_ff + internal_energy_ff × f_clk     (clock toggles every cycle)
/// Net switching (C × V² × f) is intentionally omitted — it would require a
/// capacitance-per-fanout estimate and adds ~2× complexity for little gain at
/// this accuracy level.
#[derive(Clone, Debug, Default)]
pub struct PowerReport {
    pub total_mw: f64,
    pub leakage_mw: f64,
    pub dynamic_mw: f64,
    pub by_kind: Vec<PowerKindRow>,
    pub ff_count: usize,
    pub ff_leakage_nw: f64,
    pub ff_dynamic_uw: f64,
    pub clock_freq_mhz: f64,
    pub activity: f64,
}

/// Per-cell-kind breakdown row in a [`PowerReport`].
#[derive(Clone, Debug)]
pub struct PowerKindRow {
    pub kind: CellKind,
    pub count: usize,
    pub leakage_nw: f64,
    pub dynamic_uw: f64,
}

impl fmt::Display for PowerReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(
            f,
            "power: {:.4} mW  (leakage {:.4} mW, dynamic {:.4} mW)",
            self.total_mw, self.leakage_mw, self.dynamic_mw
        )?;
        writeln!(
            f,
            "  assumptions: f_clk = {} MHz, activity = {:.2}",
            self.clock_freq_mhz, self.activity
        )?;
        let kind_counts = self.by_kind.iter().map(|r| r.count);
        let kind_leaks = self.by_kind.iter().map(|r| r.leakage_nw);
        let kind_dyns = self.by_kind.iter().map(|r| r.dynamic_uw);
        let ff_count = (self.ff_count > 0).then_some(self.ff_count);
        let ff_leak = (self.ff_count > 0).then_some(self.ff_leakage_nw);
        let ff_dyn = (self.ff_count > 0).then_some(self.ff_dynamic_uw);
        let count_w = max_int_width(kind_counts.chain(ff_count));
        let leak_w = max_float_width(kind_leaks.chain(ff_leak), 3);
        let dyn_w = max_float_width(kind_dyns.chain(ff_dyn), 3);
        for row in &self.by_kind {
            writeln!(
                f,
                "  {:<6} ×{:>cw$}  leak {:>lw$.3} nW  dyn {:>dw$.3} uW",
                row.kind.symbol(),
                row.count,
                row.leakage_nw,
                row.dynamic_uw,
                cw = count_w,
                lw = leak_w,
                dw = dyn_w,
            )?;
        }
        if self.ff_count > 0 {
            writeln!(
                f,
                "  FF     ×{:>cw$}  leak {:>lw$.3} nW  dyn {:>dw$.3} uW",
                self.ff_count,
                self.ff_leakage_nw,
                self.ff_dynamic_uw,
                cw = count_w,
                lw = leak_w,
                dw = dyn_w,
            )?;
        }
        Ok(())
    }
}

pub fn compute_power(
    module: &GateModule,
    library: &dyn CellLibrary,
    clock_freq_mhz: f64,
    activity: f64,
) -> PowerReport {
    let mut buckets: HashMap<CellKind, (usize, f64, f64)> = HashMap::new();
    let mut comb_leak_nw = 0.0_f64;
    let mut comb_dyn_uw = 0.0_f64;
    for cell in &module.cells {
        let info = library.info(cell.kind);
        // P (uW) = energy (pJ/tr) × activity × f_clk (MHz); pJ × MHz = uW.
        let dyn_uw = info.internal_energy * activity * clock_freq_mhz;
        let entry = buckets.entry(cell.kind).or_insert((0, 0.0, 0.0));
        entry.0 += 1;
        entry.1 += info.leakage;
        entry.2 += dyn_uw;
        comb_leak_nw += info.leakage;
        comb_dyn_uw += dyn_uw;
    }
    let mut by_kind: Vec<PowerKindRow> = buckets
        .into_iter()
        .map(|(kind, (count, leakage_nw, dynamic_uw))| PowerKindRow {
            kind,
            count,
            leakage_nw,
            dynamic_uw,
        })
        .collect();
    by_kind.sort_by_key(|r| r.kind.symbol());

    let ff_count = module.ffs.len();
    let ff_leakage_nw = ff_count as f64 * library.ff_leakage();
    let ff_dynamic_uw = ff_count as f64 * library.ff_internal_energy() * clock_freq_mhz;

    let total_leak_nw = comb_leak_nw + ff_leakage_nw;
    let total_dyn_uw = comb_dyn_uw + ff_dynamic_uw;

    PowerReport {
        total_mw: total_leak_nw / 1e6 + total_dyn_uw / 1e3,
        leakage_mw: total_leak_nw / 1e6,
        dynamic_mw: total_dyn_uw / 1e3,
        by_kind,
        ff_count,
        ff_leakage_nw,
        ff_dynamic_uw,
        clock_freq_mhz,
        activity,
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
    /// Port/FF variable this net belongs to, if any — e.g. `("i_instr", 26)`
    /// for bit 26 of port `i_instr`. Captured when the report is built so
    /// Display can reference the source identifier without a GateModule
    /// reference at format time.
    pub origin: Option<(String, usize)>,
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

impl TimingReport {
    /// Short single-line summary for top-N tables: `<ns> ns  <N> levels  <start> → <end>`.
    /// Widths fit 3-digit ns and 4-digit level counts.
    pub fn summary(&self) -> String {
        let start = port_label(self.critical_path.first());
        let end = port_label(self.critical_path.last());
        format!(
            "{:>8.3} ns  {:>5} levels  {} → {}",
            self.critical_path_delay, self.critical_path_depth, start, end
        )
    }

    fn write_path(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.critical_path.is_empty() {
            writeln!(f, "  (no combinational path)")?;
            return Ok(());
        }
        let last_idx = self.critical_path.len() - 1;

        // Build rows first, then measure column widths from the data.
        let rows: Vec<(f64, String, String)> = self
            .critical_path
            .iter()
            .enumerate()
            .scan(0.0_f64, |prev, (i, step)| {
                let is_first = i == 0;
                let is_last = i == last_idx;
                let origin_tag = step.origin.as_ref().map(|(n, b)| format!("{}[{}]", n, b));

                let label = match (&step.kind, is_first, is_last) {
                    (StepKind::StartPoint, _, _) => {
                        origin_tag.clone().unwrap_or_else(|| "input".to_string())
                    }
                    (StepKind::FfOutput(_), true, _) => {
                        origin_tag.clone().unwrap_or_else(|| "FF Q".to_string())
                    }
                    (StepKind::PortOutput, _, _) => {
                        origin_tag.clone().unwrap_or_else(|| "output".to_string())
                    }
                    (StepKind::FfInput(_), _, _) => {
                        origin_tag.clone().unwrap_or_else(|| "FF D".to_string())
                    }
                    (StepKind::CellOutput(_, kind), _, _) => kind.symbol().to_string(),
                    _ => "start".to_string(),
                };

                let tail = if is_first {
                    match &step.kind {
                        StepKind::FfOutput(_) => "(FF Q)".to_string(),
                        _ => "(input)".to_string(),
                    }
                } else if is_last {
                    match &step.kind {
                        StepKind::FfInput(_) => "(FF D, +setup)".to_string(),
                        _ => "(output)".to_string(),
                    }
                } else {
                    format!("+{:.3}", step.arrival - *prev)
                };

                *prev = step.arrival;
                Some((step.arrival, label, tail))
            })
            .collect();

        let arr_w = max_float_width(rows.iter().map(|(a, _, _)| *a), 3).max(5);
        let label_w = rows.iter().map(|(_, l, _)| l.len()).max().unwrap_or(4);

        for (arrival, label, tail) in rows {
            writeln!(
                f,
                "  {:>aw$.3}  {:<lw$}  {}",
                arrival,
                label,
                tail,
                aw = arr_w,
                lw = label_w
            )?;
        }
        Ok(())
    }
}

/// Extract the port/FF label from a path step, or `?` when the step has no origin.
pub fn port_label(step: Option<&PathStep>) -> String {
    match step.and_then(|s| s.origin.as_ref()) {
        Some((name, bit)) => format!("{}[{}]", name, bit),
        None => "?".to_string(),
    }
}

impl fmt::Display for TimingReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "timing: {}", self.summary())?;
        self.write_path(f)
    }
}

pub fn compute_timing(module: &GateModule, library: &dyn CellLibrary) -> TimingReport {
    compute_timing_top_n(module, library, 1)
        .into_iter()
        .next()
        .unwrap_or_default()
}

/// Returns the `n` endpoints with the longest arrival time, each with its
/// critical path, worst first. A single top-1 can shadow a close second;
/// top-N exposes near-critical paths a different tech-mapping might prefer.
pub fn compute_timing_top_n(
    module: &GateModule,
    library: &dyn CellLibrary,
    n: usize,
) -> Vec<TimingReport> {
    if n == 0 {
        return Vec::new();
    }

    // arrival[net] = latest input arrival for `net`; FF Q and primary inputs start at 0.
    // predecessor[n] = fan-in that produced the max, used to walk the path back.
    let n_nets = module.nets.len();
    let mut arrival = vec![0.0_f64; n_nets];
    let mut depth = vec![0_usize; n_nets];
    let mut predecessor = vec![None::<NetId>; n_nets];

    // Cells are roughly topological but branch-merging MUXes can land out of
    // order; iterate to a fixed point rather than pre-sort.
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

    // FF D pins pay an extra setup cost on top of their arrival; primary outputs don't.
    let mut endpoints: Vec<(f64, usize, Endpoint, NetId)> = Vec::new();
    for (i, ff) in module.ffs.iter().enumerate() {
        let d = ff.d as usize;
        let t = arrival[d] + library.ff_setup();
        endpoints.push((t, depth[d], Endpoint::Ff(i), ff.d));
    }
    for port in &module.ports {
        if matches!(port.dir, PortDir::Output | PortDir::Inout) {
            for &net in &port.nets {
                let t = arrival[net as usize];
                endpoints.push((t, depth[net as usize], Endpoint::Port, net));
            }
        }
    }

    // Sort worst-first, then dedup by net so a wire driving both a FF D and a
    // port output doesn't appear twice; the FF-D variant wins (includes setup).
    endpoints.sort_by(|a, b| {
        b.0.partial_cmp(&a.0)
            .unwrap_or(Ordering::Equal)
            .then_with(|| a.3.cmp(&b.3))
    });
    let mut seen_nets: HashSet<NetId> = HashSet::new();
    endpoints.retain(|(_, _, _, net)| seen_nets.insert(*net));

    // endpoint_label: net → (port/register name, bit). Without this, the final
    // step would inherit whatever intermediate let-variable shares the net via
    // cell-output origin propagation.
    let mut endpoint_label: HashMap<NetId, (String, usize)> = HashMap::new();
    for port in &module.ports {
        if matches!(port.dir, PortDir::Output | PortDir::Inout) {
            for (bit, &net) in port.nets.iter().enumerate() {
                endpoint_label.insert(net, (port.name.to_string(), bit));
            }
        }
    }
    // FF D/Q nets prefer the FF's own `origin` (register variable) over an
    // inherited one, so Q-outputs and D-inputs report the register name.
    for ff in &module.ffs {
        if let Some((name, bit)) = ff.origin {
            endpoint_label.insert(ff.d, (name.to_string(), bit));
            endpoint_label.insert(ff.q, (name.to_string(), bit));
        }
    }

    endpoints
        .into_iter()
        .take(n)
        .map(|(delay, dep, endpoint, end_net)| {
            let mut visited: HashSet<NetId> = HashSet::new();
            let mut cur = Some(end_net);
            let tail_kind = match &endpoint {
                Endpoint::Ff(i) => StepKind::FfInput(*i),
                Endpoint::Port => StepKind::PortOutput,
            };
            let mut trace = Vec::new();
            while let Some(net) = cur {
                if !visited.insert(net) {
                    break;
                }
                let idx = net as usize;
                let kind = match &module.nets[idx].driver {
                    NetDriver::Const(_) => StepKind::StartPoint,
                    NetDriver::PortInput => StepKind::StartPoint,
                    NetDriver::FfQ(i) => StepKind::FfOutput(*i),
                    NetDriver::Cell(i) => StepKind::CellOutput(*i, module.cells[*i].kind),
                    NetDriver::Undriven => StepKind::StartPoint,
                };
                // Prefer explicit boundary label; fall back to net origin.
                let origin = endpoint_label
                    .get(&net)
                    .cloned()
                    .or_else(|| module.nets[idx].origin.map(|(s, bit)| (s.to_string(), bit)));
                trace.push(PathStep {
                    net,
                    kind,
                    arrival: arrival[idx],
                    origin,
                });
                match &module.nets[idx].driver {
                    NetDriver::Cell(_) => cur = predecessor[idx],
                    _ => break,
                }
            }
            trace.reverse();
            let end_origin = endpoint_label.get(&end_net).cloned().or_else(|| {
                module.nets[end_net as usize]
                    .origin
                    .map(|(s, bit)| (s.to_string(), bit))
            });
            trace.push(PathStep {
                net: end_net,
                kind: tail_kind,
                arrival: delay,
                origin: end_origin,
            });
            TimingReport {
                critical_path_delay: delay,
                critical_path_depth: dep,
                critical_path: trace,
                endpoint: Some(endpoint),
            }
        })
        .collect()
}
