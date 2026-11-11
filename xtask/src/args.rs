//! A tiny, dependency-free parser for `--flag` and `--option value` arguments.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};
use crate::workspace_root;

/// Parsed command-line options, validated against the command's known names.
pub struct Options {
    values: BTreeMap<String, String>,
    flags: BTreeSet<String>,
}

impl Options {
    pub fn parse(args: &[String], value_names: &[&str], flag_names: &[&str]) -> Result<Self> {
        let mut values = BTreeMap::new();
        let mut flags = BTreeSet::new();
        let mut iter = args.iter();

        while let Some(argument) = iter.next() {
            let Some(name) = argument.strip_prefix("--") else {
                return Err(Error::new(format!("unexpected argument `{argument}`")));
            };

            if flag_names.contains(&name) {
                flags.insert(name.to_owned());
            } else if value_names.contains(&name) {
                let value = iter
                    .next()
                    .ok_or_else(|| Error::new(format!("option `--{name}` requires a value")))?;
                if values.insert(name.to_owned(), value.clone()).is_some() {
                    return Err(Error::new(format!("option `--{name}` was given twice")));
                }
            } else {
                return Err(Error::new(format!("unknown option `--{name}`")));
            }
        }

        Ok(Self { values, flags })
    }

    pub fn value(&self, name: &str) -> Option<&str> {
        self.values.get(name).map(String::as_str)
    }

    pub fn flag(&self, name: &str) -> bool {
        self.flags.contains(name)
    }

    pub fn path(&self, name: &str) -> Option<PathBuf> {
        self.value(name)
            .map(|value| resolve_from_root(Path::new(value)))
    }

    pub fn path_or(&self, name: &str, default: &str) -> PathBuf {
        self.path(name)
            .unwrap_or_else(|| resolve_from_root(Path::new(default)))
    }
}

/// Resolve a possibly relative path against the workspace root instead of the caller's cwd.
fn resolve_from_root(path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_owned()
    } else {
        workspace_root().join(path)
    }
}
