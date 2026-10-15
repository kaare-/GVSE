use std::path::Path;

#[test]
fn core_sources_do_not_touch_the_wall_clock_or_the_disk() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut bad = Vec::new();
    walk(&root, &mut bad);
    assert!(bad.is_empty(), "{bad:?}");
}

#[test]
fn trimmed_core_stays_under_twenty_thousand_lines() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut n = 0usize;
    count_lines(&root, &mut n);
    assert!(n < 20_000, "{n} lines");
}

fn walk(dir: &Path, bad: &mut Vec<String>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let entry = entry.unwrap();
        let path = entry.path();
        if path.is_dir() {
            walk(&path, bad);
            continue;
        }
        if path.extension().and_then(|e| e.to_str()) != Some("rs") {
            continue;
        }
        let text = std::fs::read_to_string(&path).unwrap();
        for needle in ["std::time", "std::fs", "rayon"] {
            if text.contains(needle) {
                bad.push(format!("{} contains {needle}", path.display()));
            }
        }
    }
}

fn count_lines(dir: &Path, n: &mut usize) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let entry = entry.unwrap();
        let path = entry.path();
        if path.is_dir() {
            count_lines(&path, n);
            continue;
        }
        if path.extension().and_then(|e| e.to_str()) == Some("rs") {
            let text = std::fs::read_to_string(&path).unwrap();
            *n += text.lines().count();
        }
    }
}
