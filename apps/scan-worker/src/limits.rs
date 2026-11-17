// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The resource limits the worker sets on itself.

use std::io;

use rlimit::Resource;

/// The address-space limit when none is given: 1 GiB.
pub(crate) const DEFAULT_MEMORY_LIMIT: u64 = 1 << 30;

/// The address-space limit from the command line: `--memory-limit BYTES`, 0 for
/// none. Without the option, [`DEFAULT_MEMORY_LIMIT`].
pub(crate) fn memory_limit_from_args(
    mut args: impl Iterator<Item = String>,
) -> Result<Option<u64>, String> {
    let mut limit = Some(DEFAULT_MEMORY_LIMIT);
    while let Some(arg) = args.next() {
        let value = match arg.strip_prefix("--memory-limit=") {
            Some(value) => value.to_owned(),
            None if arg == "--memory-limit" => args
                .next()
                .ok_or("--memory-limit needs a number of bytes")?,
            None => return Err(format!("unknown argument {arg:?}")),
        };
        let bytes: u64 = value
            .parse()
            .map_err(|_| format!("--memory-limit: not a number of bytes: {value:?}"))?;
        limit = (bytes > 0).then_some(bytes);
    }
    Ok(limit)
}

/// Limits the address space to `memory_limit` bytes and turns core dumps off.
///
/// The hard limits are lowered too, so code running later in the worker cannot
/// raise them again.
pub(crate) fn apply(memory_limit: Option<u64>) -> io::Result<()> {
    rlimit::setrlimit(Resource::CORE, 0, 0)?;
    if let Some(bytes) = memory_limit {
        rlimit::setrlimit(Resource::AS, bytes, bytes)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Option<u64>, String> {
        memory_limit_from_args(args.iter().map(|s| (*s).to_owned()))
    }

    #[test]
    fn arguments() {
        assert_eq!(parse(&[]), Ok(Some(DEFAULT_MEMORY_LIMIT)));
        assert_eq!(parse(&["--memory-limit", "1000"]), Ok(Some(1000)));
        assert_eq!(parse(&["--memory-limit=0"]), Ok(None));
        assert!(parse(&["--memory-limit"]).is_err());
        assert!(parse(&["--memory-limit", "lots"]).is_err());
        assert!(parse(&["--other"]).is_err());
    }
}
