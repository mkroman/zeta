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

use std::io;
use std::fmt;
use std::error;

use toml;

#[derive(Debug)]
pub enum ManifestError {
    Io(io::Error),
    ParserError(toml::ParserError),
    ParserErrors(String),
    TomlValue(String),
}

#[derive(Debug)]
pub enum Error {
    Io(io::Error),
    Manifest(ManifestError)
}

impl error::Error for ManifestError {
    fn description(&self) -> &str {
        match *self {
            ManifestError::Io(ref error) => error.description(),
            ManifestError::ParserErrors(ref string) => string,
            ManifestError::TomlValue(ref string) => string,
            ManifestError::ParserError(ref error) => error.description(),
        }
    }

    fn cause(&self) -> Option<&error::Error> {
        match *self {
            ManifestError::Io(ref error) => Some(error),
            ManifestError::ParserError(ref error) => Some(error),
            _ => None,
        }
    }
}

impl fmt::Display for ManifestError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match *self {
            ManifestError::Io(ref error) => write!(f, "IO error: {}", error),
            ManifestError::ParserErrors(ref string) => write!(f, "Could not parse TOML\n{}", string),
            ManifestError::ParserError(ref error) => write!(f, "Parser error: {}", error),
            ManifestError::TomlValue(ref string) => write!(f, "{}", string),
        }
    }
}

impl From<io::Error> for ManifestError {
    fn from(error: io::Error) -> ManifestError {
        ManifestError::Io(error)
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match *self {
            Error::Io(ref error) => write!(f, "IO error: {}", error),
            Error::Manifest(ref error) => write!(f, "Manifest error: {}", error)
        }
    }
}

impl error::Error for Error {
    fn description(&self) -> &str {
        match *self {
            Error::Io(ref error) => error.description(),
            Error::Manifest(ref error) => error.description(),
        }
    }

    fn cause(&self) -> Option<&error::Error> {
        match *self {
            Error::Io(ref error) => Some(error),
            Error::Manifest(ref error) => Some(error)
        }
    }
}

impl From<io::Error> for Error {
    fn from(error: io::Error) -> Error {
        Error::Io(error)
    }
}

impl From<ManifestError> for Error {
    fn from(error: ManifestError) -> Error {
        Error::Manifest(error)
    }
}