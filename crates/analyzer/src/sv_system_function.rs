use crate::namespace::Namespace;
use crate::symbol::{
    ClockDomain, Direction, DocComment, Port, PortProperty, Symbol, SymbolKind,
    SystemFuncitonProperty, Type, TypeKind,
};
use crate::symbol_table::SymbolTable;
use veryl_parser::token_range::TokenRange;
use veryl_parser::veryl_token::{Token, TokenSource, VerylToken};

pub struct SvSystemFunction {
    pub name: String,
    pub ports: Vec<(String, Direction)>,
}

impl SvSystemFunction {
    pub fn new(name: &str, ports: &[(&str, Direction)]) -> Self {
        Self {
            name: name.to_string(),
            ports: ports
                .iter()
                .map(|(name, direction)| (name.to_string(), *direction))
                .collect(),
        }
    }
}

pub fn insert_symbols(symbol_table: &mut SymbolTable, namespace: &Namespace) {
    let mut namespace = namespace.clone();

    for func in sv_system_functions() {
        let token = Token::new(&func.name, 0, 0, 0, 0, TokenSource::Builtin);
        let mut ports = Vec::new();

        namespace.push(token.text);
        for (name, direction) in &func.ports {
            let token = Token::new(name, 0, 0, 0, 0, TokenSource::Builtin);
            let r#type = Type {
                modifier: vec![],
                kind: TypeKind::Any,
                width: vec![],
                array: vec![],
                array_type: None,
                is_const: false,
                token: TokenRange::default(),
            };
            let property = PortProperty {
                token,
                r#type,
                direction: *direction,
                prefix: None,
                suffix: None,
                clock_domain: ClockDomain::None,
                default_value: None,
                is_proto: false,
            };
            let symbol = Symbol::new(
                &token,
                SymbolKind::Port(property),
                &namespace,
                false,
                DocComment::default(),
            );
            if let Some(id) = symbol_table.insert(&token, symbol) {
                let port = Port {
                    token: VerylToken::new(token),
                    symbol: id,
                };
                ports.push(port);
            }
        }
        namespace.pop();

        let property = SystemFuncitonProperty { ports };
        let symbol = Symbol::new(
            &token,
            SymbolKind::SystemFunction(property),
            &namespace,
            false,
            DocComment::default(),
        );
        let _ = symbol_table.insert(&token, symbol);
    }
}

// Refer IEEE Std 1800-2023  Clause 20 and 21
pub fn sv_system_functions() -> Vec<SvSystemFunction> {
    vec![
        // Simulation control system tasks
        SvSystemFunction::new(
            "$stop",
            // it has optional args but not supported
            &[],
        ),
        SvSystemFunction::new(
            "$finish",
            // it has optional args but not supported
            &[],
        ),
        SvSystemFunction::new("$exit", &[]),
        // Simulation time system functions
        SvSystemFunction::new("$time", &[]),
        SvSystemFunction::new("$stime", &[]),
        SvSystemFunction::new("$realtime", &[]),
        // Timescale system tasks and system functions
        SvSystemFunction::new(
            "$timeunit",
            // it has optional args but not supported
            &[],
        ),
        SvSystemFunction::new(
            "$timeprecision",
            // it has optional args but not supported
            &[],
        ),
        SvSystemFunction::new(
            "$printtimescale",
            // it has optional args but not supported
            &[],
        ),
        SvSystemFunction::new(
            "$timeformat",
            // it has optional args but not supported
            &[],
        ),
        // Conversion functions
        SvSystemFunction::new("$rtoi", &[("real_val", Direction::Input)]),
        SvSystemFunction::new("$itor", &[("int_val", Direction::Input)]),
        SvSystemFunction::new("$realtobits", &[("real_val", Direction::Input)]),
        SvSystemFunction::new("$bitstoreal", &[("bit_val", Direction::Input)]),
        SvSystemFunction::new("$shortrealtobits", &[("shortreal_val", Direction::Input)]),
        SvSystemFunction::new("$bitstoshortreal", &[("bit_val", Direction::Input)]),
        SvSystemFunction::new("$signed", &[("val", Direction::Input)]),
        SvSystemFunction::new("$unsigned", &[("val", Direction::Input)]),
        SvSystemFunction::new(
            "$cast",
            &[
                ("dest_variable", Direction::Output),
                ("source_expression", Direction::Input),
            ],
        ),
        // Data query functions
        SvSystemFunction::new(
            "$typename",
            &[("expression_or_data_type", Direction::Input)],
        ),
        SvSystemFunction::new("$bits", &[("expression_or_data_type", Direction::Input)]),
        SvSystemFunction::new("$isunbounded", &[("identifier", Direction::Input)]),
        // Array query functions
        SvSystemFunction::new(
            "$dimensions",
            // it has optional args but not supported
            &[("expression_or_data_type", Direction::Input)],
        ),
        SvSystemFunction::new(
            "$unpacked_dimensions",
            // it has optional args but not supported
            &[("expression_or_data_type", Direction::Input)],
        ),
        SvSystemFunction::new("$left", &[("expression_or_data_type", Direction::Input)]),
        SvSystemFunction::new("$right", &[("expression_or_data_type", Direction::Input)]),
        SvSystemFunction::new("$low", &[("expression_or_data_type", Direction::Input)]),
        SvSystemFunction::new("$high", &[("expression_or_data_type", Direction::Input)]),
        SvSystemFunction::new(
            "$increment",
            &[("expression_or_data_type", Direction::Input)],
        ),
        SvSystemFunction::new("$size", &[("expression_or_data_type", Direction::Input)]),
        // Math functions
        SvSystemFunction::new("$clog2", &[("n", Direction::Input)]),
        SvSystemFunction::new("$ln", &[("x", Direction::Input)]),
        SvSystemFunction::new("$log10", &[("x", Direction::Input)]),
        SvSystemFunction::new("$exp", &[("x", Direction::Input)]),
        SvSystemFunction::new("$sqrt", &[("x", Direction::Input)]),
        SvSystemFunction::new("$pow", &[("x", Direction::Input), ("y", Direction::Input)]),
        SvSystemFunction::new("$floor", &[("x", Direction::Input)]),
        SvSystemFunction::new("$ceil", &[("x", Direction::Input)]),
        SvSystemFunction::new("$sin", &[("x", Direction::Input)]),
        SvSystemFunction::new("$cos", &[("x", Direction::Input)]),
        SvSystemFunction::new("$tan", &[("x", Direction::Input)]),
        SvSystemFunction::new("$asin", &[("x", Direction::Input)]),
        SvSystemFunction::new("$acos", &[("x", Direction::Input)]),
        SvSystemFunction::new("$atan", &[("x", Direction::Input)]),
        SvSystemFunction::new(
            "$atan2",
            &[("x", Direction::Input), ("y", Direction::Input)],
        ),
        SvSystemFunction::new(
            "$hypot",
            &[("x", Direction::Input), ("y", Direction::Input)],
        ),
        SvSystemFunction::new("$sinh", &[("x", Direction::Input)]),
        SvSystemFunction::new("$cosh", &[("x", Direction::Input)]),
        SvSystemFunction::new("$tanh", &[("x", Direction::Input)]),
        SvSystemFunction::new("$asinh", &[("x", Direction::Input)]),
        SvSystemFunction::new("$acosh", &[("x", Direction::Input)]),
        SvSystemFunction::new("$atanh", &[("x", Direction::Input)]),
        // Bit vector system functions
        SvSystemFunction::new(
            "$countbits",
            // it has optional args but not supported
            &[
                ("expression", Direction::Input),
                ("control_bit", Direction::Input),
            ],
        ),
        SvSystemFunction::new("$countones", &[("expression", Direction::Input)]),
        SvSystemFunction::new("$onehot", &[("expression", Direction::Input)]),
        SvSystemFunction::new("$onehot0", &[("expression", Direction::Input)]),
        SvSystemFunction::new("$isunknown", &[("expression", Direction::Input)]),
        // Severity system tasks
        SvSystemFunction::new(
            "$fatal",
            // it has optional args but not supported
            &[],
        ),
        SvSystemFunction::new(
            "$error",
            // it has optional args but not supported
            &[],
        ),
        SvSystemFunction::new(
            "$warning",
            // it has optional args but not supported
            &[],
        ),
        SvSystemFunction::new(
            "$info",
            // it has optional args but not supported
            &[],
        ),
        // Assertion control system tasks
        SvSystemFunction::new(
            "$asserton",
            // it has optional args but not supported
            &[],
        ),
        SvSystemFunction::new(
            "$assertoff",
            // it has optional args but not supported
            &[],
        ),
        SvSystemFunction::new(
            "$assertkill",
            // it has optional args but not supported
            &[],
        ),
        SvSystemFunction::new(
            "$assertpasson",
            // it has optional args but not supported
            &[],
        ),
        SvSystemFunction::new(
            "$assertpassoff",
            // it has optional args but not supported
            &[],
        ),
        SvSystemFunction::new(
            "$assertfailon",
            // it has optional args but not supported
            &[],
        ),
        SvSystemFunction::new(
            "$assertfailoff",
            // it has optional args but not supported
            &[],
        ),
        SvSystemFunction::new(
            "$assertnonvacuouson",
            // it has optional args but not supported
            &[],
        ),
        SvSystemFunction::new(
            "$assertvacuousoff",
            // it has optional args but not supported
            &[],
        ),
        SvSystemFunction::new(
            "$assertcontrol",
            // it has optional args but not supported
            &[("control_type", Direction::Input)],
        ),
        // Sampled value system functions
        SvSystemFunction::new("$sampled", &[("expression", Direction::Input)]),
        SvSystemFunction::new(
            "$rose",
            // it has optional args but not supported
            &[("expression", Direction::Input)],
        ),
        SvSystemFunction::new(
            "$fell",
            // it has optional args but not supported
            &[("expression", Direction::Input)],
        ),
        SvSystemFunction::new(
            "$stable",
            // it has optional args but not supported
            &[("expression", Direction::Input)],
        ),
        SvSystemFunction::new(
            "$changed",
            // it has optional args but not supported
            &[("expression", Direction::Input)],
        ),
        SvSystemFunction::new(
            "$past",
            // it has optional args but not supported
            &[("expression", Direction::Input)],
        ),
        SvSystemFunction::new("$past_gclk", &[("expression", Direction::Input)]),
        SvSystemFunction::new("$rose_gclk", &[("expression", Direction::Input)]),
        SvSystemFunction::new("$fell_gclk", &[("expression", Direction::Input)]),
        SvSystemFunction::new("$stable_gclk", &[("expression", Direction::Input)]),
        SvSystemFunction::new("$changed_gclk", &[("expression", Direction::Input)]),
        SvSystemFunction::new("$future_gclk", &[("expression", Direction::Input)]),
        SvSystemFunction::new("$rising_gclk", &[("expression", Direction::Input)]),
        SvSystemFunction::new("$falling_gclk", &[("expression", Direction::Input)]),
        SvSystemFunction::new("$steady_gclk", &[("expression", Direction::Input)]),
        SvSystemFunction::new("$changing_gclk", &[("expression", Direction::Input)]),
        // Built-in coverage access system functions
        SvSystemFunction::new(
            "$coverage_control",
            &[
                ("control_constant,", Direction::Input),
                ("coverage_type", Direction::Input),
                ("scope_def", Direction::Input),
                ("modules_or_instance,", Direction::Input),
            ],
        ),
        SvSystemFunction::new(
            "$coverage_get_max",
            &[
                ("coverage_type", Direction::Input),
                ("scope_def", Direction::Input),
                ("modules_or_instance,", Direction::Input),
            ],
        ),
        SvSystemFunction::new(
            "$coverage_get",
            &[
                ("coverage_type", Direction::Input),
                ("scope_def", Direction::Input),
                ("modules_or_instance,", Direction::Input),
            ],
        ),
        SvSystemFunction::new(
            "$coverage_merge",
            &[
                ("coverage_type", Direction::Input),
                ("name", Direction::Input),
            ],
        ),
        SvSystemFunction::new(
            "$coverage_save",
            &[
                ("coverage_type", Direction::Input),
                ("name", Direction::Input),
            ],
        ),
        // Predefined coverage system tasks and system functions
        SvSystemFunction::new("$set_coverage_db_name", &[("filename", Direction::Input)]),
        SvSystemFunction::new("$load_coverage_db", &[("filename", Direction::Input)]),
        SvSystemFunction::new("$get_coverage", &[]),
        // Probabilistic distribution functions
        SvSystemFunction::new(
            "$random",
            // it has optional args but not supported
            &[],
        ),
        SvSystemFunction::new(
            "$urandom",
            // it has optional args but not supported
            &[],
        ),
        SvSystemFunction::new(
            "$random_range",
            // it has optional args but not supported
            &[("maxval", Direction::Input)],
        ),
        SvSystemFunction::new(
            "$dist_uniform",
            &[
                ("seed", Direction::Input),
                ("start", Direction::Input),
                ("end", Direction::Input),
            ],
        ),
        SvSystemFunction::new(
            "$dist_normal",
            &[
                ("seed", Direction::Input),
                ("mean", Direction::Input),
                ("standard_deviation", Direction::Input),
            ],
        ),
        SvSystemFunction::new(
            "$dist_exponential",
            &[("seed", Direction::Input), ("mean", Direction::Input)],
        ),
        SvSystemFunction::new(
            "$dist_poisson",
            &[("seed", Direction::Input), ("mean", Direction::Input)],
        ),
        SvSystemFunction::new(
            "$dist_chi_square",
            &[
                ("seed", Direction::Input),
                ("degree_of_freedom", Direction::Input),
            ],
        ),
        SvSystemFunction::new(
            "$dist_t",
            &[
                ("seed", Direction::Input),
                ("degree_of_freedom", Direction::Input),
            ],
        ),
        SvSystemFunction::new(
            "$dist_erlang",
            &[
                ("seed", Direction::Input),
                ("k_stage", Direction::Input),
                ("mean", Direction::Input),
            ],
        ),
        // Stochastic analysis tasks and functions
        SvSystemFunction::new(
            "$q_initialize",
            &[
                ("q_id", Direction::Input),
                ("q_type", Direction::Input),
                ("max_length", Direction::Input),
                ("status", Direction::Output),
            ],
        ),
        SvSystemFunction::new(
            "$q_add",
            &[
                ("q_id", Direction::Input),
                ("job_id", Direction::Input),
                ("inform_id", Direction::Input),
                ("status", Direction::Output),
            ],
        ),
        SvSystemFunction::new(
            "$q_remove",
            &[
                ("q_id", Direction::Input),
                ("job_id", Direction::Input),
                ("inform_id", Direction::Input),
                ("status", Direction::Output),
            ],
        ),
        SvSystemFunction::new(
            "$q_full",
            &[("q_id", Direction::Input), ("status", Direction::Output)],
        ),
        SvSystemFunction::new(
            "$q_exam",
            &[
                ("q_id", Direction::Input),
                ("q_stat_code", Direction::Input),
                ("q_stat_value", Direction::Input),
                ("status", Direction::Output),
            ],
        ),
        // Programmable logic array modeling system tasks
        SvSystemFunction::new(
            "$async$and$array",
            &[
                ("memory_identifier", Direction::Input),
                ("input_terms", Direction::Input),
                ("output_terms", Direction::Output),
            ],
        ),
        SvSystemFunction::new(
            "$sync$and$array",
            &[
                ("memory_identifier", Direction::Input),
                ("input_terms", Direction::Input),
                ("output_terms", Direction::Output),
            ],
        ),
        SvSystemFunction::new(
            "$async$and$plane",
            &[
                ("memory_identifier", Direction::Input),
                ("input_terms", Direction::Input),
                ("output_terms", Direction::Output),
            ],
        ),
        SvSystemFunction::new(
            "$sync$and$plane",
            &[
                ("memory_identifier", Direction::Input),
                ("input_terms", Direction::Input),
                ("output_terms", Direction::Output),
            ],
        ),
        SvSystemFunction::new(
            "$async$nand$array",
            &[
                ("memory_identifier", Direction::Input),
                ("input_terms", Direction::Input),
                ("output_terms", Direction::Output),
            ],
        ),
        SvSystemFunction::new(
            "$sync$nand$array",
            &[
                ("memory_identifier", Direction::Input),
                ("input_terms", Direction::Input),
                ("output_terms", Direction::Output),
            ],
        ),
        SvSystemFunction::new(
            "$async$nand$plane",
            &[
                ("memory_identifier", Direction::Input),
                ("input_terms", Direction::Input),
                ("output_terms", Direction::Output),
            ],
        ),
        SvSystemFunction::new(
            "$sync$nand$plane",
            &[
                ("memory_identifier", Direction::Input),
                ("input_terms", Direction::Input),
                ("output_terms", Direction::Output),
            ],
        ),
        SvSystemFunction::new(
            "$async$or$array",
            &[
                ("memory_identifier", Direction::Input),
                ("input_terms", Direction::Input),
                ("output_terms", Direction::Output),
            ],
        ),
        SvSystemFunction::new(
            "$sync$or$array",
            &[
                ("memory_identifier", Direction::Input),
                ("input_terms", Direction::Input),
                ("output_terms", Direction::Output),
            ],
        ),
        SvSystemFunction::new(
            "$async$or$plane",
            &[
                ("memory_identifier", Direction::Input),
                ("input_terms", Direction::Input),
                ("output_terms", Direction::Output),
            ],
        ),
        SvSystemFunction::new(
            "$sync$or$plane",
            &[
                ("memory_identifier", Direction::Input),
                ("input_terms", Direction::Input),
                ("output_terms", Direction::Output),
            ],
        ),
        SvSystemFunction::new(
            "$async$nor$array",
            &[
                ("memory_identifier", Direction::Input),
                ("input_terms", Direction::Input),
                ("output_terms", Direction::Output),
            ],
        ),
        SvSystemFunction::new(
            "$sync$nor$array",
            &[
                ("memory_identifier", Direction::Input),
                ("input_terms", Direction::Input),
                ("output_terms", Direction::Output),
            ],
        ),
        SvSystemFunction::new(
            "$async$nor$plane",
            &[
                ("memory_identifier", Direction::Input),
                ("input_terms", Direction::Input),
                ("output_terms", Direction::Output),
            ],
        ),
        SvSystemFunction::new(
            "$sync$nor$plane",
            &[
                ("memory_identifier", Direction::Input),
                ("input_terms", Direction::Input),
                ("output_terms", Direction::Output),
            ],
        ),
        // Miscellaneous tasks and functions
        SvSystemFunction::new("$system", &[("terminal_command_line", Direction::Input)]),
        SvSystemFunction::new("$stacktrace;", &[]),
        // Input/output system tasks and system functions
        SvSystemFunction::new(
            "$display",
            // it has optional args but not supported
            &[],
        ),
        SvSystemFunction::new(
            "$displayb",
            // it has optional args but not supported
            &[],
        ),
        SvSystemFunction::new(
            "$displayo",
            // it has optional args but not supported
            &[],
        ),
        SvSystemFunction::new(
            "$displayh",
            // it has optional args but not supported
            &[],
        ),
        SvSystemFunction::new(
            "$write",
            // it has optional args but not supported
            &[],
        ),
        SvSystemFunction::new(
            "$writeb",
            // it has optional args but not supported
            &[],
        ),
        SvSystemFunction::new(
            "$writeo",
            // it has optional args but not supported
            &[],
        ),
        SvSystemFunction::new(
            "$writeh",
            // it has optional args but not supported
            &[],
        ),
        SvSystemFunction::new(
            "$strobe",
            // it has optional args but not supported
            &[],
        ),
        SvSystemFunction::new(
            "$strobeb",
            // it has optional args but not supported
            &[],
        ),
        SvSystemFunction::new(
            "$strobeo",
            // it has optional args but not supported
            &[],
        ),
        SvSystemFunction::new(
            "$strobeh",
            // it has optional args but not supported
            &[],
        ),
        SvSystemFunction::new(
            "$monitor",
            // it has optional args but not supported
            &[],
        ),
        SvSystemFunction::new(
            "$monitorb",
            // it has optional args but not supported
            &[],
        ),
        SvSystemFunction::new(
            "$monitoro",
            // it has optional args but not supported
            &[],
        ),
        SvSystemFunction::new(
            "$monitorh",
            // it has optional args but not supported
            &[],
        ),
        SvSystemFunction::new("$monitoron", &[]),
        SvSystemFunction::new("$monitoroff", &[]),
        SvSystemFunction::new(
            "$fopen",
            // it has optional args but not supported
            &[("filename", Direction::Input)],
        ),
        SvSystemFunction::new("$fclose", &[("fd", Direction::Input)]),
        SvSystemFunction::new(
            "$fdisplay",
            // it has optional args but not supported
            &[("fd", Direction::Input)],
        ),
        SvSystemFunction::new(
            "$fdisplayb",
            // it has optional args but not supported
            &[("fd", Direction::Input)],
        ),
        SvSystemFunction::new(
            "$fdisplayo",
            // it has optional args but not supported
            &[("fd", Direction::Input)],
        ),
        SvSystemFunction::new(
            "$fdisplayh",
            // it has optional args but not supported
            &[("fd", Direction::Input)],
        ),
        SvSystemFunction::new(
            "$fwrite",
            // it has optional args but not supported
            &[("fd", Direction::Input)],
        ),
        SvSystemFunction::new(
            "$fwriteb",
            // it has optional args but not supported
            &[("fd", Direction::Input)],
        ),
        SvSystemFunction::new(
            "$fwriteo",
            // it has optional args but not supported
            &[("fd", Direction::Input)],
        ),
        SvSystemFunction::new(
            "$fwriteh",
            // it has optional args but not supported
            &[("fd", Direction::Input)],
        ),
        SvSystemFunction::new(
            "$fstrobe",
            // it has optional args but not supported
            &[("fd", Direction::Input)],
        ),
        SvSystemFunction::new(
            "$fstrobeb",
            // it has optional args but not supported
            &[("fd", Direction::Input)],
        ),
        SvSystemFunction::new(
            "$fstrobeo",
            // it has optional args but not supported
            &[("fd", Direction::Input)],
        ),
        SvSystemFunction::new(
            "$fstrobeh",
            // it has optional args but not supported
            &[("fd", Direction::Input)],
        ),
        SvSystemFunction::new(
            "$fmonitor",
            // it has optional args but not supported
            &[("fd", Direction::Input)],
        ),
        SvSystemFunction::new(
            "$fmonitorb",
            // it has optional args but not supported
            &[("fd", Direction::Input)],
        ),
        SvSystemFunction::new(
            "$fmonitoro",
            // it has optional args but not supported
            &[("fd", Direction::Input)],
        ),
        SvSystemFunction::new(
            "$fmonitorh",
            // it has optional args but not supported
            &[("fd", Direction::Input)],
        ),
        SvSystemFunction::new(
            "$swrite",
            // it has optional args but not supported
            &[("output_var", Direction::Output)],
        ),
        SvSystemFunction::new(
            "$swriteb",
            // it has optional args but not supported
            &[("output_var", Direction::Output)],
        ),
        SvSystemFunction::new(
            "$swriteo",
            // it has optional args but not supported
            &[("output_var", Direction::Output)],
        ),
        SvSystemFunction::new(
            "$swriteh",
            // it has optional args but not supported
            &[("output_var", Direction::Output)],
        ),
        SvSystemFunction::new(
            "$sformat",
            // it has optional args but not supported
            &[
                ("output_var", Direction::Output),
                ("format_string", Direction::Input),
            ],
        ),
        SvSystemFunction::new(
            "$sformatf",
            // it has optional args but not supported
            &[("format_string", Direction::Input)],
        ),
        SvSystemFunction::new("$fgetc", &[("fd", Direction::Input)]),
        SvSystemFunction::new(
            "$ungetc",
            &[("c", Direction::Output), ("fd", Direction::Input)],
        ),
        SvSystemFunction::new(
            "$fgets",
            &[("str", Direction::Output), ("fd", Direction::Input)],
        ),
        SvSystemFunction::new(
            "$fscanf",
            // it has optional args but not supported
            &[("fd", Direction::Input), ("format", Direction::Input)],
        ),
        SvSystemFunction::new(
            "$sscanf",
            // it has optional args but not supported
            &[("str", Direction::Input), ("format", Direction::Input)],
        ),
        SvSystemFunction::new(
            "$fread",
            // it has optional args but not supported
            &[
                ("integral_va_or_memory", Direction::Input),
                ("fd", Direction::Input),
            ],
        ),
        SvSystemFunction::new("$ftell", &[("fd", Direction::Input)]),
        SvSystemFunction::new(
            "$fseek",
            &[
                ("fd", Direction::Input),
                ("offset", Direction::Input),
                ("operation", Direction::Input),
            ],
        ),
        SvSystemFunction::new("$rewind", &[("fd", Direction::Input)]),
        SvSystemFunction::new(
            "$fflush",
            // it has optional args but not supported
            &[],
        ),
        SvSystemFunction::new(
            "$ferror",
            // it has optional args but not supported
            &[("fd", Direction::Input), ("str", Direction::Output)],
        ),
        SvSystemFunction::new("$feof", &[("fd", Direction::Input)]),
        // Loading memory array data from a file
        SvSystemFunction::new(
            "$readmemb",
            // it has optional args but not supported
            &[
                ("filename", Direction::Input),
                ("memory_name", Direction::Output),
            ],
        ),
        SvSystemFunction::new(
            "$readmemh",
            // it has optional args but not supported
            &[
                ("filename", Direction::Input),
                ("memory_name", Direction::Output),
            ],
        ),
        // Writing memory array data to a file
        SvSystemFunction::new(
            "$writememb",
            // it has optional args but not supported
            &[
                ("filename", Direction::Input),
                ("memory_name", Direction::Input),
            ],
        ),
        SvSystemFunction::new(
            "$writememh",
            // it has optional args but not supported
            &[
                ("filename", Direction::Input),
                ("memory_name", Direction::Input),
            ],
        ),
        // Command line input
        SvSystemFunction::new("$test$plusargs", &[("string", Direction::Input)]),
        SvSystemFunction::new(
            "$value$plusargs",
            &[
                ("user_string", Direction::Input),
                ("variable", Direction::Output),
            ],
        ),
        // Value change dump (VCD) files
        SvSystemFunction::new("$dumpfile", &[("filename", Direction::Input)]),
        SvSystemFunction::new(
            "$dumpvars",
            // it has optional args but not supported
            &[],
        ),
        SvSystemFunction::new("$dumpoff", &[]),
        SvSystemFunction::new("$dumpon", &[]),
        SvSystemFunction::new("$dumpall", &[]),
        SvSystemFunction::new("$dumplimit", &[("filesize", Direction::Input)]),
        SvSystemFunction::new("$dumpflush", &[]),
        SvSystemFunction::new(
            "$dumpports",
            // it has optional args but not supported
            &[],
        ),
        SvSystemFunction::new("$dumpportsoff", &[("filename", Direction::Input)]),
        SvSystemFunction::new("$dumpportson", &[("filename", Direction::Input)]),
        SvSystemFunction::new("$dumpportsall", &[("filename", Direction::Input)]),
        SvSystemFunction::new(
            "$dumpportslimit",
            &[
                ("filelimit", Direction::Input),
                ("filename", Direction::Input),
            ],
        ),
        SvSystemFunction::new("$dumpportsflush", &[("filename", Direction::Input)]),
    ]
}
