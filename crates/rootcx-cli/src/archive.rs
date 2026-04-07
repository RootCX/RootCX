use anyhow::Result;
use flate2::Compression;
use flate2::write::GzEncoder;
use std::path::Path;
use walkdir::WalkDir;

const EXCLUDE: &[&str] = &["node_modules", ".git", ".rootcx", "bun.lock", "src-tauri"];

pub fn pack_dir(root: &Path, rel: &Path) -> Result<Vec<u8>> {
    let full = root.join(rel);
    let encoder = GzEncoder::new(Vec::new(), Compression::default());
    let mut tar = tar::Builder::new(encoder);

    for entry in WalkDir::new(&full).into_iter().filter_entry(|e| {
        !EXCLUDE.iter().any(|ex| e.file_name().to_string_lossy() == *ex)
    }) {
        let entry = entry?;
        let path = entry.path();
        if path == full {
            continue;
        }
        let rel_path = path.strip_prefix(&full)?;
        if entry.file_type().is_dir() {
            tar.append_dir(rel_path, path)?;
        } else if entry.file_type().is_file() {
            let mut f = std::fs::File::open(path)?;
            tar.append_file(rel_path, &mut f)?;
        }
    }

    let encoder = tar.into_inner()?;
    Ok(encoder.finish()?)
}

#[cfg(test)]
mod tests {
    use super::pack_dir;
    use flate2::read::GzDecoder;
    use std::collections::BTreeSet;
    use std::path::Path;

    fn entries(bytes: &[u8]) -> BTreeSet<String> {
        let mut out = BTreeSet::new();
        let mut tar = tar::Archive::new(GzDecoder::new(bytes));
        for e in tar.entries().unwrap() {
            let e = e.unwrap();
            if e.header().entry_type().is_file() {
                out.insert(e.path().unwrap().to_string_lossy().into_owned());
            }
        }
        out
    }

    #[test]
    fn packs_files_relative_to_subdir() {
        let root = crate::testutil::scratch("archive-rel");
        crate::testutil::touch_with(&root, "backend/index.ts", "hello");
        crate::testutil::touch_with(&root, "backend/agent/system.md", "md");
        let tar = pack_dir(&root, Path::new("backend")).unwrap();
        let files = entries(&tar);
        assert!(files.contains("index.ts"), "got {files:?}");
        assert!(files.contains("agent/system.md"), "got {files:?}");
    }

    #[test]
    fn excludes_all_blocklisted_entries() {
        let root = crate::testutil::scratch("archive-excl");
        crate::testutil::touch_with(&root, "backend/index.ts", "keep");
        crate::testutil::touch_with(&root, "backend/node_modules/pkg/a.js", "nope");
        crate::testutil::touch_with(&root, "backend/.git/HEAD", "nope");
        crate::testutil::touch_with(&root, "backend/.rootcx/cache", "nope");
        crate::testutil::touch_with(&root, "backend/bun.lock", "nope");
        crate::testutil::touch_with(&root, "backend/src-tauri/conf", "nope");
        let tar = pack_dir(&root, Path::new("backend")).unwrap();
        let files = entries(&tar);
        for bad in ["node_modules", ".git", ".rootcx", "bun.lock", "src-tauri"] {
            assert!(!files.iter().any(|f| f.contains(bad)), "{bad} leaked: {files:?}");
        }
        assert!(files.contains("index.ts"));
    }
}
