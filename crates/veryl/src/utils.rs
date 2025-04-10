use miette::{IntoDiagnostic, Result};
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::Path;

pub fn write_file_if_changed<T: AsRef<Path>>(path: T, data: &[u8]) -> Result<bool> {
    if let Ok(mut file) = File::open(path.as_ref()) {
        let mut content = Vec::new();
        if file.read_to_end(&mut content).is_ok() && content == data {
            return Ok(false);
        }
    }

    let mut file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(path.as_ref())
        .into_diagnostic()?;
    file.write_all(data).into_diagnostic()?;
    file.flush().into_diagnostic()?;
    Ok(true)
}
