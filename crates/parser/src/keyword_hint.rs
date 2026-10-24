/// Hint for a word that lexes as a plain identifier but was written where a
/// keyword is required.
pub(crate) fn keyword_hint(text: &str) -> Option<&'static str> {
    let hint = match text {
        "local" | "localparam" => "'local' and 'localparam' were replaced by 'const'",
        "parameter" => "'parameter' was renamed to 'param'",
        "export" => "'export' declaration was removed",
        "ref" => "'ref' direction was removed, use 'output' or 'inout'",
        "bool" => "'bool' was split into 'bbool' and 'lbool'",
        "async_high" => "'async_high' was renamed to 'reset_async_high'",
        "async_low" => "'async_low' was renamed to 'reset_async_low'",
        "sync_high" => "'sync_high' was renamed to 'reset_sync_high'",
        "sync_low" => "'sync_low' was renamed to 'reset_sync_low'",
        "posedge" => "clock edge is specified by the 'clock_posedge' type, not by 'posedge'",
        "negedge" => "clock edge is specified by the 'clock_negedge' type, not by 'negedge'",

        "always" => "Veryl has no 'always', use 'always_comb' or 'always_ff'",
        "begin" | "end" => "Veryl delimits a block by '{' and '}', not by 'begin' and 'end'",
        "endmodule" | "endinterface" | "endpackage" | "endfunction" | "endtask" | "endcase" => {
            "Veryl closes a block by '}'"
        }
        "generate" | "endgenerate" => {
            "Veryl has no generate block, write 'for' or 'if' declaration directly"
        }
        "genvar" => "Veryl has no 'genvar', 'for' declares its loop variable implicitly",
        "wire" => "Veryl has no 'wire', use 'let' (with an initial value) or 'var'",
        "reg" => "Veryl has no 'reg', use 'var'",
        "typedef" => "'typedef' is spelled 'type' in Veryl (e.g. 'type Word = logic<32>;')",
        "task" => "Veryl has no 'task', use 'function'",
        "casex" | "casez" => "Veryl has no 'casex' and 'casez', use 'case'",
        "int" => "'int' is spelled 'i32' in Veryl",
        "integer" => "'integer' is spelled 'i32' in Veryl",
        "byte" => "'byte' is spelled 'i8' in Veryl",
        "shortint" => "'shortint' is spelled 'i16' in Veryl",
        "longint" => "'longint' is spelled 'i64' in Veryl",
        "real" => "'real' is spelled 'f64' in Veryl",
        "shortreal" => "'shortreal' is spelled 'f32' in Veryl",
        "unsigned" => "Veryl types are unsigned unless 'signed' is specified",
        "packed" => "Veryl aggregate types are always packed",

        _ => return None,
    };
    Some(hint)
}
