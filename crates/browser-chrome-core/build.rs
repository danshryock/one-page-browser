//! Zips `assets/` (this crate's own directory of embedded pages — see that
//! directory's `index.html` for what it currently holds and why) into
//! `$OUT_DIR/embedded_assets.zip` at build time, using `zip`'s own writer
//! API directly rather than shelling out to a `zip`/`unzip` CLI tool —
//! portable, and avoids the exact class of "is this tool installed on the
//! build machine" uncertainty `browser-windows-reactor/build.rs`'s own
//! `unzip` shell-out already has. `src/lib.rs` embeds the resulting archive
//! via `include_bytes!` and reads pages/assets back out of it at runtime
//! through `zip_assets::ZipAssets` — never extracted to disk at any point,
//! build time or run time.
use std::io::Write;

fn main() {
    let assets_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets");
    println!("cargo::rerun-if-changed={}", assets_dir.display());

    let out_path = std::path::Path::new(&std::env::var("OUT_DIR").expect("OUT_DIR is always set for a build script")).join("embedded_assets.zip");
    let out_file = std::fs::File::create(&out_path).unwrap_or_else(|err| panic!("creating {out_path:?} failed: {err}"));
    let mut writer = zip::ZipWriter::new(out_file);
    let options = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);

    for entry in walkdir::WalkDir::new(&assets_dir) {
        let entry = entry.unwrap_or_else(|err| panic!("walking {assets_dir:?} failed: {err}"));
        if !entry.file_type().is_file() {
            continue;
        }
        // Zip entry names always use `/`, regardless of the build
        // machine's own path separator (relevant on Windows) — `assets/`
        // itself is stripped so entry names match the paths
        // `EmbeddedAssetServer` serves them under (e.g. `index.html`, not
        // `assets/index.html`).
        let relative = entry.path().strip_prefix(&assets_dir).expect("walkdir always yields paths under the root it was given");
        let entry_name = relative.components().map(|c| c.as_os_str().to_string_lossy()).collect::<Vec<_>>().join("/");

        writer.start_file(&entry_name, options).unwrap_or_else(|err| panic!("starting zip entry {entry_name:?} failed: {err}"));
        let contents = std::fs::read(entry.path()).unwrap_or_else(|err| panic!("reading {:?} failed: {err}", entry.path()));
        writer.write_all(&contents).unwrap_or_else(|err| panic!("writing zip entry {entry_name:?} failed: {err}"));
    }

    writer.finish().expect("finishing embedded_assets.zip should succeed");
}
