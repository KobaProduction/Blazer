#[cfg(windows)]
fn main() {
    use std::{env, fs, path::PathBuf};

    let target = env::var("TARGET").expect("Cargo must set TARGET");
    if !target.ends_with("-windows-msvc") {
        return;
    }

    println!("cargo::rerun-if-changed=assets/blazer.rc");
    println!("cargo::rerun-if-changed=assets/blazer.ico");
    println!("cargo::rerun-if-env-changed=BLAZER_RELEASE_VERSION");
    let output = PathBuf::from(env::var_os("OUT_DIR").expect("Cargo must set OUT_DIR"));
    let resource = output.join("blazer.res");
    let version = env::var("BLAZER_RELEASE_VERSION").unwrap_or_else(|_| {
        concat!(
            env!("CARGO_PKG_VERSION_MAJOR"),
            ".",
            env!("CARGO_PKG_VERSION_MINOR")
        )
        .to_owned()
    });
    let parts: Vec<u16> = version
        .split('.')
        .map(str::parse)
        .collect::<Result<_, _>>()
        .expect("Windows requires a numeric version with 16-bit components");
    let [major, minor] = parts.as_slice() else {
        panic!("Expected version X.Y")
    };
    fs::write(
        output.join("blazer-version.h"),
        format!("#define BLAZER_FILE_VERSION {major},{minor},0,0\n#define BLAZER_VERSION \"{version}\"\n"),
    )
    .expect("Write the Windows version resource header");
    let compiler = find_msvc_tools::find_tool(&target, "rc.exe")
        .expect("Windows SDK resource compiler (rc.exe) is required to build the Blazer icon");
    let status = compiler
        .to_command()
        .current_dir("assets")
        .arg("/nologo")
        .arg("/i")
        .arg(&output)
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
