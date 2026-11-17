// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The seccomp filter: `socket()` and `socketpair()` with the domain `AF_INET`
//! or `AF_INET6` kill the whole process with `SIGSYS`; every other system call
//! is allowed.
//!
//! The filter is compiled for the architecture this program was built for and
//! kills a process that makes a system call through another architecture's
//! interface (seccompiler checks the audit architecture first). On x86-64 the
//! x32 numbers of both calls are covered as well.

use std::collections::BTreeMap;
use std::env;

use seccompiler::{
    BpfProgram, SeccompAction, SeccompCmpArgLen, SeccompCmpOp, SeccompCondition, SeccompFilter,
    SeccompRule, TargetArch,
};

/// The socket domains that end the process.
const BLOCKED_DOMAINS: [libc::c_int; 2] = [libc::AF_INET, libc::AF_INET6];

/// Installs the filter on the calling thread. It is inherited across `exec`
/// and by every process and thread started afterwards, and cannot be removed.
///
/// seccompiler sets `no_new_privs` first, which an unprivileged process needs
/// before it may install a filter.
pub(crate) fn install() -> Result<(), seccompiler::Error> {
    let program = build()?;
    seccompiler::apply_filter(&program)
}

/// Compiles the filter for the architecture of this build.
pub(crate) fn build() -> Result<BpfProgram, seccompiler::Error> {
    let arch = TargetArch::try_from(env::consts::ARCH)?;
    let mut rules = BTreeMap::new();
    for syscall in blocked_syscalls() {
        let mut domain_rules = Vec::new();
        for domain in BLOCKED_DOMAINS {
            // The domain is the first argument, a C `int`: compare 32 bits.
            let condition = SeccompCondition::new(
                0,
                SeccompCmpArgLen::Dword,
                SeccompCmpOp::Eq,
                u64::from(domain.unsigned_abs()),
            )?;
            domain_rules.push(SeccompRule::new(vec![condition])?);
        }
        rules.insert(syscall, domain_rules);
    }
    let filter = SeccompFilter::new(
        rules,
        SeccompAction::Allow,
        SeccompAction::KillProcess,
        arch,
    )?;
    Ok(BpfProgram::try_from(filter)?)
}

/// The system call numbers that create sockets.
fn blocked_syscalls() -> Vec<i64> {
    let native = [libc::SYS_socket, libc::SYS_socketpair];
    #[cfg(target_arch = "x86_64")]
    {
        // The x32 interface uses the same numbers with this bit set and
        // reports the x86-64 audit architecture, so it needs its own rules.
        const X32_SYSCALL_BIT: i64 = 0x4000_0000;
        let mut all = native.to_vec();
        all.extend(native.iter().map(|number| number | X32_SYSCALL_BIT));
        all
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        native.to_vec()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_filter_compiles_for_this_architecture() {
        let program = build().unwrap();
        assert!(!program.is_empty());
    }

    #[test]
    fn both_socket_calls_are_covered() {
        let syscalls = blocked_syscalls();
        assert!(syscalls.contains(&libc::SYS_socket));
        assert!(syscalls.contains(&libc::SYS_socketpair));
    }
}
