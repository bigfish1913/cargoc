// Tiny Cargo stand-in used by integration tests. Compiled with rustc, no Python/shell.
use std::{env, fs, path::PathBuf, process, thread, time::Duration};
fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    if args.iter().any(|s| s == "--version") { println!("cargo 1.99.0 (test)"); return; }
    if args.iter().any(|s| s == "locate-project") {
        println!("{}", env::var("FAKE_MANIFEST").unwrap()); return;
    }
    let workspace = PathBuf::from(env::var_os("FAKE_WORKSPACE").unwrap());
    let dir = PathBuf::from(env::var_os("CARGO_BUILD_BUILD_DIR").unwrap());
    fs::create_dir_all(dir.join("debug/deps")).unwrap();
    fs::write(dir.join("debug/.cargo-lock"), b"").unwrap();
    let cached = dir.join("debug/deps/cache.bin");
    fs::write(&cached, vec![42_u8; 4096]).unwrap();
    fs::create_dir_all(workspace.join("target/debug")).unwrap();
    fs::write(workspace.join("target/debug/demo.exe"), "final artifact").unwrap();
    fs::write(env::var_os("FAKE_ARGUMENTS").unwrap(), format!("{args:?}")).unwrap();
    fs::write(workspace.join("started"), "started").unwrap();
    match env::var("FAKE_MODE").unwrap_or_default().as_str() {
        "fail" => process::exit(7),
        "wait" => {
            thread::sleep(Duration::from_secs(2));
            if !cached.is_file() { process::exit(55); }
        },
        "post-gc-error" => {
            let root = dir.parent().unwrap().parent().unwrap();
            fs::write(root.join("state/invalid.json"), "{}").unwrap();
            process::exit(7);
        },
        _ => {},
    }
}
