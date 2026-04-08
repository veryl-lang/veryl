/// String buffer with indentation tracking for emitting Veryl source.
pub struct Writer {
    buf: String,
    indent: usize,
    indent_width: usize,
    at_line_start: bool,
    newline: &'static str,
}

impl Writer {
    pub fn new(newline: &'static str) -> Self {
        Self {
            buf: String::new(),
            indent: 0,
            indent_width: 4,
            at_line_start: true,
            newline,
        }
    }

    pub fn as_str(&self) -> &str {
        &self.buf
    }

    pub fn into_string(self) -> String {
        self.buf
    }

    fn ensure_indent(&mut self) {
        if self.at_line_start {
            for _ in 0..(self.indent * self.indent_width) {
                self.buf.push(' ');
            }
            self.at_line_start = false;
        }
    }

    pub fn str(&mut self, s: &str) {
        if s.is_empty() {
            return;
        }
        self.ensure_indent();
        self.buf.push_str(s);
    }

    pub fn space(&mut self) {
        self.ensure_indent();
        self.buf.push(' ');
    }

    pub fn newline(&mut self) {
        self.buf.push_str(self.newline);
        self.at_line_start = true;
    }

    pub fn indent(&mut self) {
        self.indent += 1;
    }

    pub fn dedent(&mut self) {
        if self.indent > 0 {
            self.indent -= 1;
        }
    }

    /// Emit an unsupported-construct comment block. Original SV source for the
    /// node is included as `// > ` quoted lines so the user can hand-edit it.
    pub fn unsupported(&mut self, kind: &str, line: usize, src: &str) {
        self.str(&format!(
            "// TODO(translate): unsupported {kind} at line {line}"
        ));
        self.newline();
        for l in src.lines() {
            self.str("// > ");
            self.str(l);
            self.newline();
        }
    }
}
