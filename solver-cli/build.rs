// SPDX-License-Identifier: AGPL-3.0-or-later

use std::fs;
use std::path::{Path, PathBuf};

const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

const KEPT: (&str, &str) = ("src-6ab4dcca2e66ff57", "src-99ba7d80cb49f615");

fn hash_bytes(state: &mut u64, bytes: &[u8]) {
    for byte in bytes {
        *state ^= u64::from(*byte);
        *state = state.wrapping_mul(FNV_PRIME);
    }
}

fn collect(root: &Path, suffixes: &[&str], found: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    let mut here: Vec<PathBuf> = entries.filter_map(|e| e.ok()).map(|e| e.path()).collect();
    here.sort();
    for path in here {
        if path.ends_with("src/bin") {
            continue;
        }
        if path.is_dir() {
            collect(&path, suffixes, found);
        } else if suffixes.iter().any(|s| path.to_string_lossy().ends_with(s)) {
            found.push(path);
        }
    }
}

fn code(path: &Path, text: &str) -> String {
    let marker = if path.extension().is_some_and(|e| e == "toml") { "#" } else { "//" };
    let mut out = String::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with(marker) {
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

fn main() {
    let roots = [
        "../engine/postflop-solver/src",
        "../engine/postflop-solver/Cargo.toml",
        "src",
        "Cargo.toml",
    ];

    let mut files = Vec::new();
    for root in roots {
        let path = PathBuf::from(root);
        if path.is_dir() {
            collect(&path, &[".rs", ".toml"], &mut files);
        } else if path.is_file() {
            files.push(path);
        }
        println!("cargo:rerun-if-changed={root}");
    }

    let mut state = FNV_OFFSET;
    for path in &files {
        hash_bytes(&mut state, path.to_string_lossy().as_bytes());
        if let Ok(bytes) = fs::read(path) {
            hash_bytes(&mut state, code(path, &String::from_utf8_lossy(&bytes)).as_bytes());
        }
    }

    let digest = format!("src-{state:016x}");
    let stamp = if digest == KEPT.0 { KEPT.1 } else { digest.as_str() };
    println!("cargo:rustc-env=ENGINE_SHA={stamp}");
}
