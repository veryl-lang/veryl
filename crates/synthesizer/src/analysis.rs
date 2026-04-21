use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::fmt;

use crate::ir::{CellKind, GateModule, NetDriver, NetId, PortDir};
use crate::library::BuiltinLibrary;

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
            "area: {:.2} um²  (comb {:.2}, seq {:.2} × {} FF)",
            self.total, self.combinational, self.sequential, self.ff_count
        )?;
        // Size count / area columns to the widest entry so everything
        // stays aligned even for designs with 6-digit cell counts and
        // 7-digit areas (e.g. heliodor_top).
        let count_w = self
            .by_kind
            .iter()
            .map(|(_, c, _)| c.to_string().len())
            .max()
            .unwrap_or(1);
        let area_w = self
            .by_kind
            .iter()
            .map(|(_, _, a)| format!("{:.2}", a).len())
            .max()
            .unwrap_or(1);
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
    /// Short single-line summary, suitable for top-N tables. Format:
    /// `0.580 ns   6 gates   i_imm_type[0] → o_imm[1]`.
    /// Widths accommodate up to 3-digit ns (e.g. flatten-synthesised
    /// CPU cores at ~130 ns) and 4-digit gate counts without wrecking
    /// column alignment.
    pub fn summary(&self) -> String {
        let start = port_label(self.critical_path.first());
        let end = port_label(self.critical_path.last());
        format!(
            "{:>8.3} ns  {:>5} gates  {} → {}",
            self.critical_path_delay, self.critical_path_depth, start, end
        )
    }

    fn write_path(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.critical_path.is_empty() {
            writeln!(f, "  (no combinational path)")?;
            return Ok(());
        }
        let last_idx = self.critical_path.len() - 1;

        // Build all per-step label strings first so we can measure column
        // widths from the actual data rather than guessing a minimum. This
        // keeps arrival / label columns aligned regardless of path length
        // or which port names happen to appear.
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

        let arr_w = rows
            .iter()
            .map(|(a, _, _)| format!("{:.3}", a).len())
            .max()
            .unwrap_or(5);
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

/// Extract the port/FF label from a path step, falling back to `?` when
/// the step has no origin. Shared between summary and per-path rendering.
fn port_label(step: Option<&PathStep>) -> String {
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

pub fn compute_timing(module: &GateModule, library: &BuiltinLibrary) -> TimingReport {
    compute_timing_top_n(module, library, 1)
        .into_iter()
        .next()
        .unwrap_or_default()
}

/// Returns the `n` endpoints with the longest arrival time, each paired
/// with its critical path. The list is sorted with the worst first. Useful
/// for spotting cases where the absolute top path is accidentally shadowed
/// by a close second (e.g. when a different tech-mapping would pick
/// another endpoint as critical).
pub fn compute_timing_top_n(
    module: &GateModule,
    library: &BuiltinLibrary,
    n: usize,
) -> Vec<TimingReport> {
    if n == 0 {
        return Vec::new();
    }

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

    // Collect every candidate endpoint along with its effective delay.
    // FF D pins pay an extra setup cost on top of their arrival; primary
    // outputs don't. The endpoint's `net` is what we trace back from.
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

    // Sort endpoints by delay descending; deduplicate by the `net` field
    // so reporting the same wire twice (e.g. a net driving both a FF d
    // and a port output) doesn't spam the top-N list. We keep the higher-
    // delay variant (FF d with setup vs port arrival without setup).
    endpoints.sort_by(|a, b| {
        b.0.partial_cmp(&a.0)
            .unwrap_or(Ordering::Equal)
            .then_with(|| a.3.cmp(&b.3))
    });
    let mut seen_nets: HashSet<NetId> = HashSet::new();
    endpoints.retain(|(_, _, _, net)| seen_nets.insert(*net));

    // Build a report for each of the top n endpoints. Path tracing uses
    // the shared predecessor table, so the cost of adding more reports
    // is O(path_length) per extra report — cheap.
    // Build per-endpoint-net → (name, bit-index) reverse index so end-point
    // labels show the destination (output port or register variable)
    // rather than whichever intermediate let-variable happened to share
    // the origin net via cell-output inheritance.
    let mut endpoint_label: HashMap<NetId, (String, usize)> = HashMap::new();
    for port in &module.ports {
        if matches!(port.dir, PortDir::Output | PortDir::Inout) {
            for (bit, &net) in port.nets.iter().enumerate() {
                endpoint_label.insert(net, (port.name.to_string(), bit));
            }
        }
    }
    // FF D / Q nets prefer the FF's own `origin` (the register variable
    // it stores) over the net's inherited origin. Without this, we'd
    // report e.g. the first cell-input variable propagated through
    // add_cell even though the net is actually at a FF boundary.
    //
    // This map is used both for path endpoints (FF D) and for any step
    // in the trace that sits on an FF boundary (Q outputs at path start).
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
                // Prefer the explicit endpoint label when the net sits
                // on a port/FF boundary (avoids misleading cell-input-
                // inherited origins for FF Q outputs and port outputs).
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
            // Prefer the dedicated endpoint label (output port or FF
            // register variable). Fallback to the net's stored origin
            // only when no explicit label is registered.
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
