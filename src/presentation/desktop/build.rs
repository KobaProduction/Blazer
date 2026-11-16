#[cfg(windows)]
fn main() {
    use std::{env, path::PathBuf};

    let target = env::var("TARGET").expect("Cargo must set TARGET");
    if !target.ends_with("-windows-msvc") {
        return;
    }

    println!("cargo::rerun-if-changed=assets/blazer.rc");
    println!("cargo::rerun-if-changed=assets/blazer.ico");
    let resource =
        PathBuf::from(env::var_os("OUT_DIR").expect("Cargo must set OUT_DIR")).join("blazer.res");
    let compiler = find_msvc_tools::find_tool(&target, "rc.exe")
        .expect("Windows SDK resource compiler (rc.exe) is required to build the Blazer icon");
    let status = compiler
        .to_command()
        .current_dir("assets")
        .arg("/nologo")
        .arg("/fo")
        .arg(&resource)
        .arg("blazer.rc")
        .status()
        .expect("Run the Windows SDK resource compiler");
    assert!(status.success(), "Compile the Blazer icon resource");
    println!("cargo::rustc-link-arg-bins={}", resource.display());
}

#[cfg(not(windows))]
fn main() {}
