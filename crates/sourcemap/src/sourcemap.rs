use crate::SourceMapError;
use relative_path::PathExt;
use sourcemap::SourceMapBuilder;
use std::fs;
use std::path::{Path, PathBuf};

const LINK_HEADER: &str = "//# sourceMappingURL=";

pub struct SourceMap {
    pub src_path: PathBuf,
    pub dst_path: PathBuf,
    pub map_path: PathBuf,
    pub src_path_from_map: String,
    pub map_path_from_dst: String,
    builder: SourceMapBuilder,
    source_map: Option<sourcemap::SourceMap>,
}

impl SourceMap {
    pub fn new(src_path: &Path, dst_path: &Path, map_path: &Path) -> Self {
        let src_path = src_path.to_path_buf();
        let dst_path = dst_path.to_path_buf();
        let map_path = map_path.to_path_buf();
        let src_path_from_map = if let Ok(x) = src_path.relative_to(map_path.parent().unwrap()) {
            x.as_str().to_owned()
        } else {
            src_path.to_string_lossy().to_string()
        };
        let map_path_from_dst = if let Ok(x) = map_path.relative_to(dst_path.parent().unwrap()) {
            x.as_str().to_owned()
        } else {
            map_path.to_string_lossy().to_string()
        };
        let builder = SourceMapBuilder::new(Some(&map_path.file_name().unwrap().to_string_lossy()));
        Self {
            src_path,
            dst_path,
            map_path,
            src_path_from_map,
            map_path_from_dst,
            builder,
            source_map: None,
        }
    }

    pub fn from_src(src_path: &Path) -> Result<Self, SourceMapError> {
        let src = fs::read_to_string(src_path)?;

        if let Some(line) = src.lines().last() {
            if line.starts_with(LINK_HEADER) {
                let map_path = line.strip_prefix(LINK_HEADER).unwrap();
                let map_path = src_path.parent().unwrap().join(map_path);
                let text = fs::read(&map_path)?;

                let src_path = src_path.to_path_buf();
                let dst_path = PathBuf::new();
                let src_path_from_map = String::new();
                let map_path_from_dst = String::new();
                let builder =
                    SourceMapBuilder::new(Some(&map_path.file_name().unwrap().to_string_lossy()));
                let source_map = Some(sourcemap::SourceMap::from_reader(text.as_slice())?);

                return Ok(Self {
                    src_path,
                    dst_path,
                    map_path,
                    src_path_from_map,
                    map_path_from_dst,
                    builder,
                    source_map,
                });
            }
        }

        Err(SourceMapError::NotFound)
    }

    pub fn add(
        &mut self,
        dst_line: u32,
        dst_column: u32,
        src_line: u32,
        src_column: u32,
        name: &str,
    ) {
        // Line and column of sourcemap crate is 0-based
        let dst_line = dst_line - 1;
        let dst_column = dst_column - 1;
        let src_line = src_line - 1;
        let src_column = src_column - 1;

        self.builder.add(
            dst_line,
            dst_column,
            src_line,
            src_column,
            Some(&self.src_path_from_map),
            Some(name),
            false,
        );
    }

    pub fn set_source_content(&mut self, content: &str) {
        let id = self.builder.add_source(&self.src_path_from_map);
        self.builder.set_source_contents(id, Some(content));
    }

    pub fn build(&mut self) {
        let mut builder =
            SourceMapBuilder::new(Some(&self.map_path.file_name().unwrap().to_string_lossy()));
        std::mem::swap(&mut builder, &mut self.builder);
        self.source_map = Some(builder.into_sourcemap());
    }

    pub fn get_link(&self) -> String {
        format!("{}{}", LINK_HEADER, self.map_path_from_dst)
    }

    pub fn to_bytes(&self) -> Result<Vec<u8>, SourceMapError> {
        if let Some(ref x) = self.source_map {
            let mut ret = Vec::new();
            x.to_writer(&mut ret)?;
            Ok(ret)
        } else {
            Err(SourceMapError::NotFound)
        }
    }

    pub fn lookup(&self, line: u32, column: u32) -> Option<(PathBuf, u32, u32)> {
        if let Some(ref x) = self.source_map {
            if let Some(token) = x.lookup_token(line - 1, column - 1) {
                if let Some(path) = token.get_source() {
                    let path = self.map_path.parent().unwrap().join(path);
                    if let Ok(path) = fs::canonicalize(path) {
                        let line = token.get_src_line() + 1;
                        let column = token.get_src_col() + 1;
                        Some((path, line, column))
                    } else {
                        None
                    }
                } else {
                    None
                }
            } else {
                None
            }
        } else {
            None
        }
    }
}
