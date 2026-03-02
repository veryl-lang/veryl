use std::fmt;
use veryl_analyzer::ir::VarId;

//#[derive(Clone)]
//pub struct DeclarationPtr(pub *const Declaration);
//
//unsafe impl Send for DeclarationPtr {}
//unsafe impl Sync for DeclarationPtr {}
//
//#[derive(Clone)]
//pub struct Declaration {
//    pub clock: Option<VarId>,
//    pub reset: Option<VarId>,
//    pub statements: Vec<Statement>,
//    pub binary: Option<(FuncPtr, Vec<*mut Value>)>,
//}
//
//impl Declaration {
//    pub fn eval_step(&self, event: &Event) {
//        let (update, reset) = if let Some(clock) = &self.clock {
//            match event {
//                Event::Clock(x) => (x == clock, false),
//                Event::Reset(x) => {
//                    if let Some(reset) = &self.reset {
//                        let ret = x == reset;
//                        (ret, ret)
//                    } else {
//                        (false, false)
//                    }
//                }
//                _ => (false, false),
//            }
//        } else {
//            (true, false)
//        };
//
//        if update {
//            if let Some((func, args)) = &self.binary {
//                unsafe {
//                    func(reset, args.as_ptr());
//                }
//            } else {
//                for x in &self.statements {
//                    x.eval_step(reset);
//                }
//            }
//        }
//    }
//}
//
//unsafe impl Send for Declaration {}
//unsafe impl Sync for Declaration {}
//
//impl fmt::Display for Declaration {
//    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
//        let mut ret = if let Some(clock) = &self.clock {
//            if let Some(x) = &self.reset {
//                format!("ff ({}, {}) {{\n", clock, x)
//            } else {
//                format!("ff ({}) {{\n", clock)
//            }
//        } else {
//            "comb {\n".to_string()
//        };
//
//        for x in &self.statements {
//            let text = format!("{}\n", x);
//            ret.push_str(&indent_all_by(2, text));
//        }
//
//        ret.push('}');
//        ret.fmt(f)
//    }
//}

#[derive(Clone)]
pub struct Clock {
    pub id: VarId,
    pub index: Option<usize>,
    pub select: Option<usize>,
}

impl fmt::Display for Clock {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut ret = format!("{}", self.id);
        if let Some(index) = self.index {
            ret.push_str(&format!("[{index}]"));
        }
        if let Some(select) = self.select {
            ret.push_str(&format!("[{select}]"));
        }
        ret.fmt(f)
    }
}

#[derive(Clone)]
pub struct Reset {
    pub id: VarId,
    pub index: Option<usize>,
    pub select: Option<usize>,
}

impl fmt::Display for Reset {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut ret = format!("{}", self.id);
        if let Some(index) = self.index {
            ret.push_str(&format!("[{index}]"));
        }
        if let Some(select) = self.select {
            ret.push_str(&format!("[{select}]"));
        }
        ret.fmt(f)
    }
}
