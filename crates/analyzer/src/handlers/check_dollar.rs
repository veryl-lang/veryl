use crate::analyzer_error::AnalyzerError;
use veryl_parser::veryl_grammar_trait::*;
use veryl_parser::veryl_walker::{Handler, HandlerPoint, VerylWalker};
use veryl_parser::ParolError;
use veryl_parser::Stringifier;

#[derive(Default)]
pub struct CheckDollar<'a> {
    pub errors: Vec<AnalyzerError>,
    text: &'a str,
    point: HandlerPoint,
}

impl<'a> CheckDollar<'a> {
    pub fn new(text: &'a str) -> Self {
        Self {
            text,
            ..Default::default()
        }
    }
}

impl<'a> Handler for CheckDollar<'a> {
    fn set_point(&mut self, p: HandlerPoint) {
        self.point = p;
    }
}

impl<'a> VerylGrammarTrait for CheckDollar<'a> {
    fn scoped_identifier(&mut self, arg: &ScopedIdentifier) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            // if Dollar exists
            if arg.scoped_identifier_opt.is_some() {
                let name = arg.identifier.identifier_token.text();
                match name.as_str() {
                    name if DEFINED_NAMESPACES.contains(&name) => {
                        if arg.scoped_identifier_list.is_empty() {
                            self.errors.push(AnalyzerError::undefined_identifier(
                                &format!("${}", name),
                                self.text,
                                &arg.identifier.identifier_token,
                            ));
                        }
                    }
                    name => {
                        self.errors.push(AnalyzerError::invalid_namespace(
                            name,
                            self.text,
                            &arg.identifier.identifier_token,
                        ));
                    }
                }
            }
        }
        Ok(())
    }

    fn expression_identifier(&mut self, arg: &ExpressionIdentifier) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            // if Dollar exists
            if arg.expression_identifier_opt.is_some() {
                match arg.expression_identifier_group.as_ref() {
                    // Namespace
                    ExpressionIdentifierGroup::ColonColonIdentifierExpressionIdentifierGroupListExpressionIdentifierGroupList0(_) => {
                        match arg.identifier.identifier_token.text().as_str() {
                            name if DEFINED_NAMESPACES.contains(&name) => (),
                            name => {
                                self.errors.push(AnalyzerError::invalid_namespace(
                                    name,
                                    self.text,
                                    &arg.identifier.identifier_token,
                                ));
                            }
                        }
                    },
                    // System function call
                    _ => {
                        let mut stringifier = Stringifier::new();
                        stringifier.expression_identifier(arg);
                        match stringifier.as_str() {
                            name if DEFINED_SYSTEM_FUNCTIONS.contains(&name) => (),
                            name => {
                                self.errors.push(AnalyzerError::invalid_system_function(
                                    name,
                                    self.text,
                                    &arg.identifier.identifier_token,
                                ));
                            }
                        }
                    },
                }
            }
        }
        Ok(())
    }
}

const DEFINED_NAMESPACES: [&str; 1] = ["sv"];

// Refer IEEE Std 1800-2012  Clause 20 and 21
const DEFINED_SYSTEM_FUNCTIONS: [&str; 196] = [
    "$acos",
    "$acosh",
    "$asin",
    "$asinh",
    "$assertcontrol",
    "$assertfailoff",
    "$assertfailon",
    "$assertkill",
    "$assertnonvacuouson",
    "$assertoff",
    "$asserton",
    "$assertpassoff",
    "$assertpasson",
    "$assertvacuousoff",
    "$async$and$array",
    "$async$and$plane",
    "$async$nand$array",
    "$async$nand$plane",
    "$async$nor$array",
    "$async$nor$plane",
    "$async$or$array",
    "$async$or$plane",
    "$atan",
    "$atan2",
    "$atanh",
    "$bits",
    "$bitstoreal",
    "$bitstoshortreal",
    "$cast",
    "$ceil",
    "$changed",
    "$changed_gclk",
    "$changing_gclk",
    "$clog2",
    "$cos",
    "$cosh",
    "$countbits",
    "$countones",
    "$coverage_control",
    "$coverage_get",
    "$coverage_get_max",
    "$coverage_merge",
    "$coverage_save",
    "$dimensions",
    "$display",
    "$displayb",
    "$displayh",
    "$displayo",
    "$dist_chi_square",
    "$dist_erlang",
    "$dist_exponential",
    "$dist_normal",
    "$dist_poisson",
    "$dist_t",
    "$dist_uniform",
    "$dumpall",
    "$dumpfile",
    "$dumpflush",
    "$dumplimit",
    "$dumpoff",
    "$dumpon",
    "$dumpports",
    "$dumpportsall",
    "$dumpportsflush",
    "$dumpportslimit",
    "$dumpportsoff",
    "$dumpportson",
    "$dumpvars",
    "$error",
    "$exit",
    "$exp",
    "$falling_gclk",
    "$fatal",
    "$fclose",
    "$fdisplay",
    "$fdisplayb",
    "$fdisplayh",
    "$fdisplayo",
    "$fell",
    "$fell_gclk",
    "$feof",
    "$ferror",
    "$fflush",
    "$fgetc",
    "$fgets",
    "$finish",
    "$floor",
    "$fmonitor",
    "$fmonitorb",
    "$fmonitorh",
    "$fmonitoro",
    "$fopen",
    "$fread",
    "$fscanf",
    "$fseek",
    "$fstrobe",
    "$fstrobeb",
    "$fstrobeh",
    "$fstrobeo",
    "$ftell",
    "$future_gclk",
    "$fwrite",
    "$fwriteb",
    "$fwriteh",
    "$fwriteo",
    "$get_coverage",
    "$high",
    "$hypot",
    "$increment",
    "$info",
    "$isunbounded",
    "$isunknown",
    "$itor",
    "$left",
    "$ln",
    "$load_coverage_db",
    "$log10",
    "$low",
    "$monitor",
    "$monitorb",
    "$monitorh",
    "$monitoro",
    "$monitoroff",
    "$monitoron",
    "$onehot",
    "$onehot0",
    "$past",
    "$past_gclk",
    "$pow",
    "$printtimescale",
    "$q_add",
    "$q_exam",
    "$q_full",
    "$q_initialize",
    "$q_remove",
    "$random",
    "$readmemb",
    "$readmemh",
    "$realtime",
    "$realtobits",
    "$rewind",
    "$right",
    "$rising_gclk",
    "$rose",
    "$rose_gclk",
    "$rtoi",
    "$sampled",
    "$set_coverage_db_name",
    "$sformat",
    "$sformatf",
    "$shortrealtobits",
    "$signed",
    "$sin",
    "$sinh",
    "$size",
    "$sqrt",
    "$sscanf",
    "$stable",
    "$stable_gclk",
    "$steady_gclk",
    "$stime",
    "$stop",
    "$strobe",
    "$strobeb",
    "$strobeh",
    "$strobeo",
    "$swrite",
    "$swriteb",
    "$swriteh",
    "$swriteo",
    "$sync$and$array",
    "$sync$and$plane",
    "$sync$nand$array",
    "$sync$nand$plane",
    "$sync$nor$array",
    "$sync$nor$plane",
    "$sync$or$array",
    "$sync$or$plane",
    "$system",
    "$tan",
    "$tanh",
    "$test$plusargs",
    "$time",
    "$timeformat",
    "$typename",
    "$ungetc",
    "$unpacked_dimensions",
    "$unsigned",
    "$value$plusargs",
    "$warning",
    "$write",
    "$writeb",
    "$writeh",
    "$writememb",
    "$writememh",
    "$writeo",
];
