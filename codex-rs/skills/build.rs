use std::fs;
use std::path::Path;

fn main() {
    let assets_dir = Path::new("src/assets");
    if !assets_dir.exists() {
        return;
    }

    println!("cargo:rerun-if-changed={}", assets_dir.display());
    visit_dir(assets_dir);
}

fn visit_dir(dir: &Path) {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => return,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        println!("cargo:rerun-if-changed={}", path.display());
        if path.is_dir() {
            visit_dir(&path);
        }
    }
}
