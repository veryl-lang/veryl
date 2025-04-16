use std::env;

#[derive(Clone, Debug)]
pub struct EnvVar {
    pub analyzer_pass1_enables: [bool; 8],
    pub analyzer_pass2_enables: [bool; 13],
    pub analyzer_pass3_enables: [bool; 3],
}

impl Default for EnvVar {
    fn default() -> Self {
        let analyzer_pass1_enables = if let Ok(x) = env::var("ANALYZER_PASS1_ENABLES") {
            parse_bit_flag(&x).unwrap_or([true; 8])
        } else {
            [true; 8]
        };
        let analyzer_pass2_enables = if let Ok(x) = env::var("ANALYZER_PASS2_ENABLES") {
            parse_bit_flag(&x).unwrap_or([true; 13])
        } else {
            [true; 13]
        };
        let analyzer_pass3_enables = if let Ok(x) = env::var("ANALYZER_PASS3_ENABLES") {
            parse_bit_flag(&x).unwrap_or([true; 3])
        } else {
            [true; 3]
        };
        Self {
            analyzer_pass1_enables,
            analyzer_pass2_enables,
            analyzer_pass3_enables,
        }
    }
}

fn parse_bit_flag<const N: usize>(s: &str) -> Option<[bool; N]> {
    if let Ok(x) = usize::from_str_radix(s, 16) {
        let mut ret = [false; N];

        for (i, item) in ret.iter_mut().enumerate() {
            *item = (x >> i) & 1 == 1;
        }
        Some(ret)
    } else {
        None
    }
}
