// Fixed readable-text worker logic shared only with its standalone binary.

use std::{
    fs,
    io::{self, Read, Write},
    path::Path,
};

pub(crate) const MAX_TEXT_INPUT_BYTES: u64 = 1024 * 1024;
pub(crate) const MAX_TEXT_OUTPUT_BYTES: usize = 1024 * 1024;

pub(crate) fn run_fixed_text_worker(input: &Path, work: &Path) -> io::Result<usize> {
    let input_meta = fs::symlink_metadata(input)?;
    if input_meta.file_type().is_symlink()
        || !input_meta.is_file()
        || input_meta.len() > MAX_TEXT_INPUT_BYTES
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "unsupported input",
        ));
    }
    let input_parent = input
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "missing input parent"))?;
    let input_names = fs::read_dir(input_parent)?
        .map(|entry| entry.map(|entry| entry.file_name()))
        .collect::<io::Result<Vec<_>>>()?;
    if input_names.len() != 1 || input_names[0] != input.file_name().unwrap_or_default() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "unexpected input files",
        ));
    }
    let work_meta = fs::symlink_metadata(work)?;
    if work_meta.file_type().is_symlink()
        || !work_meta.is_dir()
        || fs::read_dir(work)?.next().is_some()
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "work directory is not empty",
        ));
    }
    let mut source = fs::File::open(input)?;
    let mut bytes = Vec::with_capacity(input_meta.len() as usize);
    Read::by_ref(&mut source)
        .take(MAX_TEXT_INPUT_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_TEXT_INPUT_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "input too large",
        ));
    }
    let text = std::str::from_utf8(&bytes)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "input is not UTF-8"))?;
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
    if normalized.len() > MAX_TEXT_OUTPUT_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "output too large",
        ));
    }
    let output = work.join("output");
    let mut destination = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&output)?;
    destination.write_all(normalized.as_bytes())?;
    destination.sync_all()?;
    Ok(normalized.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(bytes: &[u8]) -> (std::path::PathBuf, std::path::PathBuf, std::path::PathBuf) {
        let root =
            std::env::temp_dir().join(format!("pastey-worker-test-{}", uuid::Uuid::new_v4()));
        let input_root = root.join("input");
        let work = root.join("work");
        fs::create_dir_all(&input_root).unwrap();
        fs::create_dir(&work).unwrap();
        let input = input_root.join("artifact");
        fs::write(&input, bytes).unwrap();
        (root, input, work)
    }

    #[test]
    fn accepts_bounded_text_markdown_json_and_csv_bytes() {
        for bytes in [
            b"plain\r\ntext".as_slice(),
            b"# markdown\n".as_slice(),
            br#"{"ok":true}"#.as_slice(),
            b"a,b\n1,2\n".as_slice(),
        ] {
            let (root, input, work) = fixture(bytes);
            run_fixed_text_worker(&input, &work).unwrap();
            assert!(
                fs::read_to_string(work.join("output"))
                    .unwrap()
                    .contains('\n')
                    || !bytes.contains(&b'\n')
            );
            fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn rejects_invalid_utf8_oversize_missing_and_extra_files() {
        let (root, input, work) = fixture(&[0xff, 0xfe]);
        assert!(run_fixed_text_worker(&input, &work).is_err());
        fs::remove_dir_all(root).unwrap();
        let (root, input, work) = fixture(b"ok");
        fs::write(input.parent().unwrap().join("extra"), b"x").unwrap();
        assert!(run_fixed_text_worker(&input, &work).is_err());
        fs::remove_dir_all(root).unwrap();
        let (root, input, work) = fixture(b"ok");
        fs::remove_file(&input).unwrap();
        assert!(run_fixed_text_worker(&input, &work).is_err());
        fs::remove_dir_all(root).unwrap();
        let (root, input, work) = fixture(&vec![b'x'; MAX_TEXT_INPUT_BYTES as usize + 1]);
        assert!(run_fixed_text_worker(&input, &work).is_err());
        fs::remove_dir_all(root).unwrap();
    }
}
