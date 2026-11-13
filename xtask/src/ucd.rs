//! Fetch the five Unicode Character Database files used by the Nomina trainer.
//!
//! The files are downloaded with the system `curl`, so the xtask needs no third-party
//! dependencies. Windows 10 and later ship `curl.exe`, and Git for Windows bundles one as a
//! fallback.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

use zeta_unicode_format::{DERIVED_NAME, EMOJI_TEST, INPUTS};

use crate::cli::FetchUcdArgs;
use crate::error::{Error, Result};

/// The candidate paths the trainer probes before the files this task writes.
///
/// If any of them exists in the destination, training would silently read stale data, so the
/// destination must be scratch. The paths come from the shared
/// [`zeta_unicode_format::InputSpec`] table, in the order the trainer probes them.
fn stale_paths() -> impl Iterator<Item = &'static str> {
    INPUTS
        .iter()
        .flat_map(|spec| spec.candidates_before_fetch().iter().copied())
}

#[cfg(windows)]
const CURL_HINT: &str = "curl is required to fetch the UCD files; install curl or Git for \
                         Windows (Windows 10 and later already ship curl.exe)";
#[cfg(not(windows))]
const CURL_HINT: &str = "curl is required to fetch the UCD files; install curl";

/// Fetch a Unicode release into the scratch directory in `args`.
///
/// # Errors
///
/// Returns an error for invalid versions, a destination that already contains trainer inputs,
/// a missing `curl`, a failed download, or files whose release headers do not match.
pub fn run(args: &FetchUcdArgs) -> Result<()> {
    let curl = resolve_curl()?;
    execute(args, |url, path| curl_fetch(&curl, url, path))
}

fn execute(args: &FetchUcdArgs, mut fetch: impl FnMut(&str, &Path) -> Result<()>) -> Result<()> {
    if !is_version(&args.version, 3) {
        return Err(Error::new(format!(
            "Unicode version must be <major>.<minor>.<patch>, got {}",
            args.version
        )));
    }
    let emoji_version = args
        .emoji_version
        .as_deref()
        .unwrap_or_else(|| default_emoji_version(&args.version));
    if !is_version(emoji_version, 2) {
        return Err(Error::new(format!(
            "Emoji version must be <major>.<minor>, got {emoji_version}"
        )));
    }

    ensure_scratch(&args.dest)?;

    let public = format!("https://www.unicode.org/Public/{}", args.version);
    let emoji_public = format!("https://www.unicode.org/Public/emoji/{emoji_version}");

    for spec in INPUTS {
        let destination = under(&args.dest, spec.fetch_path);
        let url = format!("{public}/{}", spec.fetch_path);
        // Newer releases publish emoji-test under `/Public/<version>/emoji`; older ones only
        // carry it in the standalone emoji release directory.
        if spec.file == EMOJI_TEST.file {
            if fetch(&url, &destination).is_err() {
                fetch(&format!("{emoji_public}/{}", spec.file), &destination)?;
            }
        } else {
            fetch(&url, &destination)?;
        }
    }

    validate_files(&args.dest, &args.version, emoji_version)?;

    println!(
        "Fetched Unicode {} (emoji {emoji_version}) into {}",
        args.version,
        args.dest.display()
    );
    println!("Train with:");
    println!(
        "  cargo run -p zeta-unicode-build --bin zeta-unicode-train -- --data-dir \"{}\" \
         --out-dir \"target/nomina-{}\" --report --verify",
        args.dest.display(),
        args.version
    );
    Ok(())
}

/// Resolve the `curl` program, with a Git for Windows fallback when it is not on PATH.
fn resolve_curl() -> Result<PathBuf> {
    if Command::new("curl")
        .arg("--version")
        .output()
        .is_ok_and(|output| output.status.success())
    {
        return Ok(PathBuf::from("curl"));
    }

    #[cfg(windows)]
    {
        for candidate in [
            r"C:\Program Files\Git\mingw64\bin\curl.exe",
            r"C:\Program Files (x86)\Git\mingw64\bin\curl.exe",
        ] {
            let path = Path::new(candidate);
            if path.is_file() {
                return Ok(path.to_path_buf());
            }
        }
    }

    Err(Error::new(CURL_HINT))
}

fn curl_fetch(curl: &Path, url: &str, path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let part = part_path(path);

    let output = match Command::new(curl)
        .args([
            "-fsSL",
            "--retry",
            "3",
            "--connect-timeout",
            "15",
            "--max-time",
            "300",
            "--proto",
            "=https",
            "-o",
        ])
        .arg(&part)
        .arg(url)
        .output()
    {
        Ok(output) => output,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Err(Error::new(CURL_HINT));
        }
        Err(error) => return Err(Error::new(format!("failed to run curl: {error}"))),
    };

    if !output.status.success() {
        let _ = fs::remove_file(&part);
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(Error::new(format!(
            "failed to fetch {url}: {}",
            stderr.trim()
        )));
    }

    replace_file(&part, path)
}

/// Move a completed download into place, working around Windows' no-overwrite rename.
fn replace_file(part: &Path, path: &Path) -> Result<()> {
    if path.exists() {
        fs::remove_file(path)?;
    }
    fs::rename(part, path)?;
    Ok(())
}

fn ensure_scratch(dest: &Path) -> Result<()> {
    for relative in stale_paths() {
        let path = under(dest, relative);
        if path.exists() {
            return Err(Error::new(format!(
                "{} already exists; {} must be a scratch directory. The trainer probes this \
                 path before the fetched file, so training would keep using it. Update the \
                 vendored data by copying the five UCD files instead.",
                path.display(),
                dest.display()
            )));
        }
    }
    Ok(())
}

fn validate_files(dest: &Path, version: &str, emoji_version: &str) -> Result<()> {
    for spec in INPUTS {
        let path = under(dest, spec.fetch_path);
        let metadata = fs::metadata(&path).map_err(|error| {
            Error::new(format!(
                "{} is missing or unreadable: {error}",
                path.display()
            ))
        })?;
        if metadata.len() == 0 {
            return Err(Error::new(format!(
                "{} is missing or empty",
                path.display()
            )));
        }
    }

    let derived_name = fs::read_to_string(under(dest, DERIVED_NAME.fetch_path))?;
    if !has_derived_name_version(&derived_name, version) {
        return Err(Error::new(format!(
            "DerivedName.txt does not mention Unicode {version}"
        )));
    }
    let emoji_test = fs::read_to_string(under(dest, EMOJI_TEST.fetch_path))?;
    if !has_emoji_version(&emoji_test, emoji_version) {
        return Err(Error::new(format!(
            "emoji-test.txt does not mention Emoji {emoji_version}"
        )));
    }
    Ok(())
}

#[must_use]
fn is_version(value: &str, components: usize) -> bool {
    let mut parts = value.split('.');
    (0..components).all(|_| {
        parts
            .next()
            .is_some_and(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))
    }) && parts.next().is_none()
}

#[must_use]
fn default_emoji_version(version: &str) -> &str {
    version
        .rsplit_once('.')
        .map_or(version, |(major_minor, _)| major_minor)
}

#[must_use]
fn has_derived_name_version(contents: &str, version: &str) -> bool {
    let marker = format!("# DerivedName-{version}");
    contents.lines().any(|line| line.contains(&marker))
}

#[must_use]
fn has_emoji_version(contents: &str, version: &str) -> bool {
    let expected = format!("# Version: {version}");
    let contents = contents.strip_prefix('\u{feff}').unwrap_or(contents);
    contents.lines().any(|line| line.trim_end() == expected)
}

#[must_use]
fn under(dest: &Path, relative: &str) -> PathBuf {
    relative
        .split('/')
        .fold(dest.to_path_buf(), |path, segment| path.join(segment))
}

#[must_use]
fn part_path(path: &Path) -> PathBuf {
    let mut part = path.as_os_str().to_owned();
    part.push(".part");
    PathBuf::from(part)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static NEXT: AtomicUsize = AtomicUsize::new(0);

    fn scratch_dir(name: &str) -> PathBuf {
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("xtask-ucd-{}-{name}-{id}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("create scratch directory");
        dir
    }

    fn args(dest: &Path) -> FetchUcdArgs {
        FetchUcdArgs {
            version: "18.0.0".to_owned(),
            dest: dest.to_path_buf(),
            emoji_version: None,
        }
    }

    fn write(path: &Path, contents: &str) {
        fs::create_dir_all(path.parent().expect("file parent")).expect("create parent");
        fs::write(path, contents).expect("write file");
    }

    fn successful_fetcher(urls: &mut Vec<String>) -> impl FnMut(&str, &Path) -> Result<()> + '_ {
        move |url, path| {
            urls.push(url.to_owned());
            match path.file_name().and_then(|name| name.to_str()) {
                Some("DerivedName.txt") => write(path, "# DerivedName-18.0.0.txt\n"),
                Some("emoji-test.txt") => {
                    if url.ends_with("/emoji/18.0/emoji-test.txt") {
                        write(path, "# Version: 18.0\n");
                    } else {
                        return Err(Error::new("emoji-test is not in the UCD tree"));
                    }
                }
                _ => write(path, "data\n"),
            }
            Ok(())
        }
    }

    #[test]
    fn validates_versions() {
        assert!(is_version("18.0.0", 3));
        assert!(is_version("10.0.0", 3));
        for value in ["", "18", "18.0", "18.0.0.0", "18..0", "a.b.c", "18.0.x"] {
            assert!(!is_version(value, 3), "{value:?} should be invalid");
        }
        assert!(is_version("16.0", 2));
        assert!(!is_version("16", 2));
        assert_eq!(default_emoji_version("18.0.0"), "18.0");
        assert_eq!(default_emoji_version("10.0.0"), "10.0");
    }

    #[test]
    fn stale_paths_are_every_candidate_before_a_fetch_path() {
        assert_eq!(
            stale_paths().collect::<Vec<_>>(),
            [
                "DerivedName.txt",
                "extracted/DerivedName.txt",
                "ucd/DerivedName.txt",
                "DerivedGeneralCategory.txt",
                "extracted/DerivedGeneralCategory.txt",
                "ucd/DerivedGeneralCategory.txt",
                "NameAliases.txt",
                "extracted/NameAliases.txt",
                "NamedSequences.txt",
                "extracted/NamedSequences.txt",
                "emoji-test.txt",
            ]
        );
    }

    #[test]
    fn checks_release_headers() {
        assert!(has_derived_name_version(
            "\u{feff}# a comment\n# DerivedName-18.0.0.txt\n",
            "18.0.0"
        ));
        assert!(!has_derived_name_version(
            "# DerivedName-17.0.0.txt",
            "18.0.0"
        ));
        assert!(has_emoji_version("# Version: 16.0  \r\n", "16.0"));
        assert!(has_emoji_version("\u{feff}# Version: 16.0\n", "16.0"));
        assert!(!has_emoji_version("# Version: 16.0.1\r\n", "16.0"));
    }

    #[test]
    fn replaces_an_existing_destination() {
        let dir = scratch_dir("replace");
        let part = dir.join("file.part");
        let path = dir.join("file");
        write(&part, "new");
        write(&path, "old");

        replace_file(&part, &path).expect("replace the file");

        assert_eq!(fs::read_to_string(&path).expect("read target"), "new");
        assert!(!part.exists());
        fs::remove_dir_all(dir).expect("clean up");
    }

    #[test]
    fn rejects_a_non_scratch_destination() {
        let dest = scratch_dir("stale");
        write(&dest.join("DerivedName.txt"), "old\n");
        let mut fetched = false;
        let error = execute(&args(&dest), |_, _| {
            fetched = true;
            Ok(())
        })
        .expect_err("stale files must be rejected");
        assert!(error.to_string().contains("scratch directory"));
        assert!(!fetched, "nothing should be downloaded");
        fs::remove_dir_all(dest).expect("clean up");
    }

    #[test]
    fn fetches_every_file_and_falls_back_for_emoji() {
        let dest = scratch_dir("ok");
        let mut urls = Vec::new();
        execute(&args(&dest), successful_fetcher(&mut urls)).expect("fetch succeeds");

        for spec in INPUTS {
            let path = under(&dest, spec.fetch_path);
            assert!(path.is_file(), "{} should exist", path.display());
            assert!(fs::metadata(&path).expect("metadata").len() > 0);
        }
        assert!(urls.contains(
            &"https://www.unicode.org/Public/18.0.0/ucd/extracted/DerivedName.txt".to_owned()
        ));
        assert!(
            urls.contains(&"https://www.unicode.org/Public/emoji/18.0/emoji-test.txt".to_owned())
        );
        assert!(
            urls.contains(&"https://www.unicode.org/Public/18.0.0/emoji/emoji-test.txt".to_owned())
        );
        fs::remove_dir_all(dest).expect("clean up");
    }

    #[test]
    fn rejects_a_mismatched_release_header() {
        let dest = scratch_dir("header");
        let error = execute(&args(&dest), |url, path| {
            let contents = if path.ends_with("DerivedName.txt") {
                "# DerivedName-17.0.0.txt\n"
            } else if path.ends_with("emoji-test.txt") {
                "# Version: 18.0\n"
            } else {
                "data\n"
            };
            write(path, contents);
            let _ = url;
            Ok(())
        })
        .expect_err("mismatched release must be rejected");
        assert!(
            error
                .to_string()
                .contains("does not mention Unicode 18.0.0")
        );
        fs::remove_dir_all(dest).expect("clean up");
    }
}
