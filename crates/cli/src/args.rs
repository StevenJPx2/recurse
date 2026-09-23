//! Hand-rolled argument parsing: positionals plus a fixed set of flags.

use std::collections::BTreeMap;

use recurse_protocol::{Target, TargetKind};

const VALUE_FLAGS: &[&str] = &["--target-id", "--target-kind", "--timeout", "--port", "-c"];
const BOOL_FLAGS: &[&str] = &["--full"];

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Args {
    pub positional: Vec<String>,
    flags: BTreeMap<String, String>,
}

impl Args {
    pub fn parse(tokens: &[String]) -> Result<Self, String> {
        let mut args = Self::default();
        let mut tokens = tokens.iter();

        while let Some(token) = tokens.next() {
            let (name, inline) = match token.split_once('=') {
                Some((name, value)) if name.starts_with("--") => (name, Some(value.to_owned())),
                _ => (token.as_str(), None),
            };

            if VALUE_FLAGS.contains(&name) {
                let value = match inline {
                    Some(value) => value,
                    None => tokens
                        .next()
                        .cloned()
                        .ok_or_else(|| format!("{name} needs a value"))?,
                };

                args.flags.insert(name.to_owned(), value);
            } else if BOOL_FLAGS.contains(&name) {
                args.flags.insert(name.to_owned(), String::new());
            } else if name.starts_with('-') && name != "-" {
                return Err(format!("unknown flag {name}"));
            } else {
                args.positional.push(token.clone());
            }
        }

        Ok(args)
    }

    pub fn value(&self, flag: &str) -> Option<&str> {
        self.flags.get(flag).map(String::as_str)
    }

    pub fn has(&self, flag: &str) -> bool {
        self.flags.contains_key(flag)
    }

    pub fn number(&self, flag: &str) -> Result<Option<u64>, String> {
        self.value(flag)
            .map(|value| {
                value
                    .parse()
                    .map_err(|_| format!("{flag} expects a number, got {value}"))
            })
            .transpose()
    }

    /// `--target-kind` (default `cli`) and `--target-id` (default: hostname).
    pub fn target(&self) -> Result<Target, String> {
        let kind = match self.value("--target-kind") {
            Some(kind) => {
                TargetKind::parse(kind).ok_or_else(|| format!("unknown target kind {kind}"))?
            }
            None => TargetKind::Cli,
        };
        let id = self
            .value("--target-id")
            .map_or_else(hostname, str::to_owned);
        let target = Target::new(kind, id);

        if target.is_valid() {
            Ok(target)
        } else {
            Err("target id must be 1-256 characters".into())
        }
    }
}

pub fn hostname() -> String {
    let mut buffer = [0u8; 256];

    // SAFETY: the buffer is valid for `len` bytes and gethostname NUL-terminates within it or
    // truncates; we read only up to the first NUL.
    let result = unsafe { libc::gethostname(buffer.as_mut_ptr().cast(), buffer.len()) };
    let end = buffer
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(buffer.len());
    let name = String::from_utf8_lossy(buffer.get(..end).unwrap_or_default()).into_owned();

    if result == 0 && !name.is_empty() {
        name
    } else {
        "localhost".into()
    }
}

#[cfg(test)]
mod tests {
    use super::Args;

    fn parse(tokens: &[&str]) -> Result<Args, String> {
        Args::parse(
            &tokens
                .iter()
                .map(|token| (*token).to_owned())
                .collect::<Vec<_>>(),
        )
    }

    #[test]
    fn parses_flags_and_positionals() {
        let args = parse(&[
            "--target-id",
            "box",
            "-c",
            "1 + 1",
            "--timeout=5",
            "--full",
            "-",
        ])
        .unwrap();

        assert_eq!(args.value("-c"), Some("1 + 1"));
        assert_eq!(args.number("--timeout").unwrap(), Some(5));
        assert!(args.has("--full"));
        assert_eq!(args.positional, ["-"]);
        assert_eq!(args.target().unwrap().key(), "cli:box");
        assert!(parse(&["--nope"]).is_err());
        assert!(parse(&["--port"]).is_err());
        assert!(
            parse(&["--target-kind", "bogus"])
                .unwrap()
                .target()
                .is_err()
        );
        assert!(!super::hostname().is_empty());
    }
}
