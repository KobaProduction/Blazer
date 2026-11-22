#[cfg(target_os = "linux")]
mod app;
#[cfg(target_os = "linux")]
mod catalog;

#[cfg(target_os = "linux")]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args_os().skip(1);
    let Some(argument) = args.next() else {
        return Ok(app::run(None)?);
    };
    let import = argument
        .to_str()
        .and_then(vpn_presentation::import::import_link)
        .filter(|_| args.next().is_none())
        .ok_or("usage: blazer-linux [blazer://add/{url}]")?;
    Ok(app::run(Some(import.into_owned()))?)
}

#[cfg(not(target_os = "linux"))]
fn main() -> std::process::ExitCode {
    eprintln!("blazer-linux requires Linux");
    std::process::ExitCode::FAILURE
}
