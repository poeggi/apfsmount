// SPDX-License-Identifier: AGPL-3.0-or-later
//! The command line tool. See `win_mountkit::cli`.

fn main() -> std::process::ExitCode {
    apfsmount::init();
    apfsmount::win_mountkit::cli::main()
}
