//! `docs/specs/media-runtime.md` §4: a discard in the media path has a counter or a reason.
//!
//! This is a source enumeration because the dangerous site is the one a later author adds and no
//! runtime test knows to exercise. Runtime tests prove individual counters rise; this guard proves
//! the list cannot grow silently.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::path::{Path, PathBuf};

const LOOKBACK: usize = 10;
const MARKER: &str = "// discard:";
const LOSS_WORDS: &[&str] = &[
    "dropping",
    "dropped",
    "discard",
    "ignoring",
    "ignored",
    "malformed",
    "refus",
    "failed",
    "could not be sent",
];

#[derive(Debug)]
struct Site {
    file: PathBuf,
    line: usize,
    text: String,
}

fn sources() -> Vec<PathBuf> {
    fn visit(directory: &Path, files: &mut Vec<PathBuf>) {
        for entry in std::fs::read_dir(directory).expect("a media source directory") {
            let path = entry.expect("a media source entry").path();
            if path.is_dir() {
                visit(&path, files);
            } else if path.extension().is_some_and(|extension| extension == "rs") {
                files.push(path);
            }
        }
    }

    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    visit(&root, &mut files);
    files.sort();
    assert!(files.len() > 10, "the scan found almost no media sources");
    files
}

fn production_lines(body: &str) -> Vec<&str> {
    let lines: Vec<&str> = body.lines().collect();
    let end = lines
        .iter()
        .position(|line| line.trim() == "#[cfg(test)]")
        .unwrap_or(lines.len());
    lines[..end].to_vec()
}

fn unexplained(path: &Path) -> Vec<Site> {
    let body = std::fs::read_to_string(path).expect("a media source file");
    let lines = production_lines(&body);
    let mut found = Vec::new();

    for (index, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        let discarded_result = trimmed.starts_with("let _ =");
        let tracing = if trimmed.contains("tracing::") {
            let mut statement = Vec::new();
            for near in lines
                .get(index..=(index + LOOKBACK).min(lines.len().saturating_sub(1)))
                .unwrap_or(&[])
            {
                statement.push(*near);
                if near.contains(");") {
                    break;
                }
            }
            statement.join(" ")
        } else {
            String::new()
        };
        let reported_loss =
            !tracing.is_empty() && LOSS_WORDS.iter().any(|word| tracing.contains(word));
        if !discarded_result && !reported_loss {
            continue;
        }

        let from = index.saturating_sub(LOOKBACK);
        let context = lines.get(from..=index).unwrap_or(&[]);
        let explained = context.iter().any(|near| {
            near.contains(MARKER)
                || near.contains("discards.")
                || near.contains("self.discards")
                || near.contains("fetch_add(")
        });
        if !explained {
            found.push(Site {
                file: path.to_path_buf(),
                line: index + 1,
                text: trimmed.to_owned(),
            });
        }
    }
    found
}

#[test]
fn no_discard_in_the_media_path_is_silent() {
    let unexplained: Vec<Site> = sources()
        .iter()
        .flat_map(|path| unexplained(path))
        .collect();
    if !unexplained.is_empty() {
        let listing = unexplained
            .iter()
            .map(|site| {
                format!(
                    "  {}:{}\n      {}",
                    site.file.display(),
                    site.line,
                    site.text
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        panic!(
            "{} media discard site(s) have neither a counter nor a reason:\n{listing}",
            unexplained.len()
        );
    }
}

#[test]
fn the_scan_detects_an_unexplained_discard() {
    let directory =
        std::env::temp_dir().join(format!("sipx-media-discards-{}", std::process::id()));
    std::fs::create_dir_all(&directory).expect("scratch directory");
    let cases = [
        ("bare.rs", "fn f() {\n let _ = send();\n}\n", true),
        (
            "counted.rs",
            "fn f() {\n discards.queue.fetch_add(1, Ordering::Relaxed);\n let _ = send();\n}\n",
            false,
        ),
        (
            "reasoned.rs",
            "fn f() {\n // discard: no observer exists.\n let _ = send();\n}\n",
            false,
        ),
        (
            "logged.rs",
            "fn f() {\n tracing::debug!(\"dropping a packet\");\n}\n",
            true,
        ),
    ];
    for (name, body, should_fire) in cases {
        let path = directory.join(name);
        std::fs::write(&path, body).expect("writes fixture");
        assert_eq!(!unexplained(&path).is_empty(), should_fire, "{name}");
    }
    std::fs::remove_dir_all(directory).ok();
}
