use crate::Formatter;
use veryl_analyzer::Analyzer;
use veryl_metadata::Metadata;
use veryl_parser::Parser;

#[track_caller]
fn format(metadata: &Metadata, code: &str) -> String {
    let parser = Parser::parse(&code, &"").unwrap();
    let analyzer = Analyzer::new(metadata);

    analyzer.analyze_pass1(&"prj", &"", &parser.veryl);
    Analyzer::analyze_post_pass1();
    analyzer.analyze_pass2(&"prj", &"", &parser.veryl);

    let mut formatter = Formatter::new(metadata);
    formatter.format(&parser.veryl);
    let result = formatter.as_str().to_string();

    if cfg!(windows) {
        result.replace("\r\n", "\n")
    } else {
        result
    }
}

#[test]
fn empty_body_with_comment() {
    let code = r#"module ModuleA {
    /* */
}
"#;
    let expect = r#"module ModuleA {
    /* */
}
"#;

    let metadata = Metadata::create_default("prj").unwrap();

    let ret = format(&metadata, &code);
    assert_eq!(ret, expect);

    let code = r#"module ModuleA {
    /* foo */
    /* bar */
}
"#;
    let expect = r#"module ModuleA {
    /* foo */
    /* bar */
}
"#;

    let metadata = Metadata::create_default("prj").unwrap();

    let ret = format(&metadata, &code);
    assert_eq!(ret, expect);

    let code = r#"module ModuleA {
    /* foo */
    // bar
}
"#;
    let expect = r#"module ModuleA {
    /* foo */
    // bar
}
"#;

    let metadata = Metadata::create_default("prj").unwrap();

    let ret = format(&metadata, &code);
    assert_eq!(ret, expect);
}

#[test]
fn empty_list() {
    let code = r#"module ModuleA #(

) (

) {

}
module ModuleB {
  inst u: ModuleA #(

    ) (

    );

    function Func (

    ) {

    }

    always_comb {
        Func(

        );
    }
}
"#;

    let expect = r#"module ModuleA #() () {}
module ModuleB {
    inst u: ModuleA ;

    function Func () {}

    always_comb {
        Func();
    }
}
"#;

    let metadata = Metadata::create_default("prj").unwrap();

    let ret = format(&metadata, &code);

    println!("ret\n{}\nexp\n{}", ret, expect);
    assert_eq!(ret, expect);
}

#[test]
fn skip_formatting() {
    let code = r#"#[fmt(skip)]
module ModuleA {
    let _a: logic = 0;
}

#[fmt(skip)]
interface InterfaceA {
    var a: logic;

    modport mp {
        a: input
    }
}

#[fmt(skip)]
package PackageA {
    const A: u32 = 0;

    function FuncA(
        a: input u32,
        b: input u32
    ) -> u32 {
        return a + b;
    }
}
"#;

    let mut metadata = Metadata::create_default("prj").unwrap();

    metadata.format.indent_width = 2;

    let ret = format(&metadata, &code);

    println!("ret\n{}\nexp\n{}", ret, code);
    assert_eq!(ret, code);

    let code = r#"#[fmt(skip)]
module ModuleA () {
    /* this comment line is important */
}
#[fmt(skip)]
module ModuleB () {
    // this comment line is important
}
"#;

    let mut metadata = Metadata::create_default("prj").unwrap();

    metadata.format.indent_width = 2;

    let ret = format(&metadata, &code);

    println!("ret\n{}\nexp\n{}", ret, code);
    assert_eq!(ret, code);
}
