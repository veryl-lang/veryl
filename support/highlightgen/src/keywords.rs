use serde::Serialize;
use std::fs;
use std::path::Path;

#[derive(Clone, Debug, Serialize)]
pub struct Keywords {
    pub conditional: Vec<String>,
    pub direction: Vec<String>,
    pub literal: Vec<String>,
    pub repeat: Vec<String>,
    pub statement: Vec<String>,
    pub structure: Vec<String>,
    pub r#type: Vec<String>,
}

impl Keywords {
    pub fn load(root: &Path) -> Self {
        let path = root.join("./crates/parser/veryl.par");
        let text = fs::read_to_string(path).unwrap();

        let mut conditional = vec![];
        let mut direction = vec![];
        let mut literal = vec![];
        let mut repeat = vec![];
        let mut statement = vec![];
        let mut structure = vec![];
        let mut r#type = vec![];

        for line in text.lines() {
            if line.contains("// Keyword: ") {
                let keyword = line.split('/').nth(1).unwrap();
                let keyword = keyword.replace("(?-u:\\b)", "");
                let keyword = keyword.replace("(?-u:\\b)", "");
                let category = line.split("// Keyword: ").nth(1).unwrap();

                match category {
                    "Conditional" => conditional.push(keyword),
                    "Direction" => direction.push(keyword),
                    "Literal" => literal.push(keyword),
                    "Repeat" => repeat.push(keyword),
                    "Statement" => statement.push(keyword),
                    "Structure" => structure.push(keyword),
                    "Type" => r#type.push(keyword),
                    _ => unreachable!(),
                }
            }
        }

        Self {
            conditional,
            direction,
            literal,
            repeat,
            statement,
            structure,
            r#type,
        }
    }
}
