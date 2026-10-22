//! Regression test for: `samply record` SIGKILLs `dsymutil`.
//!
//! samply injects `DYLD_INSERT_LIBRARIES` (+ `SAMPLY_BOOTSTRAP_SERVER_NAME`) into
//! the *entire* descendant process tree it launches. Any descendant that loads
//! the preload hands its mach task port to samply and lets it "control us
//! completely" (see `samply-mac-preload`). For `dsymutil` this takeover ends in a
//! deterministic `SIGKILL`, which breaks builds run under `samply record` on
//! macOS (the linker invokes `dsymutil` and reports `running dsymutil failed:
//! signal: killed`).
//!
//! This test launches, under the built `samply` binary, a small locally-built
//! "spawner" that execs `dsymutil` on a Mach-O with DWARF, and asserts that
//! `dsymutil` is NOT killed (the desired behaviour).
//!
//! macOS-only (`cfg(target_os = "macos")`), so it compiles out elsewhere. It
//! needs Xcode's `dsymutil` and a working `cc`, both present on `macos-latest`
//! CI runners. Launch-mode profiling does not need `task_for_pid` entitlements
//! (the child volunteers its task port), so no `samply setup` is required.
//!
//! Run with:
//!   cargo test -p samply --test dsymutil_sigkill -- --nocapture
#![cfg(target_os = "macos")]

use std::path::Path;
use std::process::Command;

fn cc(args: &[&str]) {
    let status = Command::new("cc").args(args).status().expect("failed to run cc");
    assert!(status.success(), "cc {args:?} failed");
}

fn xcrun_dsymutil() -> String {
    let out = Command::new("xcrun")
        .args(["-f", "dsymutil"])
        .output()
        .expect("failed to run xcrun");
    assert!(out.status.success(), "xcrun -f dsymutil failed");
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}

#[test]
fn dsymutil_is_not_killed_under_samply() {
    let samply = env!("CARGO_BIN_EXE_samply");
    let dsymutil = xcrun_dsymutil();

    let tmp = std::env::temp_dir().join(format!("samply_dsym_repro_{}", std::process::id()));
    std::fs::create_dir_all(&tmp).unwrap();

    // A Mach-O with enough DWARF that dsymutil does real work.
    let src = tmp.join("big.cpp");
    {
        use std::fmt::Write as _;
        let mut s = String::from("#include <cstdio>\n");
        for i in 0..1200 {
            writeln!(s, "template<int N> struct S{i} {{ int v[N%7+1]; int f(int x){{return x*{i}+N;}} }};").unwrap();
            writeln!(s, "int g{i}(int x){{ S{i}<{}> s; return s.f(x)+{i}; }}", i % 9 + 1).unwrap();
        }
        s.push_str("int main(){int t=0;");
        for i in 0..1200 {
            write!(s, "t+=g{i}(t);").unwrap();
        }
        s.push_str("printf(\"%d\\n\",t);return 0;}\n");
        std::fs::write(&src, s).unwrap();
    }
    let macho = tmp.join("bigcpp");
    cc(&["-g", "-O0", "-o", macho.to_str().unwrap(), src.to_str().unwrap()]);

    // A locally-built (non-restricted) parent that execs dsymutil and reports
    // how the child died via its own exit code: 0 = clean, 1 = killed by signal.
    let spawner_src = tmp.join("spawner.c");
    std::fs::write(
        &spawner_src,
        r#"
#include <stdio.h>
#include <stdlib.h>
#include <unistd.h>
#include <sys/wait.h>
int main(int argc, char** argv){
    pid_t pid = fork();
    if(pid==0){ execl(argv[1],"dsymutil","-f",argv[2],"-o",argv[3],(char*)0); _exit(127); }
    int st=0; waitpid(pid,&st,0);
    if(WIFSIGNALED(st)){ fprintf(stderr,"dsymutil killed by signal %d\n", WTERMSIG(st)); return 1; }
    fprintf(stderr,"dsymutil exited code %d\n", WEXITSTATUS(st)); return 0;
}
"#,
    )
    .unwrap();
    let spawner = tmp.join("spawner");
    cc(&["-O0", "-o", spawner.to_str().unwrap(), spawner_src.to_str().unwrap()]);

    let out_dwarf = tmp.join("out.dwarf");
    let profile = tmp.join("profile.json.gz");

    // samply record --save-only -o <profile> -- <spawner> <dsymutil> <macho> <out.dwarf>
    let status = Command::new(samply)
        .args(["record", "--save-only", "-o"])
        .arg(&profile)
        .arg("--")
        .arg(&spawner)
        .arg(&dsymutil)
        .arg(&macho)
        .arg(&out_dwarf)
        .status()
        .expect("failed to run samply");

    // The spawner exits 0 iff dsymutil completed normally. Under the bug it exits
    // 1 because dsymutil was SIGKILLed by samply.
    let killed = !status.success();
    let produced_output = Path::new(&out_dwarf).exists();

    // Clean up only after we've inspected the results (out.dwarf lives in `tmp`).
    let _ = std::fs::remove_dir_all(&tmp);

    assert!(
        !killed,
        "dsymutil was killed when run under `samply record` \
         (spawner exit = {:?}). samply must not SIGKILL build subprocesses it \
         injects into.",
        status.code()
    );
    assert!(produced_output, "dsymutil did not produce its output");
}
