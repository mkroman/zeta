// Copyright (c) 2016, Mikkel Kroman <mk@uplink.io>
// All rights reserved.
//
// Redistribution and use in source and binary forms, with or without
// modification, are permitted provided that the following conditions are met:
//
// * Redistributions of source code must retain the above copyright notice, this
//   list of conditions and the following disclaimer.
//
// * Redistributions in binary form must reproduce the above copyright notice,
//   this list of conditions and the following disclaimer in the documentation
//   and/or other materials provided with the distribution.
//
// THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS"
// AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE
// IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE ARE
// DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT HOLDER OR CONTRIBUTORS BE LIABLE
// FOR ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL
// DAMAGES (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR
// SERVICES; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER
// CAUSED AND ON ANY THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY,
// OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE
// OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.

use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

use toml;

use error::{Error, ManifestError};

#[derive(Debug)]
pub struct Plugin {
    pub name: String,
    pub version: String,
    pub authors: Vec<String>,
    pub source_path: PathBuf,
    pub build_path: Option<PathBuf>,
    pub dependencies: toml::Table,
}

impl Plugin {
    /// Creates a new `Plugin` by reading `Plugin.toml` in the provided `plugin_source_dir`.
    pub fn from_dir<P: AsRef<Path>>(plugin_source_dir: P) -> Result<Plugin, Error> {
        let plugin_source_dir = plugin_source_dir.as_ref();
        let toml_path = plugin_source_dir.join("Plugin.toml");
        let buffer = {
            let mut file = File::open(&toml_path).map_err(ManifestError::Io)?;
            let mut buf = String::new();

            file.read_to_string(&mut buf)?;
            buf
        };
        let mut parser = toml::Parser::new(&buffer);

        let plugin = match parser.parse() {
            Some(value) => Plugin::from_toml_table(toml::Value::Table(value), &plugin_source_dir)?,
            None => {
                let mut err_msg = String::new();

                for parser_error in &parser.errors {
                    let (line, column) = parser.to_linecol(parser_error.lo);

                    err_msg.push_str(&format!("{}:{}:{} {}\n", &toml_path.display(),
                        line, column, parser_error.desc));
                }

                return Err(ManifestError::ParserErrors(err_msg).into());
            }
        };

        Ok(plugin)
    }

    /// Creates a new `Plugin` with associated information of a TOML table.
    fn from_toml_table(value: toml::Value, path: &Path) -> Result<Plugin, Error> {
        let plugin_name = match value.lookup("plugin.name") {
            Some(&toml::Value::String(ref string)) => string,
            _ => return Err(ManifestError::TomlValue(
                format!("Expected key `plugin.name` to be a string")).into())
        };

        let plugin_version = match value.lookup("plugin.version") {
            Some(&toml::Value::String(ref string)) => string,
            _ => return Err(ManifestError::TomlValue(
                format!("Expected key `plugin.version` to be a string")).into())
        };

        let plugin_authors = match value.lookup("plugin.authors") {
            Some(&toml::Value::Array(ref array)) => array,
            _ => return Err(ManifestError::TomlValue(
                format!("Expected key `plugin.authors` to be an array")).into())
        };

        let dependencies = value.lookup("dependencies").unwrap().as_table().unwrap().clone();

        Ok(Plugin {
            name: plugin_name.to_owned(),
            version: plugin_version.to_owned(),
            authors: plugin_authors.iter().map(|v| v.as_str().unwrap().to_owned()).collect(),
            source_path: path.to_path_buf(),
            build_path: None,
            dependencies: dependencies,
        })
    }
}