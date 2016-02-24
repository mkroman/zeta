// Copyright (c) 2015, Mikkel Kroman <mk@uplink.io>
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
use std::fmt;

enum Fragment<'a> {
    /// Optional argument.
    ///
    /// "[optional_arg]"
    Optional(&'a str),
    /// Required argument.
    ///
    /// "<required_arg>"
    Required(&'a str),
    /// Literal string.
    ///
    /// "literal_string"
    Literal(&'a str)
}

impl<'a> fmt::Display for Fragment<'a> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match *self {
            Fragment::Optional(ref name) => write!(f, "[{}]", name),
            Fragment::Required(ref name) => write!(f, "<{}>", name),
            Fragment::Literal(ref string) => write!(f, "{}", string)
        }
    }
}

pub struct Route<'a> {
    fragments: Vec<Fragment<'a>>
}

impl<'a> Route<'a> {
    /// Create a new route with fragments from a given syntax.
    ///
    /// Returns a static str describing the error if the parsing of the fragments fail.
    pub fn new(syntax: &str) -> Result<Route, &'static str> {
        if syntax.is_empty() {
            return Err("Expected non-empty input");
        }

        let mut fragments = vec![];

        for token in syntax.split_whitespace() {
            let chr = token.chars().nth(0).unwrap();

            match chr {
                '<' => {
                    if let Some(end) = token.find('>') {
                        let name = &token[1..end];

                        match fragments.last() {
                            Some(&Fragment::Optional(_)) => {
                                return Err("Can't have a required fragment right after a optional one - try adding a literal inbetween");
                            },
                            Some(&Fragment::Required(_)) => {
                                return Err("Can't have a required fragment right after a optional one - try adding a literal inbetween");
                            },
                            _ => fragments.push(Fragment::Required(name))
                        }
                    } else {
                        return Err("Invalid end character, expected '>'");
                    }
                },
                '[' => {
                    if let Some(end) = token.find(']') {
                        let name = &token[1..end];
                        
                        match fragments.last() {
                            Some(&Fragment::Optional(_)) => {
                                return Err("Can't have an optional fragment right after a required one - try adding a literal inbetween");
                            },
                            Some(&Fragment::Required(_)) => {
                                return Err("Can't have an optional fragment right after a required one - try adding a literal inbetween");
                            },
                            _ => fragments.push(Fragment::Optional(name))
                        }
                    } else {
                        return Err("Invalid end character, expected ']'");
                    }
                }
                _ => {
                    fragments.push(Fragment::Literal(token));
                }
            }
        }

        Ok(Route { fragments: fragments })
    }

    /// Check whether a string matches the route.
    pub fn matches(&self, line: &str) {
        let tokens = line.split_whitespace();

        for fragment in &self.fragments {
            match *fragment {
                Fragment::Literal(ref string) => {
                },
                Fragment::Optional(ref name) => {
                },
                Fragment::Required(ref name) => {
                }
            }
        }
    }
}

impl<'a> fmt::Display for Route<'a> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let mut ret = String::new();

        for fragment in &self.fragments {
            ret.push_str(format!("{} ", *fragment).as_ref());
        }

        write!(f, "{}", ret.trim_right())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::Fragment;

    #[test]
    fn test_optional_arg_matches() {
        let _ = Route::new(".g [query]").unwrap();
    }

    #[test]
    #[should_panic]
    fn test_optional_after_required() {
        let _ = Route::new(".g [query] <required>").unwrap();
    }

    #[test]
    #[should_panic]
    fn test_required_after_optional() {
        let _ = Route::new(".g <required> [optional]").unwrap();
    }

    #[test]
    fn test_required_after_optional_after_literal() {
        let _ = Route::new(".g [optional] hello <ddd>").unwrap();
    }

    #[test]
    fn test_display_for_optional_fragment() { 
        assert_eq!(format!("{}", Fragment::Optional("optional")), "[optional]");
    }

    #[test]
    fn test_display_for_required_fragment() { 
        assert_eq!(format!("{}", Fragment::Required("required")), "<required>");
    }

    #[test]
    fn test_display_for_literal_fragment() { 
        assert_eq!(format!("{}", Fragment::Literal("literal")), "literal");
    }

    #[test]
    fn test_display_for_route() {
        let route = Route::new(".g [optional] hello <d>").unwrap();
        assert_eq!(format!("{}", route), ".g [optional] hello <d>");
    }
}
