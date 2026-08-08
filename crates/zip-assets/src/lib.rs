//! `ZipAssets` — reads individual entries out of a zip archive fully into
//! memory, with no disk extraction step at all: decompression happens
//! straight from the archive's own bytes (wherever they live — a
//! compile-time `include_bytes!` slice, or a file read into a `Vec<u8>` at
//! runtime) into the returned `Vec<u8>`. Exists as its own crate (rather
//! than living inside `browser-chrome-core`, alongside the thing that first
//! needed it) specifically so `browser-extension-host`'s Deno module loader
//! can depend on this one small piece without pulling in
//! `browser-chrome-core`'s much larger, UI-adjacent dependency footprint.

use std::io::Cursor;
use std::sync::Mutex;

/// A zip archive's bytes, ready to have individual entries read out of it.
/// `Mutex`-wrapped because `zip::ZipArchive`'s entry-reading methods need
/// `&mut self` (they seek internally) — the same "wrap for a shared
/// `&self` API" reasoning `rpc_protocol::RpcChildProcess` already uses for
/// its own `Mutex<BufWriter<ChildStdin>>`.
pub struct ZipAssets(Mutex<zip::ZipArchive<Cursor<std::borrow::Cow<'static, [u8]>>>>);

impl ZipAssets {
    /// `bytes` is `Cow<'static, [u8]>` (not just `Vec<u8>` or `&'static
    /// [u8]`) so the exact same type serves both an embedded, compile-time
    /// zip (`Cow::Borrowed`, backing an `include_bytes!` slice) and one
    /// read from disk at runtime (`Cow::Owned`, an extension's own zip) —
    /// one type for both, not two.
    pub fn from_bytes(bytes: std::borrow::Cow<'static, [u8]>) -> anyhow::Result<Self> {
        let archive = zip::ZipArchive::new(Cursor::new(bytes))?;
        Ok(Self(Mutex::new(archive)))
    }

    /// Reads `path`'s entry fully into memory, decompressed. Errors (rather
    /// than panicking) if `path` isn't in the archive, or the archive is
    /// otherwise malformed.
    pub fn read(&self, path: &str) -> anyhow::Result<Vec<u8>> {
        use std::io::Read;
        let mut archive = self.0.lock().unwrap();
        let mut entry = archive.by_name(path).map_err(|err| anyhow::anyhow!("no {path:?} entry in this archive: {err}"))?;
        let mut buf = Vec::with_capacity(entry.size() as usize);
        entry.read_to_end(&mut buf)?;
        Ok(buf)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// Builds a small zip in memory via `zip`'s own writer API — no
    /// fixture file needed, and it means these tests exercise the exact
    /// same DEFLATE path a real archive produced by `browser-chrome-core`'s
    /// `build.rs` (or a real extension package) would use.
    fn build_test_zip(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut buf = Vec::new();
        {
            let mut writer = zip::ZipWriter::new(Cursor::new(&mut buf));
            let options = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
            for (name, contents) in entries {
                writer.start_file(*name, options).expect("starting a zip entry should succeed");
                writer.write_all(contents).expect("writing zip entry contents should succeed");
            }
            writer.finish().expect("finishing the zip should succeed");
        }
        buf
    }

    #[test]
    fn reads_an_entry_back_out_unchanged() {
        let zip_bytes = build_test_zip(&[("hello.txt", b"hello, world"), ("nested/dir/file.txt", b"nested contents")]);
        let assets = ZipAssets::from_bytes(zip_bytes.into()).expect("a well-formed zip should open");
        assert_eq!(assets.read("hello.txt").unwrap(), b"hello, world");
        assert_eq!(assets.read("nested/dir/file.txt").unwrap(), b"nested contents");
    }

    #[test]
    fn reading_a_missing_entry_errors_instead_of_panicking() {
        let zip_bytes = build_test_zip(&[("only.txt", b"contents")]);
        let assets = ZipAssets::from_bytes(zip_bytes.into()).expect("a well-formed zip should open");
        let err = assets.read("does_not_exist.txt").unwrap_err();
        assert!(err.to_string().contains("does_not_exist.txt"));
    }

    #[test]
    fn opening_malformed_bytes_errors_instead_of_panicking() {
        // `.err()` (not `.unwrap_err()`): `ZipAssets` isn't `Debug` (it
        // wraps a `Mutex`-guarded `zip::ZipArchive`, which isn't either),
        // and `unwrap_err` needs the `Ok` type to be `Debug` even though it
        // never actually prints it on this path.
        let result = ZipAssets::from_bytes((&b"not a zip file at all"[..]).into());
        assert!(result.is_err());
        assert!(!result.err().unwrap().to_string().is_empty());
    }

    #[test]
    fn works_with_a_borrowed_static_slice_too() {
        static ZIP_BYTES: &[u8] = &[]; // placeholder — real usage is `include_bytes!`
        // Just proving `Cow::Borrowed` type-checks and constructs the same
        // way `Cow::Owned` does above; an empty archive is expected to
        // fail to parse, which is fine — this test is about the type, not
        // the content.
        let result = ZipAssets::from_bytes(std::borrow::Cow::Borrowed(ZIP_BYTES));
        assert!(result.is_err());
    }
}
